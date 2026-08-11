//! Compilation of application-plan content writes onto `regional-content`.
//!
//! A grant and its pin remain two logical actions so a cancellation names the
//! exact failed authority row. The root application compiler still submits
//! them with every other family as one `TransactWriteItems` request.

use aex_content_domain::Pin;
use aex_session_app::plan::{Condition, Write};
use aex_session_dynamodb::application_plan::{
    AuthorityBinding, ExternalActionCompiler, LogicalAction,
};
use aex_session_dynamodb::attr::s;
use aex_session_dynamodb::error::StoreError;
use aex_session_dynamodb::plan::{
    IMMUTABLE, Participant, RegionalTables, TransactionPlan, key, keyed,
};
use aws_sdk_dynamodb::types::{ConditionCheck, Put};

/// Compiles the content-authority portion of an application transaction.
#[derive(Debug, Clone, Copy, Default)]
pub struct ContentAuthorityCompiler;

impl ExternalActionCompiler for ContentAuthorityCompiler {
    fn compile_action(
        &self,
        tables: &RegionalTables,
        binding: AuthorityBinding,
        action: &LogicalAction<'_>,
        output: &mut TransactionPlan,
    ) -> Result<(), StoreError> {
        match action.write {
            None => check_content_owned(tables, binding, action, output),
            Some(Write::PutGrant(grant)) => {
                require_immutable_guard(action)?;
                put_grant(tables, binding, grant, output)
            }
            Some(Write::PutPin(pin)) if matches!(pin.as_ref(), Pin::Grant { .. }) => {
                require_immutable_guard(action)?;
                put_grant_pin(tables, binding, pin.as_ref(), output)
            }
            Some(_) => Err(StoreError::Invalid {
                detail: "the registry-download content compiler accepts one grant or grant pin"
                    .to_owned(),
            }),
        }
    }
}

fn require_immutable_guard(action: &LogicalAction<'_>) -> Result<(), StoreError> {
    if action
        .conditions
        .iter()
        .all(|(_, condition)| matches!(condition, Condition::ItemAbsent(_)))
    {
        return Ok(());
    }
    Err(StoreError::Invalid {
        detail: "a content grant write accepts only its immutable item guard".to_owned(),
    })
}

fn check_content_owned(
    tables: &RegionalTables,
    binding: AuthorityBinding,
    action: &LogicalAction<'_>,
    output: &mut TransactionPlan,
) -> Result<(), StoreError> {
    let [(_, Condition::ContentOwned { workspace, digest })] = action.conditions.as_slice() else {
        return Err(StoreError::Invalid {
            detail: "a read-only content action must carry exactly one ownership guard".to_owned(),
        });
    };
    if *workspace != binding.workspace {
        return Err(StoreError::Invalid {
            detail: "a content ownership guard crosses its authenticated workspace".to_owned(),
        });
    }
    let descriptor = crate::keys::descriptor(*workspace, digest);
    output.condition_check(
        Participant::CONTENT_DESCRIPTOR,
        ConditionCheck::builder()
            .table_name(&tables.regional_content)
            .set_key(Some(key(&descriptor.pk, &descriptor.sk)))
            .condition_expression(
                "#itemType = :descriptor AND workspaceId = :workspace AND organizationId = \
                 :organization AND digestSha256 = :digest AND #state = :committed AND placement = \
                 :s3 AND attribute_exists(objectKey)",
            )
            .expression_attribute_names("#itemType", "itemType")
            .expression_attribute_names("#state", "state")
            .expression_attribute_values(":descriptor", s(crate::codec::CONTENT_DESCRIPTOR))
            .expression_attribute_values(":workspace", s(workspace.to_string()))
            .expression_attribute_values(":organization", s(binding.organization.to_string()))
            .expression_attribute_values(":digest", s(digest.to_wire()))
            .expression_attribute_values(":committed", s("committed"))
            .expression_attribute_values(":s3", s("s3")),
    )?;
    Ok(())
}

