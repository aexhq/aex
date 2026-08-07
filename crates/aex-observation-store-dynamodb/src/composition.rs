//! The capability grants each observation deployable may and may not hold.
//!
//! A deployable never mounts a route it cannot fully serve (RS-18): there is no
//! `route_binding_unavailable` and no unavailable-port stub anywhere in this
//! stream, which is the single largest behavioural difference from the
//! implementation being replaced.
//!
//! Each role's grant is declared here and asserted at startup, so a launcher bug
//! cannot become a data path and a read role cannot become a writer.

/// One capability a role may hold over the observation authority.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Capability {
    /// `dynamodb:Query`/`GetItem`/`BatchGetItem` on the table and its indexes.
    ReadAuthority,
    /// `dynamodb:TransactWriteItems`/`PutItem`/`UpdateItem` on admission rows.
    WriteAdmission,
    /// `dynamodb:PutItem`/`UpdateItem` on export-control rows only.
    WriteExportControl,
    /// `dynamodb:BatchWriteItem` on `OBS#` for deletion.
    DeleteObservations,
    /// `s3:GetObject` on `observations/*`.
    ReadBodies,
    /// `s3:PutObject` on `observations/*`.
    WriteBodies,
    /// `s3:DeleteObject` on `observations/*`.
    DeleteBodies,
    /// `s3:PutObject`/`AbortMultipartUpload`/`ListMultipartUploadParts` on
    /// `exports/*`.
    WriteExportObjects,
    /// `ecs:RunTask`/`ListTasks`/`DescribeTasks` and `iam:PassRole`.
    LaunchExportTasks,
    /// `sqs:SendMessage` to the usage queue.
    DeliverUsage,
}

impl Capability {
    /// Every capability, in declared order.
    pub const ALL: &'static [Capability] = &[
        Capability::ReadAuthority,
        Capability::WriteAdmission,
        Capability::WriteExportControl,
        Capability::DeleteObservations,
        Capability::ReadBodies,
        Capability::WriteBodies,
        Capability::DeleteBodies,
        Capability::WriteExportObjects,
        Capability::LaunchExportTasks,
        Capability::DeliverUsage,
    ];

    /// The name used in a startup assertion failure.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReadAuthority => "read_authority",
            Self::WriteAdmission => "write_admission",
            Self::WriteExportControl => "write_export_control",
            Self::DeleteObservations => "delete_observations",
            Self::ReadBodies => "read_bodies",
            Self::WriteBodies => "write_bodies",
            Self::DeleteBodies => "delete_bodies",
            Self::WriteExportObjects => "write_export_objects",
            Self::LaunchExportTasks => "launch_export_tasks",
            Self::DeliverUsage => "deliver_usage",
        }
    }
}

/// Each deployable in the observation stream.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Role {
    /// The public observation query and export-control API.
    ObservationApi,
    /// The customer OTLP admission edge.
    Otlp,
    /// The scheduled reconciler, in a duty deployment that does not delete.
    Reconciler,
    /// The reconciler's `deletion.execute` deployment, the only one that may
    /// delete an object.
    ReconcilerDeletion,
    /// The export launcher.
    ExportLauncher,
    /// The one-shot export task.
    ExportTask,
    /// `regional-stream`, which reads this table and holds no write at all.
    Stream,
}

impl Role {
    /// Every role, in declared order.
    pub const ALL: &'static [Role] = &[
        Role::ObservationApi,
        Role::Otlp,
        Role::Reconciler,
        Role::ReconcilerDeletion,
        Role::ExportLauncher,
        Role::ExportTask,
        Role::Stream,
    ];

    /// The capabilities this role holds.
    #[must_use]
    pub const fn granted(self) -> &'static [Capability] {
        match self {
            Self::ObservationApi => &[
                Capability::ReadAuthority,
                Capability::WriteExportControl,
                Capability::ReadBodies,
            ],
            Self::Otlp => &[
                Capability::WriteAdmission,
                Capability::WriteBodies,
                Capability::DeliverUsage,
            ],
            Self::Reconciler => &[
                Capability::ReadAuthority,
                Capability::WriteAdmission,
                Capability::ReadBodies,
                Capability::DeliverUsage,
            ],
            Self::ReconcilerDeletion => &[
                Capability::ReadAuthority,
                Capability::WriteAdmission,
                Capability::DeleteObservations,
                Capability::ReadBodies,
                Capability::DeleteBodies,
            ],
            // The launcher holds **no** observation read permission at all, so a
            // launcher bug cannot become a data path.
            Self::ExportLauncher => &[Capability::LaunchExportTasks],
            Self::ExportTask => &[
                Capability::ReadAuthority,
                Capability::ReadBodies,
                Capability::WriteExportObjects,
            ],
            Self::Stream => &[Capability::ReadAuthority],
        }
    }

    /// Whether the role holds one capability.
    #[must_use]
    pub fn holds(self, capability: Capability) -> bool {
        self.granted().contains(&capability)
    }

    /// The capabilities this role's startup assertion must **deny**.
    #[must_use]
    pub fn denied(self) -> Vec<Capability> {
        Capability::ALL
            .iter()
            .copied()
            .filter(|capability| !self.holds(*capability))
            .collect()
    }
}

