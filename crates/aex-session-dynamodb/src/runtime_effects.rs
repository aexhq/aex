//! Exact open Hands-effect recount over `session-authority`.
//!
//! This adapter intentionally reads every durable effect row belonging to the
//! session's bounded agent registry. A filter-only count could silently hide a
//! Hands row whose generation binding was missing or malformed; this decoder
//! fails closed instead.

use aex_runtime_control::store::{OpenEffectCounter, RuntimeStoreError, StoreFuture};
use aex_wire::ids::{AgentId, GenerationId, PrefixedId as _, SessionId};
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::types::AttributeValue;
use futures::StreamExt as _;

use crate::error::{Idempotence, StoreError, classify};
use crate::keys;

const PAGE_ITEMS: i32 = 100;
const QUERY_CONCURRENCY: usize = 8;

/// Strongly consistent authoritative Hands-effect counter.
#[derive(Debug, Clone)]
pub struct OpenHandsEffectCounter {
    client: Client,
    table: String,
}

impl OpenHandsEffectCounter {
    /// Binds the counter to the physical session-authority table.
    #[must_use]
    pub fn new(client: Client, table: impl Into<String>) -> Self {
        Self {
            client,
            table: table.into(),
        }
    }

    /// The table this authority reader uses.
    #[must_use]
    pub fn table(&self) -> &str {
        &self.table
    }

    async fn session_agents(&self, session: SessionId) -> Result<Vec<AgentId>, RuntimeStoreError> {
        let mut exclusive_start_key = None;
        let mut agents = Vec::new();
        loop {
            let output = self
                .client
                .query()
                .table_name(&self.table)
                .key_condition_expression("pk = :pk AND begins_with(sk, :prefix)")
                .expression_attribute_values(
                    ":pk",
                    AttributeValue::S(keys::session_partition(session)),
                )
                .expression_attribute_values(":prefix", AttributeValue::S("AGENT#".to_owned()))
                .projection_expression("sk")
                .consistent_read(true)
                .limit(PAGE_ITEMS)
                .set_exclusive_start_key(exclusive_start_key)
                .send()
                .await
                .map_err(|error| runtime_error(classify(&error, Idempotence::Read)))?;
            for item in output.items() {
                let sort = item
                    .get("sk")
                    .and_then(|value| value.as_s().ok())
                    .ok_or_else(|| malformed("an agent index row has no string sk"))?;
                let raw = sort
                    .strip_prefix("AGENT#")
                    .ok_or_else(|| malformed("an agent index row is outside AGENT#"))?;
                agents.push(AgentId::parse(raw).map_err(|error| malformed(&error.to_string()))?);
            }
            exclusive_start_key = output.last_evaluated_key().cloned();
            if exclusive_start_key.is_none() {
                break;
            }
        }
        agents.sort_unstable();
        agents.dedup();
        Ok(agents)
    }

    async fn count_agent(
        &self,
        session: SessionId,
        agent: AgentId,
        generation: GenerationId,
    ) -> Result<u32, RuntimeStoreError> {
        let mut exclusive_start_key = None;
        let mut open = 0_u32;
        loop {
            let output = self
                .client
                .query()
                .table_name(&self.table)
                .key_condition_expression("pk = :pk AND begins_with(sk, :prefix)")
                .expression_attribute_values(
                    ":pk",
                    AttributeValue::S(keys::agent_partition(session, agent)),
                )
                .expression_attribute_values(":prefix", AttributeValue::S("EFFECT#".to_owned()))
                .projection_expression("kind, #state, generationId")
                .expression_attribute_names("#state", "state")
                .consistent_read(true)
                .limit(PAGE_ITEMS)
                .set_exclusive_start_key(exclusive_start_key)
                .send()
                .await
                .map_err(|error| runtime_error(classify(&error, Idempotence::Read)))?;
            for item in output.items() {
                if string(item, "kind")? != "HandsOperation" {
                    continue;
                }
                let bound = GenerationId::parse(string(item, "generationId")?)
                    .map_err(|error| malformed(&error.to_string()))?;
                let is_open = match string(item, "state")? {
                    "prepared" | "dispatched" | "responding" => true,
                    "settled" | "unknown" => false,
                    state => {
                        return Err(malformed(&format!(
                            "a Hands effect has unmodelled state `{state}`"
                        )));
                    }
                };
                if bound == generation && is_open {
                    open = open
                        .checked_add(1)
                        .ok_or_else(|| malformed("the open Hands-effect count exceeds u32"))?;
                }
            }
            exclusive_start_key = output.last_evaluated_key().cloned();
            if exclusive_start_key.is_none() {
                break;
            }
        }
        Ok(open)
    }
}

impl OpenEffectCounter for OpenHandsEffectCounter {
    fn count_open_hands_effects(
        &self,
        session: SessionId,
        generation: GenerationId,
    ) -> StoreFuture<'_, u32> {
        Box::pin(async move {
            let agents = self.session_agents(session).await?;
            let counts =
                futures::stream::iter(agents.into_iter().map(|agent| async move {
                    self.count_agent(session, agent, generation).await
                }))
                .buffer_unordered(QUERY_CONCURRENCY)
                .collect::<Vec<_>>()
                .await;
            counts.into_iter().try_fold(0_u32, |total, count| {
                total
                    .checked_add(count?)
                    .ok_or_else(|| malformed("the session open Hands-effect count exceeds u32"))
            })
        })
    }
}

fn string<'a>(
    item: &'a std::collections::HashMap<String, AttributeValue>,
    name: &str,
) -> Result<&'a str, RuntimeStoreError> {
    item.get(name)
        .and_then(|value| value.as_s().ok())
        .map(String::as_str)
        .ok_or_else(|| malformed(&format!("a Hands effect has no string `{name}`")))
}

fn runtime_error(error: StoreError) -> RuntimeStoreError {
    match error {
        StoreError::Corrupt(error) => malformed(&error.to_string()),
        StoreError::Invalid { detail } => malformed(&detail),
        StoreError::Key(error) => malformed(&error.to_string()),
        other => RuntimeStoreError::Unavailable {
            reason: other.to_string(),
        },
    }
}

fn malformed(reason: &str) -> RuntimeStoreError {
    RuntimeStoreError::Malformed {
        reason: reason.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::{OpenHandsEffectCounter, malformed, string};
    use aex_runtime_control::store::OpenEffectCounter as _;
    use aws_sdk_dynamodb::types::AttributeValue;

    #[test]
    fn a_missing_generation_binding_is_a_malformed_authority_row() {
        let row = std::collections::HashMap::from([
            (
                "kind".to_owned(),
                AttributeValue::S("HandsOperation".to_owned()),
            ),
            ("state".to_owned(), AttributeValue::S("prepared".to_owned())),
        ]);
        assert_eq!(
            string(&row, "generationId").expect_err("the binding is mandatory"),
            malformed("a Hands effect has no string `generationId`")
        );
    }

    #[test]
    fn the_production_counter_satisfies_the_runtime_port() {
        fn assert_port<T: aex_runtime_control::store::OpenEffectCounter>() {}
        assert_port::<OpenHandsEffectCounter>();
        let _ = OpenHandsEffectCounter::count_open_hands_effects;
    }
}
