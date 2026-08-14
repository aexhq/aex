//! First-login personal-account provisioning at the application boundary.

use std::sync::Mutex;

use async_trait::async_trait;
use time::OffsetDateTime;
use uuid::{Builder, Uuid};

use aex_control_app::ControlError;
use aex_control_app::personal_account::{
    CEREMONY, PersonalAccountProvision, PersonalAccountProvisionCommand,
    PersonalAccountProvisioner, ProvisionPersonalAccount,
};
use aex_control_app::ports::{IdFactory, ReconcileIdentity, StoreError, TxOutcome, UnknownCommit};

const USER: Uuid = Uuid::from_u128(0x0192_3f2a_1c00_7000_8000_0000_0000_0001);

struct OrderedIds(Mutex<u64>);

impl OrderedIds {
    fn new() -> Self {
        Self(Mutex::new(0))
    }
}

impl IdFactory for OrderedIds {
    fn next(&self) -> Uuid {
        let mut issued = self.0.lock().expect("id sequence");
        let id =
            Builder::from_unix_timestamp_millis(1_700_000_000_000 + *issued, &[0; 10]).into_uuid();
        *issued += 1;
        id
    }
}

#[derive(Default)]
struct RecordingProvisioner {
    commands: Mutex<Vec<PersonalAccountProvisionCommand>>,
    replay: bool,
}

#[async_trait]
impl PersonalAccountProvisioner for RecordingProvisioner {
    async fn provision_personal_account(
        &self,
        command: &PersonalAccountProvisionCommand,
    ) -> Result<TxOutcome<PersonalAccountProvision>, StoreError> {
        self.commands
            .lock()
            .expect("commands")
            .push(command.clone());
        let result = PersonalAccountProvision {
            account_id: command.ids.account_id,
            user_id: command.user_id,
            membership_id: command.ids.membership_id,
            workspace_id: command.ids.workspace_id,
            available_account_id: command.ids.available_account_id,
            reserved_account_id: command.ids.reserved_account_id,
            created_at: command.now,
        };
        Ok(if self.replay {
            TxOutcome::Replayed(result)
        } else {
            TxOutcome::Committed(result)
        })
    }
}

#[tokio::test]
async fn first_login_mints_distinct_uuid7_authority_identities() {
    let ids = OrderedIds::new();
    let store = RecordingProvisioner::default();
    let now = OffsetDateTime::UNIX_EPOCH;

    let provisioned = ProvisionPersonalAccount::run(&store, &ids, USER, now)
        .await
        .expect("personal account provisions");
    let command = store.commands.lock().expect("commands")[0].clone();

    let minted = command.ids.all();
    assert_eq!(minted.len(), 9);
    assert!(minted.iter().all(|id| id.get_version_num() == 7));
    let mut unique = minted;
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), 9, "every authority owns a distinct identity");
    assert_eq!(provisioned.account_id, command.ids.account_id);
    assert_eq!(provisioned.workspace_id, command.ids.workspace_id);
    assert_eq!(command.user_id, USER);
}

#[tokio::test]
async fn an_existing_users_personal_account_replays_without_a_second_identity() {
    let ids = OrderedIds::new();
    let store = RecordingProvisioner {
        commands: Mutex::new(Vec::new()),
        replay: true,
    };

    let provisioned = ProvisionPersonalAccount::run(&store, &ids, USER, OffsetDateTime::UNIX_EPOCH)
        .await
        .expect("existing account is a successful replay");

    assert_eq!(provisioned.user_id, USER);
    assert_eq!(store.commands.lock().expect("commands").len(), 1);
}

struct UnknownProvisioner;

#[async_trait]
impl PersonalAccountProvisioner for UnknownProvisioner {
    async fn provision_personal_account(
        &self,
        command: &PersonalAccountProvisionCommand,
    ) -> Result<TxOutcome<PersonalAccountProvision>, StoreError> {
        Ok(TxOutcome::Unknown(UnknownCommit {
            identity: ReconcileIdentity {
                ceremony: CEREMONY,
                id: command.user_id,
            },
        }))
    }
}

#[tokio::test]
async fn an_unknown_commit_reconciles_by_user_not_a_fresh_candidate() {
    let error = ProvisionPersonalAccount::run(
        &UnknownProvisioner,
        &OrderedIds::new(),
        USER,
        OffsetDateTime::UNIX_EPOCH,
    )
    .await
    .expect_err("an unknown commit is never reported as success");

    assert_eq!(
        error,
        ControlError::CommitOutcomeUnknown {
            ceremony: CEREMONY,
            id: USER,
        }
    );
}
