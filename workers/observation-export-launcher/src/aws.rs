//! The production implementations of the two ports, over the real `AWS` clients.
//!
//! Every expression string is assembled through
//! [`aex_observation_store_dynamodb::expressions::ExpressionBuilder`], so an attribute
//! name reaches `DynamoDB` as a generated `#n0` placeholder and a value as a
//! generated `:v0` placeholder. Nothing customer-supplied is ever interpolated
//! into an expression.
//!
//! The due-scan is a `Query` against the sparse `gsi_control` index. There is no
//! `Scan` here, and there is nowhere for one to hide: the launcher holds no
//! observation read permission, so a full-table read would be denied by `IAM`
//! before it could be slow.

use aex_observation_store_dynamodb::expressions::{ExpressionBuilder, ITEM_TYPE, Index, PK, SK};
use aex_wire::types::Timestamp;
use aws_sdk_dynamodb::types::{AttributeValue, ReturnValue};
use aws_sdk_ecs::types::{
    AssignPublicIp, AwsVpcConfiguration, ContainerOverride, DesiredStatus, KeyValuePair,
    LaunchType, NetworkConfiguration, TaskOverride,
};

use crate::launcher::{
    Claim, ClaimOutcome, DueItem, ExportRows, LaunchRequest, LauncherError, Lease, RecordOutcome,
    RunTaskOutcome, STATE_ADMITTED, STATE_LAUNCHING, TaskLauncher, TaskStatus, due_upper_bound,
    parse_control_sk, parse_export_key, shard_partition,
};

/// The `EXPORT#` and `CTRL#` rows, over `DynamoDB`.
#[derive(Clone, Debug)]
pub struct DynamoExportRows {
    client: aws_sdk_dynamodb::Client,
    table: String,
}

impl DynamoExportRows {
    /// Binds the adapter to one table.
    #[must_use]
    pub const fn new(client: aws_sdk_dynamodb::Client, table: String) -> Self {
        Self { client, table }
    }

    /// Proves the table is reachable and correctly named.
    ///
    /// # Errors
    ///
    /// Returns [`LauncherError::Rows`] when the table cannot be described.
    pub async fn probe(&self) -> Result<(), LauncherError> {
        self.client
            .describe_table()
            .table_name(self.table.as_str())
            .send()
            .await
            .map(|_| ())
            .map_err(|error| LauncherError::Rows {
                operation: "DescribeTable",
                reason: error.to_string(),
            })
    }
}

#[async_trait::async_trait]
impl ExportRows for DynamoExportRows {
    async fn due(
        &self,
        shard: u8,
        now: Timestamp,
        limit: usize,
    ) -> Result<Vec<DueItem>, LauncherError> {
        let mut builder = ExpressionBuilder::new();
        let partition_name = builder.name(Index::Control.partition_key());
        let sort_name = builder.name(Index::Control.sort_key());
        let partition = builder.string(shard_partition(shard));
        let ceiling = builder.string(due_upper_bound(now)?);
        let condition = format!("{partition_name} = {partition} AND {sort_name} < {ceiling}");
        let page = i32::try_from(limit).unwrap_or(i32::MAX);

        let response = self
            .client
            .query()
            .table_name(self.table.as_str())
            .index_name(Index::Control.as_str())
            .key_condition_expression(condition)
            .set_expression_attribute_names(Some(builder.names()))
            .set_expression_attribute_values(Some(builder.values()))
            .limit(page)
            .send()
            .await
            .map_err(|error| LauncherError::Rows {
                operation: "due scan",
                reason: error.to_string(),
            })?;

        let mut due = Vec::new();
        for item in response.items() {
            let pk = attribute(item, PK)?;
            let sk = attribute(item, Index::Control.sort_key())?;
            let (workspace, export) = parse_export_key(pk, attribute(item, SK)?)?;
            due.push(DueItem {
                workspace,
                export,
                due_at: parse_control_sk(sk)?,
            });
        }
        Ok(due)
    }

