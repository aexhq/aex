//! Derives the immutable digest of every compiled provider-adapter source module.

use std::fs;
use std::path::{Path, PathBuf};

const DOMAIN: &[u8] = b"aex-provider-adapter-source/v1\0";
const REQUIRED_ADAPTERS: [&str; 8] = [
    "src/anthropic.rs",
    "src/deepseek.rs",
    "src/google.rs",
    "src/moonshotai.rs",
    "src/openai.rs",
    "src/openrouter.rs",
    "src/vercel_ai_gateway.rs",
    "src/zai.rs",
];

fn main() {
    let manifest = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("Cargo supplies CARGO_MANIFEST_DIR"),
    );
    let source = manifest.join("src");
    println!("cargo:rerun-if-changed={}", source.display());

    let mut files = Vec::new();
    collect_rust_sources(&source, &mut files);
    files.sort();
    assert!(
        !files.is_empty(),
        "the provider adapter source tree is empty"
    );
    for required in REQUIRED_ADAPTERS {
        assert!(
            files.iter().any(|path| {
                path.strip_prefix(&manifest)
                    .is_ok_and(|relative| relative.to_string_lossy().replace('\\', "/") == required)
            }),
            "required provider adapter {required} is outside the build-identity scope"
        );
    }

    let mut digest = blake3::Hasher::new();
    digest.update(DOMAIN);
    for path in files {
        println!("cargo:rerun-if-changed={}", path.display());
        let relative = path
            .strip_prefix(&manifest)
            .expect("source lives below the crate root")
            .to_string_lossy()
            .replace('\\', "/");
        let bytes = fs::read(&path).unwrap_or_else(|error| {
            panic!(
                "cannot read provider adapter source {}: {error}",
                path.display()
            )
        });
        let relative_len = u64::try_from(relative.len()).expect("source path length fits u64");
        digest.update(&relative_len.to_le_bytes());
        digest.update(relative.as_bytes());
        let byte_len = u64::try_from(bytes.len()).expect("source file length fits u64");
        digest.update(&byte_len.to_le_bytes());
        digest.update(&bytes);
    }

    println!(
        "cargo:rustc-env=AEX_PROVIDER_ADAPTER_SOURCE_DIGEST={}",
        digest.finalize().to_hex()
    );
}

fn collect_rust_sources(root: &Path, files: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(root)
        .unwrap_or_else(|error| panic!("cannot walk provider source {}: {error}", root.display()));
    for entry in entries {
        let entry = entry.expect("provider source directory entry is readable");
        let path = entry.path();
        let kind = entry
            .file_type()
            .expect("provider source directory entry type is readable");
        if kind.is_dir() {
            collect_rust_sources(&path, files);
        } else if kind.is_file() && path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}
