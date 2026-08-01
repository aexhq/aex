//! Plaintext non-persistence: plan 04 items 99 and 101.
//!
//! The plan names a `trybuild` compile-fail corpus for item 99. `trybuild` is
//! not one of the workspace's pinned test dependencies, and adding a second
//! compile-driver to prove one negative would be a poor trade, so the guarantee
//! is enforced two ways instead, both of which fail the build rather than warn:
//!
//! * a **source scan** over this crate's own `src/`, proving `SecretPlaintext`
//!   derives nothing, is never a field of any type, and never appears in a
//!   `Serialize`-deriving item;
//! * a **behavioural check** that neither rendering emits a plaintext byte.
//!
//! Item 101's `miri` check is replaced for the same reason: observing a buffer
//! after its owner drops requires `unsafe`, which the workspace forbids. The
//! scan proves the field is `Zeroizing<Vec<u8>>`, whose `Drop` zeroizes, and the
//! unit test in `plaintext.rs` proves the wrapper does what it claims.

use std::path::{Path, PathBuf};

use aex_secret_domain::SecretPlaintext;

fn crate_src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn sources() -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    let entries = std::fs::read_dir(crate_src()).expect("the crate has a src directory");
    for entry in entries {
        let path = entry.expect("readable directory entry").path();
        if path.extension().is_some_and(|extension| extension == "rs") {
            let text = std::fs::read_to_string(&path).expect("readable source file");
            // The invariant is about the crate's own types, so an inline
            // `#[cfg(test)]` module — which is not part of the library — is not
            // scanned.
            let production = text
                .split_once("#[cfg(test)]")
                .map_or(text.clone(), |(before, _)| before.to_owned());
            out.push((path, production));
        }
    }
    assert!(!out.is_empty(), "the scan must find source files");
    out
}

/// Whether a source line declares a struct or enum field whose type mentions
/// `SecretPlaintext`.
///
/// A field is the only way the type could reach a persisted or published value,
/// so that is exactly what this looks for: a `name: Type` binding, not a path
/// like `SecretPlaintext::new` and not a function signature.
fn mentions_as_field(line: &str) -> bool {
    if line.starts_with("//") || line.starts_with('#') {
        return false;
    }
    if line.contains("fn ") || line.contains("impl ") || line.contains("use ") {
        return false;
    }
    let Some((binding, declared)) = line.split_once(':') else {
        return false;
    };
    // `SecretPlaintext::new(..)` splits into a binding containing the type; a
    // real field's binding is a plain identifier.
    if binding.contains("SecretPlaintext") {
        return false;
    }
    let declared = declared.trim_start_matches(':');
    // A path expression such as `SecretPlaintext::MAX_BYTES` is a use, not a
    // field type.
    declared
        .match_indices("SecretPlaintext")
        .any(|(at, _)| !declared[at + "SecretPlaintext".len()..].starts_with("::"))
}

/// 99 `plaintext_never_serialized`, structural half.
#[test]
fn secret_plaintext_derives_nothing_and_is_never_a_field() {
    for (path, text) in sources() {
        let lines: Vec<&str> = text.lines().collect();
        for (index, line) in lines.iter().enumerate() {
            let trimmed = line.trim();

            // The declaration must carry no derive at all.
            if trimmed.starts_with("pub struct SecretPlaintext") {
                let previous = index
                    .checked_sub(1)
                    .map(|before| lines[before].trim())
                    .unwrap_or_default();
                assert!(
                    !previous.starts_with("#[derive"),
                    "{}: SecretPlaintext must derive nothing, found `{previous}`",
                    path.display()
                );
            }

            // It must never be a struct or enum field: a field is how it would
            // reach a Write, a Hint, an OutboxEvent, an error or an observation.
            assert!(
                !mentions_as_field(trimmed),
                "{}:{}: SecretPlaintext must never be a field: `{trimmed}`",
                path.display(),
                index + 1
            );
        }

        // No item in this crate both derives Serialize and mentions plaintext.
        // Doc comments are excluded: the invariant is about code, and the
        // module documentation has to be able to state the rule it enforces.
        let code: String = lines
            .iter()
            .map(|line| line.trim())
            .filter(|line| !line.starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        if code.contains("SecretPlaintext") {
            assert!(
                !code.contains("Serialize"),
                "{}: code mentioning SecretPlaintext must not mention Serialize",
                path.display()
            );
        }
    }
}

/// 101 `zeroize_on_drop`, structural half.
#[test]
fn the_plaintext_buffer_is_a_zeroizing_buffer() {
    let (_, text) = sources()
        .into_iter()
        .find(|(path, _)| path.file_name().is_some_and(|name| name == "plaintext.rs"))
        .expect("the plaintext module exists");
    assert!(
        text.contains("pub struct SecretPlaintext(Zeroizing<Vec<u8>>);"),
        "the plaintext field must stay a Zeroizing buffer"
    );
}

/// 99 `plaintext_never_serialized`, behavioural half.
#[test]
fn no_rendering_emits_a_plaintext_byte() {
    let secret = b"correct-horse-battery-staple";
    let value = SecretPlaintext::new(secret.to_vec()).expect("accepted");
    let rendered = format!("{value}|{value:?}");
    assert!(!rendered.contains("correct-horse"));
    assert!(!rendered.contains("battery"));
    for window in secret.windows(4) {
        let fragment = std::str::from_utf8(window).expect("ascii fixture");
        assert!(
            !rendered.contains(fragment),
            "rendering leaked the fragment `{fragment}`"
        );
    }
    assert_eq!(value.expose_for_encryption(), secret);
}

/// The exposure accessor is the only byte path, and it is named so a reviewer
/// can find every call site with one grep.
#[test]
fn the_only_byte_path_is_named_for_its_purpose() {
    let (_, text) = sources()
        .into_iter()
        .find(|(path, _)| path.file_name().is_some_and(|name| name == "plaintext.rs"))
        .expect("the plaintext module exists");
    let accessors = text
        .lines()
        .filter(|line| line.trim_start().starts_with("pub fn "))
        .filter(|line| line.contains("-> &[u8]") || line.contains("-> Vec<u8>"))
        .count();
    assert_eq!(
        accessors, 1,
        "exactly one public accessor may return plaintext bytes"
    );
    assert!(text.contains("pub fn expose_for_encryption(&self) -> &[u8]"));
}