    async fn claim(&self, item: &DueItem, lease: &Lease) -> Result<ClaimOutcome, LauncherError> {
        let mut builder = ExpressionBuilder::new();
        let state = builder.name("state");
        let fence = builder.name("fence");
        let owner = builder.name("leaseOwner");
        let expires = builder.name("leaseExpiresAt");
        let cancelled = builder.name("cancelRequested");
        let launching = builder.string(STATE_LAUNCHING);
        let admitted = builder.string(STATE_ADMITTED);
        let one = builder.number(1);
        let me = builder.string(lease.owner.clone());
        let until = builder.number(lease.expires_at.unix_millis());
        let evaluated_at = builder.number(lease.now.unix_millis());
        let not_cancelled = builder.boolean(false);

        let update = format!(
            "SET {state} = {launching}, {fence} = {fence} + {one}, \
             {owner} = {me}, {expires} = {until}"
        );
        let condition = format!(
            "{state} IN ({admitted}, {launching}) AND \
             (attribute_not_exists({expires}) OR {expires} < {evaluated_at}) AND \
             {cancelled} = {not_cancelled}"
        );

        let response = self
            .client
            .update_item()
            .table_name(self.table.as_str())
            .key(PK, AttributeValue::S(item.state_pk()))
            .key(SK, AttributeValue::S(item.state_sk()))
            .update_expression(update)
            .condition_expression(condition)
            .set_expression_attribute_names(Some(builder.names()))
            .set_expression_attribute_values(Some(builder.values()))
            .return_values(ReturnValue::AllNew)
            .send()
            .await;

        match response {
            Ok(updated) => {
                let attributes = updated.attributes();
                let fence = attributes
                    .and_then(|values| values.get("fence"))
                    .and_then(|value| value.as_n().ok())
                    .and_then(|text| text.parse::<u64>().ok())
                    .ok_or_else(|| LauncherError::Rows {
                        operation: "claim",
                        reason: "the claimed row carries no numeric fence".to_owned(),
                    })?;
                Ok(ClaimOutcome::Won(Claim { fence }))
            }
            // Losing the condition is the normal outcome of a race, a live lease
            // or a requested cancel. It is never retried into a second launch.
            Err(error)
                if error
                    .as_service_error()
                    .is_some_and(aws_sdk_dynamodb::operation::update_item::UpdateItemError::is_conditional_check_failed_exception) =>
            {
                Ok(ClaimOutcome::Lost)
            }
            Err(error) => Err(LauncherError::Rows {
                operation: "claim",
                reason: error.to_string(),
            }),
        }
    }

    async fn record_task(
        &self,
        item: &DueItem,
        claim: Claim,
        task_arn: &str,
    ) -> Result<RecordOutcome, LauncherError> {
        let mut builder = ExpressionBuilder::new();
        let task = builder.name("taskArn");
        let fence = builder.name("fence");
        let arn = builder.string(task_arn);
        let held = builder.number(claim.fence);
        let update = format!("SET {task} = {arn}");
        let condition = format!("{fence} = {held}");

        let response = self
            .client
            .update_item()
            .table_name(self.table.as_str())
            .key(PK, AttributeValue::S(item.state_pk()))
            .key(SK, AttributeValue::S(item.state_sk()))
            .update_expression(update)
            .condition_expression(condition)
            .set_expression_attribute_names(Some(builder.names()))
            .set_expression_attribute_values(Some(builder.values()))
            .send()
            .await;

        match response {
            Ok(_) => Ok(RecordOutcome::Recorded),
            Err(error)
                if error
                    .as_service_error()
                    .is_some_and(aws_sdk_dynamodb::operation::update_item::UpdateItemError::is_conditional_check_failed_exception) =>
            {
                Ok(RecordOutcome::Superseded)
            }
            Err(error) => Err(LauncherError::Rows {
                operation: "record task",
                reason: error.to_string(),
            }),
        }
    }
}

/// Reads one required string attribute from a returned item.
fn attribute<'a>(
    item: &'a std::collections::HashMap<String, AttributeValue>,
    name: &str,
) -> Result<&'a str, LauncherError> {
    item.get(name)
        .and_then(|value| value.as_s().ok())
        .map(String::as_str)
        .ok_or_else(|| LauncherError::Key {
            key: name.to_owned(),
            reason: format!("the index page carries no `{name}` string; `{ITEM_TYPE}` disagrees"),
        })
}

/// Classifies one answered `RunTask`.
///
/// A response that names a task is the only unambiguous one. A response
/// carrying placement failures is a refusal, and a response carrying neither is
/// unknown; both are reconciled by identity rather than by calling again.
fn classify(started: &aws_sdk_ecs::operation::run_task::RunTaskOutput) -> RunTaskOutcome {
    if let Some(task_arn) = started
        .tasks()
        .iter()
        .find_map(|task| task.task_arn().map(str::to_owned))
    {
        return RunTaskOutcome::Started { task_arn };
    }
    let failures: Vec<String> = started
        .failures()
        .iter()
        .map(|failure| {
            format!(
                "{}: {}",
                failure.reason().unwrap_or("unknown"),
                failure.detail().unwrap_or("no detail")
            )
        })
        .collect();
    if failures.is_empty() {
        RunTaskOutcome::Unknown {
            reason: "the response named neither a task nor a failure".to_owned(),
        }
    } else {
        RunTaskOutcome::Refused {
            reason: failures.join("; "),
        }
    }
}

/// The export cluster, over `ECS`.
#[derive(Clone, Debug)]
pub struct EcsTaskLauncher {
    client: aws_sdk_ecs::Client,
    cluster: String,
}

impl EcsTaskLauncher {
    /// Binds the adapter to one cluster.
    #[must_use]
    pub const fn new(client: aws_sdk_ecs::Client, cluster: String) -> Self {
        Self { client, cluster }
    }

