//! Application-plan compiler for the dedicated BYOK credential fence.

use aex_session_app::plan::{Condition, TableFamily};
use aex_session_dynamodb::application_plan::{
    AuthorityBinding, ExternalActionCompiler, LogicalAction,
};
use aex_session_dynamodb::error::StoreError;
use aex_session_dynamodb::plan::{Participant, RegionalTables, TransactionPlan, key};
use aws_sdk_dynamodb::types::{AttributeValue, ConditionCheck};

/// Compiles one read-only exact binding/state/revision fence against the
/// provider-credential directory.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProviderCredentialAdmissionCompiler;

impl ExternalActionCompiler for ProviderCredentialAdmissionCompiler {
    fn compile_action(
        &self,
        tables: &RegionalTables,
        binding: AuthorityBinding,
        action: &LogicalAction<'_>,
        output: &mut TransactionPlan,
    ) -> Result<(), StoreError> {
        if action.target.family != TableFamily::SecretCustody
            || action.write.is_some()
            || action.conditions.len() != 1
        {
            return Err(invalid());
        }
        let Condition::ProviderCredentialReady {
            workspace,
            provider,
            credential,
            source_generation,
            revision,
        } = action.conditions[0].1
        else {
            return Err(invalid());
        };
        if *workspace != binding.workspace {
            return Err(StoreError::Invalid {
                detail: "a provider-credential fence crosses its authenticated workspace"
                    .to_owned(),
            });
        }
        let physical =
            crate::keys::provider_credential(*workspace, provider.as_str(), *credential)?;
        output.condition_check(
            Participant::CUSTODY_PROVIDER_CREDENTIAL,
            ConditionCheck::builder()
                .table_name(&tables.regional_secret_custody)
                .set_key(Some(key(&physical.pk, &physical.sk)))
                .condition_expression(
                    "attribute_exists(pk) AND workspaceId = :workspace AND provider = :provider \
                     AND credentialId = :credential AND sourceGeneration = :generation \
                     AND revision = :revision AND #state = :ready",
                )
                .expression_attribute_names("#state", "state")
                .expression_attribute_values(":workspace", AttributeValue::S(workspace.to_string()))
                .expression_attribute_values(
                    ":provider",
                    AttributeValue::S(provider.as_str().to_owned()),
                )
                .expression_attribute_values(
                    ":credential",
                    AttributeValue::S(credential.to_string()),
                )
                .expression_attribute_values(
                    ":generation",
                    AttributeValue::N(source_generation.to_string()),
                )
                .expression_attribute_values(":revision", AttributeValue::N(revision.to_string()))
                .expression_attribute_values(
                    ":ready",
                    AttributeValue::S(crate::codec::CredentialState::Ready.as_str().to_owned()),
                ),
        )?;
        Ok(())
    }
}

fn invalid() -> StoreError {
    StoreError::Invalid {
        detail: "the provider-credential compiler accepts one read-only ready-binding guard"
            .to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use aex_session_app::plan::{Condition, ConditionId};
    use aex_session_dynamodb::application_plan::{
        AuthorityBinding, ExternalActionCompiler, LogicalAction,
    };
    use aex_session_dynamodb::plan::{Participant, RegionalTables, TransactionPlan};
    use aex_wire::ids::{
        OrganizationId, PrefixedId as _, ProviderCredentialId, SessionId, Uuid7, WorkspaceId,
    };
    use aex_wire::provider::ProviderId;

    use super::ProviderCredentialAdmissionCompiler;

    #[test]
    fn exact_ready_revision_is_one_named_condition_check() {
        let id = |tag| Uuid7::compose(1, [tag; 10]);
        let workspace = WorkspaceId::from_uuid7(id(1));
        let organization = OrganizationId::from_uuid7(id(2));
        let session = SessionId::from_uuid7(id(3));
        let credential = ProviderCredentialId::from_uuid7(id(8));
        let condition = Condition::ProviderCredentialReady {
            workspace,
            provider: ProviderId::Openai,
            credential,
            source_generation: 4,
            revision: 9,
        };
        let action = LogicalAction {
            target: condition.target(),
            conditions: vec![(ConditionId(0), &condition)],
            write: None,
        };
        let custody = ProviderCredentialAdmissionCompiler;
        let mut transaction = TransactionPlan::new("credential-admission-test");
        custody
            .compile_action(
                &RegionalTables::composed("dev", "eu-west-1"),
                AuthorityBinding {
                    workspace,
                    organization,
                    session: Some(session),
                },
                &action,
                &mut transaction,
            )
            .expect("the exact credential fence compiles");
        assert_eq!(
            transaction.participants(),
            [Participant::CUSTODY_PROVIDER_CREDENTIAL]
        );
        let check = transaction.actions()[0]
            .condition_check()
            .expect("read-only guard");
        assert!(check.condition_expression().contains("sourceGeneration"));
        assert!(check.condition_expression().contains("#state = :ready"));
    }
}