/// Why a composition was refused at startup.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("`{role:?}` must not hold `{}`", capability.as_str())]
pub struct CapabilityViolation {
    /// The offending role.
    pub role: Role,
    /// The capability it must not hold.
    pub capability: Capability,
}

/// Asserts an observed grant matches the role's declared one.
///
/// # Errors
///
/// Returns [`CapabilityViolation`] naming the first capability the role holds
/// but must not. A deployable that fails this refuses to start.
pub fn assert_grant(role: Role, observed: &[Capability]) -> Result<(), CapabilityViolation> {
    for capability in observed {
        if !role.holds(*capability) {
            return Err(CapabilityViolation {
                role,
                capability: *capability,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Capability, Role, assert_grant};

    #[test]
    fn the_query_api_writes_only_export_control_rows() {
        for forbidden in [
            Capability::WriteAdmission,
            Capability::WriteBodies,
            Capability::DeleteObservations,
            Capability::DeleteBodies,
        ] {
            assert!(!Role::ObservationApi.holds(forbidden), "{forbidden:?}");
            assert!(assert_grant(Role::ObservationApi, &[forbidden]).is_err());
        }
        assert!(Role::ObservationApi.holds(Capability::WriteExportControl));
    }

    #[test]
    fn otlp_holds_no_read_on_observation_bodies_and_no_export_permission() {
        assert!(!Role::Otlp.holds(Capability::ReadBodies));
        assert!(!Role::Otlp.holds(Capability::ReadAuthority));
        assert!(!Role::Otlp.holds(Capability::WriteExportObjects));
        assert!(Role::Otlp.holds(Capability::WriteAdmission));
    }

    #[test]
    fn the_launcher_holds_no_observation_read_grant_at_all() {
        assert_eq!(
            Role::ExportLauncher.granted(),
            &[Capability::LaunchExportTasks]
        );
        assert!(!Role::ExportLauncher.holds(Capability::ReadAuthority));
        assert!(!Role::ExportLauncher.holds(Capability::ReadBodies));
    }

    #[test]
    fn only_the_deletion_deployment_may_delete_an_object() {
        for role in Role::ALL {
            let expected = *role == Role::ReconcilerDeletion;
            assert_eq!(role.holds(Capability::DeleteBodies), expected, "{role:?}");
        }
    }

    #[test]
    fn the_export_task_holds_no_delete_anywhere() {
        assert!(!Role::ExportTask.holds(Capability::DeleteBodies));
        assert!(!Role::ExportTask.holds(Capability::DeleteObservations));
        assert!(Role::ExportTask.holds(Capability::WriteExportObjects));
    }

    #[test]
    fn the_stream_reader_holds_no_write_permission_of_any_kind() {
        assert_eq!(Role::Stream.granted(), &[Capability::ReadAuthority]);
        for capability in Role::Stream.denied() {
            assert!(
                !matches!(capability, Capability::ReadAuthority),
                "the read grant must not be denied"
            );
        }
        assert_eq!(Role::Stream.denied().len(), Capability::ALL.len() - 1);
    }

    #[test]
    fn a_role_that_holds_only_its_declared_grant_starts() {
        for role in Role::ALL {
            assert_grant(*role, role.granted()).expect("its own grant is admitted");
        }
        assert_eq!(Capability::ReadAuthority.as_str(), "read_authority");
    }
}
