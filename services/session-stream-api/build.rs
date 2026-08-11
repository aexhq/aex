//! Generates the build-bound model-catalog authority.

#[path = "../../crates/aex-model-catalog/build_binding.rs"]
mod model_catalog_build_binding;

fn main() {
    model_catalog_build_binding::generate("session-stream-api");
}