fn put_grant(
    tables: &RegionalTables,
    binding: AuthorityBinding,
    grant: &aex_workspace_domain::DownloadGrant,
    output: &mut TransactionPlan,
) -> Result<(), StoreError> {
    if grant.subject.workspace != binding.workspace || grant.subject.session.is_some() {
        return Err(StoreError::Invalid {
            detail: "a workspace download grant crosses its authenticated binding".to_owned(),
        });
    }
    let stored = crate::codec::DownloadGrant {
        // The v1 grant is identified by an opaque UUID and is never redeemed
        // through this table. The existing key column predates that decision;
        // its value remains an opaque, non-bearer identity.
        token_sha256: grant.id.0.to_string(),
        workspace: binding.workspace,
        digest: grant.whole_sha256,
        range_start: grant.range.start,
        range_end_exclusive: grant.range.end_exclusive,
        authorized_bytes: grant.authorized_bytes,
        measurement: grant.measurement,
        media_type: grant
            .media_type
            .as_ref()
            .map_or_else(String::new, |value| value.as_str().to_owned()),
        expires_at: grant.expires_at,
    };
    let item = crate::codec::encode_grant(&stored).map_err(|error| StoreError::Invalid {
        detail: format!("a download grant could not be encoded: {error}"),
    })?;
    output.put(
        Participant::CONTENT_GRANT,
        Put::builder()
            .table_name(&tables.regional_content)
            .set_item(Some(item))
            .condition_expression(IMMUTABLE),
    )?;
    Ok(())
}

