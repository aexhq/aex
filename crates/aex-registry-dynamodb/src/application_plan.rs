//! Compilation of registry authority guards onto `regional-registry`.
//!
//! A download reads the current pointer before it mints a grant, then carries
//! the observed `ETag` into the same transaction as that grant. This compiler
//! turns that logical fence into the table owner's exact physical condition.

use aex_session_app::plan::Condition;
use aex_session_dynamodb::application_plan::{
    AuthorityBinding, ExternalActionCompiler, LogicalAction,
};
use aex_session_dynamodb::attr::s;
use aex_session_dynamodb::error::StoreError;
use aex_session_dynamodb::plan::{Participant, RegionalTables, TransactionPlan, key};
use aws_sdk_dynamodb::types::ConditionCheck;

/// Compiles registry pointer guards for cross-family application transactions.
#[derive(Debug, Clone, Copy, Default)]
pub struct RegistryAuthorityCompiler;

impl ExternalActionCompiler for RegistryAuthorityCompiler {
    fn compile_action(
        &self,
        tables: &RegionalTables,
        binding: AuthorityBinding,
        action: &LogicalAction<'_>,
        output: &mut TransactionPlan,
    ) -> Result<(), StoreError> {
        if action.write.is_some() {
            return Err(StoreError::Invalid {
                detail: "the registry download compiler accepts pointer guards only".to_owned(),
            });
        }
        let [(_, Condition::RegistryEtag { selector, expected })] = action.conditions.as_slice()
        else {
            return Err(StoreError::Invalid {
                detail: "a read-only registry action must carry exactly one pointer ETag guard"
                    .to_owned(),
            });
        };
        if selector.workspace != binding.workspace {
            return Err(StoreError::Invalid {
                detail: "a registry pointer guard crosses its authenticated workspace".to_owned(),
            });
        }
        let pointer =
            crate::keys::pointer(selector.workspace, selector.kind, selector.name.as_str())?;
        output.condition_check(
            Participant::REGISTRY_POINTER,
            ConditionCheck::builder()
                .table_name(&tables.regional_registry)
                .set_key(Some(key(&pointer.pk, &pointer.sk)))
                .condition_expression(
                    "#itemType = :pointer AND workspaceId = :workspace AND kind = :kind AND \
                     #name = :name AND etag = :etag",
                )
                .expression_attribute_names("#itemType", "itemType")
                .expression_attribute_names("#name", "name")
                .expression_attribute_values(":pointer", s(crate::codec::REGISTRY_POINTER))
                .expression_attribute_values(":workspace", s(selector.workspace.to_string()))
                .expression_attribute_values(":kind", s(selector.kind.as_str()))
                .expression_attribute_values(":name", s(selector.name.as_str()))
                .expression_attribute_values(":etag", s(expected.as_str())),
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use aex_content_domain::RegistryKind;
    use aex_session_app::plan::{Condition, SessionTransaction, TransactionIntent};
    use aex_session_dynamodb::application_plan::{
        FamilyCompilers, WorkspaceBinding, compile_workspace_transaction,
    };
    use aex_session_dynamodb::plan::{Participant, RegionalTables};
    use aex_wire::ids::{OrganizationId, PrefixedId, ResourceName, Uuid7, WorkspaceId};
    use aex_wire::types::ETag;
    use aex_workspace_domain::RegistrySelector;

    use super::RegistryAuthorityCompiler;

    fn id<T: PrefixedId>(byte: u8) -> T {
        T::from_uuid7(Uuid7::compose(1_754_051_696_789, [byte; 10]))
    }

    #[test]
    fn an_observed_pointer_becomes_one_exact_read_only_guard() {
        let workspace = id::<WorkspaceId>(1);
        let organization = id::<OrganizationId>(2);
        let plan = SessionTransaction {
            intent: TransactionIntent::StopSession,
            conditions: vec![Condition::RegistryEtag {
                selector: RegistrySelector {
                    workspace,
                    kind: RegistryKind::File,
                    name: ResourceName::parse("guide.pdf").expect("a name"),
                },
                expected: ETag::parse("0123456789abcdef0123456789abcdef").expect("an ETag"),
            }],
            writes: Vec::new(),
            after_commit: Vec::new(),
        };
        let registry = RegistryAuthorityCompiler;
        let compilers =
            FamilyCompilers::new().with(aex_session_app::plan::TableFamily::Registry, &registry);
        let tables = RegionalTables::composed("dev", "eu-west-1");
        let compiled = compile_workspace_transaction(
            &tables,
            &plan,
            WorkspaceBinding {
                workspace,
                organization,
            },
            &compilers,
        )
        .expect("the pointer guard compiles");

        assert_eq!(compiled.transaction.len(), 1);
        assert_eq!(
            compiled.transaction.participants(),
            [Participant::REGISTRY_POINTER]
        );
        let guard = compiled.transaction.actions()[0]
            .condition_check()
            .expect("a read-only check");
        assert_eq!(guard.table_name(), tables.regional_registry);
        assert!(guard.condition_expression().contains("etag = :etag"));
    }

    #[test]
    fn a_pointer_guard_cannot_cross_the_authenticated_workspace() {
        let workspace = id::<WorkspaceId>(1);
        let organization = id::<OrganizationId>(2);
        let plan = SessionTransaction {
            intent: TransactionIntent::StopSession,
            conditions: vec![Condition::RegistryEtag {
                selector: RegistrySelector {
                    workspace,
                    kind: RegistryKind::File,
                    name: ResourceName::parse("guide.pdf").expect("a name"),
                },
                expected: ETag::parse("0123456789abcdef0123456789abcdef").expect("an ETag"),
            }],
            writes: Vec::new(),
            after_commit: Vec::new(),
        };
        let registry = RegistryAuthorityCompiler;
        let compilers =
            FamilyCompilers::new().with(aex_session_app::plan::TableFamily::Registry, &registry);
        let error = compile_workspace_transaction(
            &RegionalTables::composed("dev", "eu-west-1"),
            &plan,
            WorkspaceBinding {
                workspace: id::<WorkspaceId>(9),
                organization,
            },
            &compilers,
        )
        .expect_err("tenant drift");

        assert!(matches!(
            error,
            aex_session_dynamodb::StoreError::Invalid { .. }
        ));
    }
}
