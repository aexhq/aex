//! Build-bound task shape from the deployable registry.
//!
//! `release/units.toml` is the release authority. The build script refuses a missing,
//! duplicate, non-service or malformed `brain-mux` row and emits only these two numeric
//! constants, so runtime capacity accounting cannot silently describe a different task.

include!(concat!(env!("OUT_DIR"), "/brain_mux_task_shape.rs"));

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

#[cfg(test)]
mod tests {
    use super::{TASK_CPU_UNITS, TASK_MEMORY_MIB, task_memory_bytes, task_parallelism};

    #[test]
    fn release_registry_shape_is_compiled_into_the_binary() {
        assert_eq!(TASK_CPU_UNITS, 2_048);
        assert_eq!(TASK_MEMORY_MIB, 4_096);
        assert_eq!(task_parallelism(), 2);
        assert_eq!(task_memory_bytes(), 4_096 * 1_024 * 1_024);
    }
}