fn put_grant_pin(
    tables: &RegionalTables,
    binding: AuthorityBinding,
    pin: &Pin,
    output: &mut TransactionPlan,
) -> Result<(), StoreError> {
    let Pin::Grant {
        grant,
        digest,
        expires_at,
    } = pin
    else {
        return Err(StoreError::Invalid {
            detail: "a download grant must carry a grant pin".to_owned(),
        });
    };
    let identity = grant.0.to_string();
    let key = crate::keys::grant_pin(binding.workspace, digest, &identity)?;
    let lifetime_millis = i64::try_from(aex_workspace_domain::GRANT_TTL.whole_milliseconds())
        .map_err(|_| StoreError::Invalid {
            detail: "the domain grant lifetime does not fit the timestamp representation"
                .to_owned(),
        })?;
    let created_at_millis = expires_at
        .unix_millis()
        .checked_sub(lifetime_millis)
        .ok_or_else(|| StoreError::Invalid {
            detail: "a grant pin creation time underflows the timestamp representation".to_owned(),
        })?;
    let item = keyed(
        crate::codec::grant_pin_attributes(
            binding.workspace,
            &identity,
            // A grant pin is born with the same request that writes the grant.
            // The public plan carries no second clock, so the grant's expiry
            // minus the domain-owned grant lifetime is the creation time.
            aex_wire::types::Timestamp::from_unix_millis(created_at_millis).map_err(|error| {
                StoreError::Invalid {
                    detail: format!("a grant pin creation time is invalid: {error}"),
                }
            })?,
            *expires_at,
        ),
        &key.pk,
        &key.sk,
    );
    output.put(
        Participant::CONTENT_GRANT_PIN,
        Put::builder()
            .table_name(&tables.regional_content)
            .set_item(Some(item))
            .condition_expression(IMMUTABLE),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use aex_content_domain::{ContentDigest, GrantId, Pin};
    use aex_session_app::plan::{
        Condition, SessionTransaction, TableFamily, TransactionIntent, Write,
    };
    use aex_session_domain::{
        IdempotencyIdentity, IdempotencyReceipt, ReceiptKey, ReceiptOutcome, ResourceId,
        ResourceKind, ResponseBody,
    };
    use aex_session_dynamodb::application_plan::{
        AuthorityBinding, ExternalActionCompiler, FamilyCompilers, IdempotencyCompiler,
        LogicalAction, WorkspaceBinding, compile_workspace_transaction,
    };
    use aex_session_dynamodb::error::StoreError;
    use aex_session_dynamodb::plan::{
        Participant, RegionalTables, TransactionPlan, key as physical_key,
    };
    use aex_wire::idempotency::{IdempotencyKey, IntentDigest, PrincipalScope, ReplayIdentity};
    use aex_wire::ids::{
        ApiKeyId, MeasurementId, OrganizationId, ResourceName, Uuid7, WorkspaceId,
    };
    use aex_wire::routes::RouteId;
    use aex_wire::types::{ETag, Timestamp};
    use aex_workspace_domain::{
        ByteRange, DownloadGrant, GrantPlacement, GrantSubject, ObjectChecksum, RegistrySelector,
    };
    use aws_sdk_dynamodb::types::ConditionCheck;

    use super::ContentAuthorityCompiler;

    fn id<T: aex_wire::ids::PrefixedId>(byte: u8) -> T {
        T::from_uuid7(Uuid7::compose(1_754_051_696_789, [byte; 10]))
    }

    fn moment(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("a timestamp")
    }

    struct RegistryGuardCompiler;

    impl ExternalActionCompiler for RegistryGuardCompiler {
        fn compile_action(
            &self,
            tables: &RegionalTables,
            _binding: AuthorityBinding,
            action: &LogicalAction<'_>,
            output: &mut TransactionPlan,
        ) -> Result<(), StoreError> {
            if action.write.is_some()
                || !matches!(
                    action.conditions.as_slice(),
                    [(_, Condition::RegistryEtag { .. })]
                )
            {
                return Err(StoreError::Invalid {
                    detail: "the test registry compiler accepts one pointer guard".to_owned(),
                });
            }
            output.condition_check(
                Participant::REGISTRY_POINTER,
                ConditionCheck::builder()
                    .table_name(&tables.regional_registry)
                    .set_key(Some(physical_key(
                        &format!("REG#{}", action.target.partition),
                        &action.target.sort,
                    )))
                    .condition_expression("attribute_exists(pk)"),
            )?;
            Ok(())
        }
    }

    fn plan(workspace: WorkspaceId, organization: OrganizationId) -> SessionTransaction {
        let grant_id = GrantId(Uuid7::compose(1_754_051_696_789, [4; 10]));
        let digest = ContentDigest::of(b"registry file");
        let expires_at = moment(
            i64::try_from(aex_workspace_domain::GRANT_TTL.whole_milliseconds())
                .expect("the grant lifetime fits"),
        );
        let grant = DownloadGrant {
            id: grant_id,
            subject: GrantSubject {
                workspace,
                session: None,
            },
            range: ByteRange {
                start: 2,
                end_exclusive: 8,
            },
            authorized_bytes: 6,
            whole_sha256: digest,
            media_type: None,
            placement: GrantPlacement::ObjectRange {
                key: aex_content_domain::ContentObjectKey::parse("wsp/body").expect("a key"),
                checksum: ObjectChecksum::Crc64Nvme("AAAAAAAAAAA=".to_owned()),
            },
            measurement: id::<MeasurementId>(5),
            expires_at,
        };
        let identity = IdempotencyIdentity::Key(Box::new(ReplayIdentity {
            principal: PrincipalScope::WorkspaceKey {
                key: id::<ApiKeyId>(6),
                workspace,
                organization,
            },
            route: RouteId::RegistryFilesDownloadCreate,
            key: IdempotencyKey::parse("download-1").expect("an idempotency key"),
            intent: IntentDigest::from_bytes([7; 32]),
        }));
        let receipt = IdempotencyReceipt {
            key: ReceiptKey::of("registry.download:file", &identity).expect("a receipt key"),
            intent: identity.intent(),
            identity,
            outcome: ReceiptOutcome::Resource {
                kind: ResourceKind::Grant,
                id: ResourceId(grant_id.0.to_string()),
                response: ResponseBody::of(br#"{"url":"https://signed.example/object"}"#),
            },
            created_at: moment(0),
            expires_at: Some(expires_at),
        };
        SessionTransaction {
            intent: TransactionIntent::RegistryDownload,
            conditions: vec![
                Condition::RegistryEtag {
                    selector: RegistrySelector {
                        workspace,
                        kind: aex_content_domain::RegistryKind::File,
                        name: ResourceName::parse("guide.pdf").expect("a name"),
                    },
                    expected: ETag::parse("0123456789abcdef0123456789abcdef").expect("an ETag"),
                },
                Condition::ContentOwned { workspace, digest },
            ],
            writes: vec![
                Write::PutGrant(Box::new(grant)),
                Write::PutPin(Box::new(Pin::Grant {
                    grant: grant_id,
                    digest,
                    expires_at,
                })),
                Write::PutIdempotencyReceipt(Box::new(receipt)),
            ],
            after_commit: Vec::new(),
        }
    }

    #[test]
    fn one_workspace_transaction_contains_grant_pin_and_registry_receipt() {
        let workspace = id::<WorkspaceId>(1);
        let organization = id::<OrganizationId>(2);
        let content = ContentAuthorityCompiler;
        let registry = RegistryGuardCompiler;
        let registry_receipts = IdempotencyCompiler::registry();
        let compilers = FamilyCompilers::new()
            .with(TableFamily::ContentAuthority, &content)
            .with(TableFamily::Registry, &registry)
            .with(TableFamily::Idempotency, &registry_receipts);
        let tables = RegionalTables::composed("dev", "eu-west-1");
        let compiled = compile_workspace_transaction(
            &tables,
            &plan(workspace, organization),
            WorkspaceBinding {
                workspace,
                organization,
            },
            &compilers,
        )
        .expect("the three-family plan compiles");

        assert_eq!(compiled.transaction.len(), 5);
        assert_eq!(
            compiled.transaction.participants(),
            [
                Participant::REGISTRY_POINTER,
                Participant::CONTENT_DESCRIPTOR,
                Participant::CONTENT_GRANT,
                Participant::CONTENT_GRANT_PIN,
                Participant::REGISTRY_IDEMPOTENCY,
            ]
        );
        let addressed = compiled
            .transaction
            .actions()
            .iter()
            .filter_map(|action| action.put().map(aws_sdk_dynamodb::types::Put::table_name))
            .collect::<Vec<_>>();
        assert_eq!(
            addressed
                .iter()
                .filter(|table| **table == tables.regional_content)
                .count(),
            2
        );
        assert_eq!(
            addressed
                .iter()
                .filter(|table| **table == tables.regional_registry)
                .count(),
            1
        );
        let receipt_put = compiled
            .transaction
            .participants()
            .iter()
            .position(|participant| *participant == Participant::REGISTRY_IDEMPOTENCY)
            .and_then(|index| compiled.transaction.actions().get(index))
            .and_then(|action| action.put())
            .expect("the receipt is one conditional put");
        assert!(
            receipt_put
                .condition_expression()
                .is_some_and(|expression| expression.contains("#receiptExpiresAt <= :receiptNow")),
            "an expired bearer receipt must be replaceable before asynchronous TTL cleanup"
        );
        let pin_put = compiled
            .transaction
            .participants()
            .iter()
            .position(|participant| *participant == Participant::CONTENT_GRANT_PIN)
            .and_then(|index| compiled.transaction.actions().get(index))
            .and_then(|action| action.put())
            .expect("the grant pin is one conditional put");
        let created_at = moment(0).to_wire();
        assert_eq!(
            pin_put
                .item()
                .get("createdAt")
                .and_then(|value| value.as_s().ok())
                .map(String::as_str),
            Some(created_at.as_str()),
            "the persisted pin lifetime must derive from the domain grant lifetime"
        );
    }

    #[test]
    fn a_workspace_mismatch_is_rejected_before_request_construction() {
        let workspace = id::<WorkspaceId>(1);
        let organization = id::<OrganizationId>(2);
        let content = ContentAuthorityCompiler;
        let registry = RegistryGuardCompiler;
        let registry_receipts = IdempotencyCompiler::registry();
        let compilers = FamilyCompilers::new()
            .with(TableFamily::ContentAuthority, &content)
            .with(TableFamily::Registry, &registry)
            .with(TableFamily::Idempotency, &registry_receipts);
        let error = compile_workspace_transaction(
            &RegionalTables::composed("dev", "eu-west-1"),
            &plan(workspace, organization),
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

    #[test]
    fn content_ownership_is_a_read_only_committed_object_guard() {
        let workspace = id::<WorkspaceId>(1);
        let organization = id::<OrganizationId>(2);
        let digest = ContentDigest::of(b"registry file");
        let plan = SessionTransaction {
            intent: TransactionIntent::CommitTerminal,
            conditions: vec![Condition::ContentOwned { workspace, digest }],
            writes: Vec::new(),
            after_commit: Vec::new(),
        };
        let content = ContentAuthorityCompiler;
        let compilers = FamilyCompilers::new().with(TableFamily::ContentAuthority, &content);
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
        .expect("the content guard compiles");

        assert_eq!(compiled.transaction.len(), 1);
        assert_eq!(
            compiled.transaction.participants(),
            [Participant::CONTENT_DESCRIPTOR]
        );
        let guard = compiled.transaction.actions()[0]
            .condition_check()
            .expect("a read-only check");
        assert_eq!(guard.table_name(), tables.regional_content);
        assert!(
            guard
                .condition_expression()
                .contains("attribute_exists(objectKey)")
        );
    }

    #[test]
    fn a_download_plan_cannot_omit_its_pin() {
        let workspace = id::<WorkspaceId>(1);
        let organization = id::<OrganizationId>(2);
        let mut partial = plan(workspace, organization);
        partial
            .writes
            .retain(|write| !matches!(write, Write::PutPin(_)));
        assert!(matches!(
            partial.validate(),
            Err(aex_session_app::plan::PlanError::RegistryDownloadAuthority { .. })
        ));
    }

    #[test]
    fn a_download_plan_cannot_omit_or_drift_its_first_write_authority_guards() {
        let workspace = id::<WorkspaceId>(1);
        let organization = id::<OrganizationId>(2);
        let mut missing = plan(workspace, organization);
        missing.conditions.clear();
        assert!(matches!(
            missing.validate(),
            Err(aex_session_app::plan::PlanError::RegistryDownloadAuthority { .. })
        ));

        let mut drifted = plan(workspace, organization);
        let Condition::ContentOwned { digest, .. } = &mut drifted.conditions[1] else {
            panic!("the fixture carries its content guard second");
        };
        *digest = ContentDigest::of(b"another body");
        assert!(matches!(
            drifted.validate(),
            Err(aex_session_app::plan::PlanError::RegistryDownloadAuthority { .. })
        ));
    }

    #[test]
    fn a_download_receipt_must_name_and_expire_with_the_grant() {
        let workspace = id::<WorkspaceId>(1);
        let organization = id::<OrganizationId>(2);
        let mut drifted = plan(workspace, organization);
        let receipt = drifted
            .writes
            .iter_mut()
            .find_map(|write| match write {
                Write::PutIdempotencyReceipt(receipt) => Some(receipt),
                _ => None,
            })
            .expect("the fixture carries a receipt");
        receipt.expires_at = None;
        assert!(matches!(
            drifted.validate(),
            Err(aex_session_app::plan::PlanError::RegistryDownloadAuthority { .. })
        ));
    }
}
