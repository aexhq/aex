//! Rewrites `src/generated.rs` from `telemetry/registry.toml`.
//!
//! Run with `cargo run -p aex-telemetry-schema --example regenerate`. The
//! `generated_source_is_byte_identical` test fails whenever the committed file
//! and a fresh render disagree, so this example is the only supported way to
//! change the generated source.

use std::path::PathBuf;

use aex_telemetry_schema::{codegen, registry::Registry};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let registry = Registry::embedded()?;
    let rendered = codegen::render(&registry);
    let target = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("generated.rs");
    std::fs::write(&target, rendered.as_bytes())?;
    println!("wrote {}", target.display());
    Ok(())
}
