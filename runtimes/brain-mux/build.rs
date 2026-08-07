//! Generates the build-bound publisher trust-root set and signed catalog collection.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[path = "task_shape_policy.rs"]
mod task_shape_policy;

const TRUST_ROOTS_JSON_VAR: &str = "AEX_MODEL_CATALOG_TRUST_ROOTS_JSON";
const TRUST_ROOTS_SHA_VAR: &str = "AEX_MODEL_CATALOG_TRUST_ROOTS_SHA256";
const COLLECTION_VAR: &str = "AEX_MODEL_CATALOG_COLLECTION_FILE";
const COLLECTION_SHA_VAR: &str = "AEX_MODEL_CATALOG_COLLECTION_SHA256";
const TRUST_ROOTS_SCHEMA: &str = "aex.model-catalog-trust-roots.v1";
const MAX_TRUST_ROOTS: usize = 8;
const MAX_TRUST_ROOTS_BYTES: usize = 8 * 1024;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TrustRoots {
    // Field order is JCS order for this ASCII-only document.
    keys: Vec<TrustRoot>,
    schema: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TrustRoot {
    // Field order is JCS order.
    key_id: String,
    sec1: String,
}

fn main() {
    for name in [
        TRUST_ROOTS_JSON_VAR,
        TRUST_ROOTS_SHA_VAR,
        COLLECTION_VAR,
        COLLECTION_SHA_VAR,
    ] {
        println!("cargo:rerun-if-env-changed={name}");
    }

    let trust_roots_json = nonempty(TRUST_ROOTS_JSON_VAR);
    let trust_roots_sha = nonempty(TRUST_ROOTS_SHA_VAR);
    let collection = nonempty(COLLECTION_VAR);
    let collection_sha = nonempty(COLLECTION_SHA_VAR);
    let configured = [
        trust_roots_json.is_some(),
        trust_roots_sha.is_some(),
        collection.is_some(),
        collection_sha.is_some(),
    ];
    assert!(
        !configured.iter().any(|present| *present) || configured.iter().all(|present| *present),
        "brain-mux catalog release inputs are partial; {TRUST_ROOTS_JSON_VAR}, \
         {TRUST_ROOTS_SHA_VAR}, {COLLECTION_VAR} and {COLLECTION_SHA_VAR} must be supplied \
         together at build time"
    );

    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo supplies OUT_DIR"));
    generate_task_shape(&out);
    let generated = out.join("model_catalog_release.rs");
    let source = generate_release_source(
        &out,
        (
            trust_roots_json,
            trust_roots_sha,
            collection,
            collection_sha,
        ),
    );
    fs::write(generated, source).expect("write model catalog release binding");
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

fn generate_release_source(
    out: &std::path::Path,
    inputs: (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ),
) -> String {
    match inputs {
        (Some(trust_roots_json), Some(trust_roots_sha), Some(collection), Some(collection_sha)) => {
            let roots = validate_trust_roots(&trust_roots_json, &trust_roots_sha);
            validate_workspace_relative_path(&collection);
            let manifest = PathBuf::from(
                std::env::var_os("CARGO_MANIFEST_DIR").expect("Cargo supplies CARGO_MANIFEST_DIR"),
            );
            let workspace = manifest
                .parent()
                .and_then(std::path::Path::parent)
                .expect("brain-mux lives two directories below the workspace root");
            let canonical_workspace = workspace
                .canonicalize()
                .expect("workspace root is canonicalizable");
            let logical_collection = workspace.join(&collection);
            let collection_path = logical_collection.canonicalize().unwrap_or_else(|error| {
                panic!(
                    "cannot resolve build-bound model catalog collection {}: {error}",
                    logical_collection.display()
                )
            });
            assert!(
                collection_path.starts_with(&canonical_workspace),
                "build-bound model catalog collection resolves outside the workspace root"
            );
            println!("cargo:rerun-if-changed={}", collection_path.display());
            let bytes = fs::read(&collection_path).unwrap_or_else(|error| {
                panic!(
                    "cannot read build-bound model catalog collection {}: {error}",
                    collection_path.display()
                )
            });
            assert!(
                !bytes.is_empty(),
                "the build-bound model catalog collection is empty"
            );
            let actual_sha = format!("sha256:{}", hex::encode(Sha256::digest(&bytes)));
            assert_eq!(
                collection_sha, actual_sha,
                "the build-bound model catalog collection does not match its release-plan digest"
            );
            let copied = out.join("model-catalog-collection.json");
            fs::write(&copied, bytes).expect("write build-bound catalog collection");
            let entries = roots
                .keys
                .iter()
                .map(|root| {
                    let public = decode_public_key(&root.sec1);
                    format!("(\"{}\", {public:?})", root.key_id)
                })
                .collect::<Vec<_>>()
                .join(",");
            format!(
                "static RELEASE_TRUSTED_KEYS: &[aex_model_catalog::signature::TrustedKey] = \
                 &[{entries}];\n\
                 static RELEASE_CATALOG_COLLECTION: &[u8] = \
                 include_bytes!(\"model-catalog-collection.json\");\n\
                 const RELEASE_INPUT_BLOCKER: Option<&str> = None;\n"
            )
        }
        (None, None, None, None) => String::from(
            "static RELEASE_TRUSTED_KEYS: &[aex_model_catalog::signature::TrustedKey] = &[];\n\
             static RELEASE_CATALOG_COLLECTION: &[u8] = &[];\n\
             const RELEASE_INPUT_BLOCKER: Option<&str> = Some(\
             \"this build has no real publisher P-256 trust-root set or signed model-catalog \
             collection; the release builder must bind both together\");\n",
        ),
        _ => unreachable!("partial inputs were rejected above"),
    }
}

fn nonempty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn validate_key_id(key_id: &str) {
    assert!(
        !key_id.is_empty() && key_id.len() <= 64,
        "catalog publisher key id exceeds 64 bytes"
    );
    assert!(
        key_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')),
        "catalog publisher key id must be ASCII alphanumeric, '-' or '_'"
    );
}

fn decode_public_key(hex: &str) -> [u8; 65] {
    assert!(
        hex.len() == 130
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "catalog publisher SEC1 key must be 130 lowercase hexadecimal characters"
    );
    let mut public = [0_u8; 65];
    for (index, byte) in public.iter_mut().enumerate() {
        let offset = index * 2;
        *byte = u8::from_str_radix(&hex[offset..offset + 2], 16)
            .expect("validated hexadecimal publisher key");
    }
    assert_eq!(
        public[0], 0x04,
        "catalog publisher key must be uncompressed SEC1"
    );
    p256::ecdsa::VerifyingKey::from_sec1_bytes(&public)
        .expect("catalog publisher SEC1 key must be a point on P-256");
    public
}

fn validate_trust_roots(json: &str, expected_sha: &str) -> TrustRoots {
    assert!(
        json.len() <= MAX_TRUST_ROOTS_BYTES,
        "catalog publisher trust roots exceed the {MAX_TRUST_ROOTS_BYTES}-byte bound"
    );
    let actual_sha = format!("sha256:{}", hex::encode(Sha256::digest(json.as_bytes())));
    assert_eq!(
        expected_sha, actual_sha,
        "catalog publisher trust roots do not match their release-plan digest"
    );
    let roots: TrustRoots = serde_json::from_str(json)
        .expect("catalog publisher trust roots must be closed-schema JSON");
    assert_eq!(
        roots.schema, TRUST_ROOTS_SCHEMA,
        "catalog publisher trust-root schema is unsupported"
    );
    assert!(
        !roots.keys.is_empty() && roots.keys.len() <= MAX_TRUST_ROOTS,
        "catalog publisher trust roots require 1..={MAX_TRUST_ROOTS} keys"
    );
    for root in &roots.keys {
        validate_key_id(&root.key_id);
        let _ = decode_public_key(&root.sec1);
    }
    assert!(
        roots
            .keys
            .windows(2)
            .all(|pair| pair[0].key_id < pair[1].key_id),
        "catalog publisher trust-root key ids must be strictly sorted and unique"
    );
    let canonical = serde_json::to_string(&roots)
        .expect("catalog publisher trust roots are serializable canonical JSON");
    assert_eq!(
        canonical, json,
        "catalog publisher trust roots must be exact canonical JSON with no trailing bytes"
    );
    roots
}

fn validate_workspace_relative_path(path: &str) {
    let valid = !path.is_empty()
        && !path.contains('\\')
        && path.split('/').all(|component| {
            !component.is_empty()
                && !matches!(component, "." | "..")
                && component
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        });
    assert!(
        valid,
        "catalog collection path must be a normalized workspace-relative forward-slash path"
    );
}
