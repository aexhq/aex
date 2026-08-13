//! Generates the build-bound task shape.

use std::fs;
use std::path::PathBuf;

#[path = "task_shape_policy.rs"]
mod task_shape_policy;

fn main() {
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo supplies OUT_DIR"));
    generate_task_shape(&out);
}

fn generate_task_shape(out: &std::path::Path) {
    let manifest = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("Cargo supplies CARGO_MANIFEST_DIR"),
    );
    let workspace = manifest
        .parent()
        .and_then(std::path::Path::parent)
        .expect("brain-mux lives two directories below the workspace root");
    let units_path = workspace.join("release/units.toml");
    println!("cargo:rerun-if-changed={}", units_path.display());
    let source = fs::read_to_string(&units_path).unwrap_or_else(|error| {
        panic!(
            "cannot read release registry {}: {error}",
            units_path.display()
        )
    });
    let shape = task_shape_policy::parse_brain_mux_shape(&source).unwrap_or_else(|reason| {
        panic!(
            "release registry {} cannot build brain-mux: {reason}",
            units_path.display()
        )
    });
    let parallelism = shape.cpu / 1_024;
    let generated = format!(
        "/// CPU units declared by the brain-mux Fargate release row.\n\
         pub const TASK_CPU_UNITS: u32 = {};\n\
         /// Memory in MiB declared by the brain-mux Fargate release row.\n\
         pub const TASK_MEMORY_MIB: u32 = {};\n\
         /// The TCP port declared by the brain-mux Fargate release row.\n\
         pub const TASK_PORT: u16 = {};\n\
         const TASK_PARALLELISM: usize = {parallelism};\n\
         const TASK_MEMORY_BYTES: u64 = {}_u64 * 1_024 * 1_024;\n",
        shape.cpu, shape.memory_mb, shape.port, shape.memory_mb,
    );
    fs::write(out.join("brain_mux_task_shape.rs"), generated)
        .expect("write build-bound brain-mux task shape");
}