    /// Proves the cluster and the task definition are both describable.
    ///
    /// # Errors
    ///
    /// Returns [`LauncherError::Tasks`] when either call fails or the cluster is
    /// not active. A launcher that cannot see its cluster never reports ready.
    pub async fn probe(&self, task_definition: &str) -> Result<(), LauncherError> {
        let clusters = self
            .client
            .describe_clusters()
            .clusters(self.cluster.as_str())
            .send()
            .await
            .map_err(|error| LauncherError::Tasks {
                operation: "DescribeClusters",
                reason: error.to_string(),
            })?;
        let active = clusters
            .clusters()
            .iter()
            .any(|cluster| cluster.status() == Some("ACTIVE"));
        if !active {
            return Err(LauncherError::Tasks {
                operation: "DescribeClusters",
                reason: format!("`{}` is not an ACTIVE cluster", self.cluster),
            });
        }
        self.client
            .describe_task_definition()
            .task_definition(task_definition)
            .send()
            .await
            .map(|_| ())
            .map_err(|error| LauncherError::Tasks {
                operation: "DescribeTaskDefinition",
                reason: error.to_string(),
            })
    }
}

#[async_trait::async_trait]
impl TaskLauncher for EcsTaskLauncher {
    async fn run_task(&self, request: &LaunchRequest<'_>) -> Result<RunTaskOutcome, LauncherError> {
        let identity = request.identity();
        let mut vpc = AwsVpcConfiguration::builder().assign_public_ip(AssignPublicIp::Disabled);
        for subnet in request.subnets {
            vpc = vpc.subnets(subnet.as_str());
        }
        for group in request.security_groups {
            vpc = vpc.security_groups(group.as_str());
        }
        let vpc = vpc.build().map_err(|error| LauncherError::Tasks {
            operation: "RunTask",
            reason: error.to_string(),
        })?;

        let mut container = ContainerOverride::builder().name(request.container);
        for (name, value) in request.overrides() {
            container =
                container.environment(KeyValuePair::builder().name(name).value(value).build());
        }

        let response = self
            .client
            .run_task()
            .cluster(request.cluster)
            .task_definition(request.task_definition)
            .launch_type(LaunchType::Fargate)
            .client_token(identity.clone())
            .started_by(identity)
            .count(1)
            .network_configuration(
                NetworkConfiguration::builder()
                    .awsvpc_configuration(vpc)
                    .build(),
            )
            .overrides(
                TaskOverride::builder()
                    .container_overrides(container.build())
                    .build(),
            )
            .send()
            .await;

        // The only unambiguous result is one that names the task it started.
        // Everything else is handed back for identity reconciliation; nothing
        // here ever calls `RunTask` a second time.
        Ok(match response {
            Ok(started) => classify(&started),
            Err(error) => RunTaskOutcome::Unknown {
                reason: error.to_string(),
            },
        })
    }

    async fn tasks_started_by(
        &self,
        cluster: &str,
        started_by: &str,
        status: TaskStatus,
    ) -> Result<Vec<String>, LauncherError> {
        let desired = DesiredStatus::from(status.as_str());
        let response = self
            .client
            .list_tasks()
            .cluster(cluster)
            .started_by(started_by)
            .desired_status(desired)
            .send()
            .await
            .map_err(|error| LauncherError::Tasks {
                operation: "ListTasks",
                reason: error.to_string(),
            })?;
        Ok(response.task_arns().to_vec())
    }
}

#[cfg(test)]
mod tests {
    use aex_wire::ids::{ExportId, PrefixedId, Uuid7, WorkspaceId};

    use crate::launcher::{EXPORT_ID_ENV, LaunchRequest, WORKSPACE_ID_ENV};

    fn request<'a>(subnets: &'a [String], groups: &'a [String]) -> LaunchRequest<'a> {
        LaunchRequest {
            cluster: "arn:aws:ecs:eu-west-1:123456789012:cluster/aex-dev-export",
            task_definition: "arn:aws:ecs:eu-west-1:123456789012:task-definition/t:1",
            container: "t",
            subnets,
            security_groups: groups,
            workspace: WorkspaceId::from_uuid7(Uuid7::compose(1, [1; 10])),
            export: ExportId::from_uuid7(Uuid7::compose(2, [2; 10])),
        }
    }

    #[test]
    fn the_started_by_marker_is_the_export_identity_itself() {
        let subnets = vec!["subnet-0a1b2c3d".to_owned()];
        let groups = vec!["sg-0123abcd".to_owned()];
        let request = request(&subnets, &groups);
        assert!(request.identity().starts_with("exp_"));
        // `ECS` caps `clientToken` and `startedBy` at 64 and 128 characters; a
        // prefixed `UUIDv7` is 30, so the identity always fits both.
        assert!(request.identity().len() <= 64);
    }

    #[test]
    fn the_container_environment_carries_no_credential_or_resource_name() {
        let subnets = vec!["subnet-0a1b2c3d".to_owned()];
        let groups = vec!["sg-0123abcd".to_owned()];
        let request = request(&subnets, &groups);
        let environment = request.overrides();
        assert_eq!(environment.len(), 2);
        assert_eq!(environment[0].0, EXPORT_ID_ENV);
        assert_eq!(environment[1].0, WORKSPACE_ID_ENV);
        for (name, _) in &environment {
            assert!(
                !name.contains("SECRET") && !name.contains("KEY") && !name.contains("TABLE"),
                "`{name}` must not travel in a container override"
            );
        }
    }
}
