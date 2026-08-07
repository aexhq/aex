//! Build-bound task shape from the deployable registry.
//!
//! `release/units.toml` is the release authority. The build script refuses a missing,
//! duplicate, non-service or malformed `brain-mux` row and emits only these numeric
//! constants, so runtime capacity accounting and the listener cannot silently describe a
//! different task.

include!(concat!(env!("OUT_DIR"), "/brain_mux_task_shape.rs"));

#[cfg(test)]
#[path = "../task_shape_policy.rs"]
mod policy;

/// Bytes declared for the complete Fargate task.
#[must_use]
pub const fn task_memory_bytes() -> u64 {
    TASK_MEMORY_BYTES
}

/// Whole vCPUs declared for the complete Fargate task.
#[must_use]
pub const fn task_parallelism() -> usize {
    TASK_PARALLELISM
}

/// The TCP port this task serves on.
///
/// Compiled from the release row rather than configured, because the ALB target group, the
/// task definition and this process all have to name one number and a defaulted-but-
/// configurable port is a value they can disagree about with no symptom until a deploy —
/// which is exactly how the binary came to listen on 9090 while the registry declared 8080
/// and the task never became healthy.
#[must_use]
pub const fn task_port() -> u16 {
    TASK_PORT
}

#[cfg(test)]
mod tests {
    use super::{TASK_CPU_UNITS, TASK_MEMORY_MIB, TASK_PORT, task_memory_bytes, task_parallelism};

    #[test]
    fn release_registry_shape_is_compiled_into_the_binary() {
        assert_eq!(TASK_CPU_UNITS, 2_048);
        assert_eq!(TASK_MEMORY_MIB, 4_096);
        assert_eq!(task_parallelism(), 2);
        assert_eq!(task_memory_bytes(), 4_096 * 1_024 * 1_024);
    }

    /// The number the target group probes. If this stops matching the `brain-mux`
    /// `[unit.fargate]` row, the deployed task never becomes healthy.
    #[test]
    fn the_listener_port_is_the_one_the_release_row_declares() {
        assert_eq!(TASK_PORT, 8_080);
        assert_eq!(super::task_port(), TASK_PORT);
    }
}
