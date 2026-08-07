//! `[package] description` is generated from the entry file's `//!` header.
//!
//! A member says what it is in exactly one place — the crate doc — and the
//! manifest field is a mechanical projection of it, verified by exact string
//! equality. Two hand-maintained copies of one sentence is the drift this
//! module exists to prevent, so a "description is present and non-empty" check
//! would be the same failure as no check at all.
//!
//! # Invariants
//!
//! - the description is exactly the first `//!` paragraph joined with single
//!   spaces; there is no stripping heuristic, so the projection is reversible
//!   by eye
//! - that paragraph is one sentence, because the field renders in listings that
//!   have room for one
//! - reading and writing use the same projection, so `description build`
//!   followed by `check` is always green

use std::path::{Path, PathBuf};

use crate::metadata::WorkspaceMetadata;
use crate::rules::Violation;

/// Why the descriptions could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum DescriptionError {
    /// A file could not be read or written.
    #[error("cannot access `{path}`: {source}")]
    Io {
        /// The path that failed.
        path: String,
        /// The underlying failure.
        source: std::io::Error,
    },
    /// A member has no `src/lib.rs` and no `src/main.rs`.
    #[error("member `{name}` has neither `src/lib.rs` nor `src/main.rs`")]
    NoEntryFile {
        /// The offending member.
        name: String,
    },
    /// A manifest has no `[package]` table to write into.
    #[error("`{path}` has no `[package]` table")]
    NoPackageTable {
        /// The offending manifest.
        path: String,
    },
}

/// One member's manifest description and the header it must be projected from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberDescription {
    /// Package name.
    pub name: String,
    /// Manifest path, relative to the workspace root.
    pub manifest: String,
    /// What `[package] description` currently says, if anything.
    pub declared: Option<String>,
    /// The first `//!` paragraph of the entry file, joined to one line.
    pub header: String,
}

/// Extracts the first `//!` paragraph, joined with single spaces.
///
/// The paragraph ends at the first blank `//!` line or the first line that is
/// not an inner doc comment, whichever comes first. Leading non-doc lines are
/// skipped so an `#![allow(...)]` above the header does not hide it.
#[must_use]
pub fn first_doc_paragraph(source: &str) -> Option<String> {
    let mut lines = Vec::new();
    let mut started = false;
    for line in source.lines() {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix("//!") else {
            if started {
                break;
            }
            // Only attributes and blank lines may precede the header; anything
            // else means this file has no crate doc at all.
            if trimmed.is_empty() || trimmed.starts_with("#!") || trimmed.starts_with("//") {
                continue;
            }
            break;
        };
        let text = rest.strip_prefix(' ').unwrap_or(rest).trim_end();
        if text.is_empty() {
            if started {
                break;
            }
            continue;
        }
        started = true;
        lines.push(text.to_owned());
    }
    (!lines.is_empty()).then(|| lines.join(" "))
}

/// How many sentences the text contains.
///
/// A sentence ends at `.`, `!` or `?` followed by whitespace or end of input,
/// so a version number, a `0.1.0` or a path never counts as a boundary.
#[must_use]
pub fn sentence_count(text: &str) -> usize {
    let bytes: Vec<char> = text.chars().collect();
    let mut count = 0;
    for (index, character) in bytes.iter().enumerate() {
        if !matches!(character, '.' | '!' | '?') {
            continue;
        }
        match bytes.get(index + 1) {
            None => count += 1,
            Some(next) if next.is_whitespace() => count += 1,
            Some(_) => {}
        }
    }
    count
}

/// The entry file of a member directory: `src/lib.rs`, else `src/main.rs`.
fn entry_file(root: &Path, directory: &str) -> Option<PathBuf> {
    let source = directory
        .split('/')
        .fold(root.to_path_buf(), |accumulator, segment| {
            accumulator.join(segment)
        })
        .join("src");
    for candidate in ["lib.rs", "main.rs"] {
        let path = source.join(candidate);
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

/// Reads what every member declares and what its header says.
///
/// # Errors
///
/// Returns [`DescriptionError`] when a manifest or entry file cannot be read,
/// or when a member has no entry file at all.
pub fn read_members(
    root: &Path,
    metadata: &WorkspaceMetadata,
) -> Result<Vec<MemberDescription>, DescriptionError> {
    let directories = metadata
        .member_directories()
        .map_err(|_| DescriptionError::NoPackageTable {
            path: "cargo metadata".to_owned(),
        })?;
    let mut rows = Vec::new();
    for package in metadata.members() {
        let directory = directories
            .get(&package.name)
            .cloned()
            .unwrap_or_else(|| package.name.clone());
        let entry = entry_file(root, &directory).ok_or_else(|| DescriptionError::NoEntryFile {
            name: package.name.clone(),
        })?;
        let source = std::fs::read_to_string(&entry).map_err(|source| DescriptionError::Io {
            path: entry.display().to_string(),
            source,
        })?;
        rows.push(MemberDescription {
            name: package.name.clone(),
            manifest: format!("{directory}/Cargo.toml"),
            declared: package.description.clone(),
            header: first_doc_paragraph(&source).unwrap_or_default(),
        });
    }
    rows.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(rows)
}

/// Every member declares a description equal to its header's first paragraph,
/// and that paragraph is one sentence.
#[must_use]
pub fn check(members: &[MemberDescription]) -> Vec<Violation> {
    const RULE: &str = "package-description";
    let mut violations = Vec::new();
    for member in members {
        if member.header.is_empty() {
            violations.push(Violation {
                rule: RULE,
                detail: format!(
                    "`{}` has no `//!` header to derive `[package] description` from",
                    member.name
                ),
            });
            continue;
        }
        let sentences = sentence_count(&member.header);
        if sentences != 1 {
            violations.push(Violation {
                rule: RULE,
                detail: format!(
                    "`{}` opens with a {sentences}-sentence `//!` paragraph; split it with a blank `//!` line so the first paragraph is one sentence",
                    member.name
                ),
            });
        }
        match member.declared.as_deref() {
            None => violations.push(Violation {
                rule: RULE,
                detail: format!(
                    "`{}` declares no `[package] description`; run `aex-workspace-check description build`",
                    member.name
                ),
            }),
            Some(declared) if declared != member.header => violations.push(Violation {
                rule: RULE,
                detail: format!(
                    "`{}` describes itself as `{declared}` but its `//!` header says `{}`; run `aex-workspace-check description build`",
                    member.name, member.header
                ),
            }),
            Some(_) => {}
        }
    }
    violations
}

/// Renders a TOML basic string.
fn toml_string(value: &str) -> String {
    let mut rendered = String::with_capacity(value.len() + 2);
    rendered.push('"');
    for character in value.chars() {
        match character {
            '"' => rendered.push_str("\\\""),
            '\\' => rendered.push_str("\\\\"),
            other => rendered.push(other),
        }
    }
    rendered.push('"');
    rendered
}

/// Rewrites one manifest's `description` to `value`, returning the new text.
///
/// The field is inserted immediately after `version` when absent, which is
/// where Cargo's own templates put it.
///
/// # Errors
///
/// Returns [`DescriptionError::NoPackageTable`] when the manifest has no
/// `[package]` table.
pub fn rewrite_manifest(
    manifest: &str,
    text: &str,
    value: &str,
) -> Result<String, DescriptionError> {
    let rendered = format!("description = {}", toml_string(value));
    let mut lines: Vec<String> = text.lines().map(ToOwned::to_owned).collect();
    let mut in_package = false;
    let mut anchor = None;
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            if in_package {
                break;
            }
            in_package = trimmed == "[package]";
            if in_package {
                anchor = Some(index);
            }
            continue;
        }
        if !in_package {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("description")
            && rest.trim_start().starts_with('=')
        {
            lines[index] = rendered;
            return Ok(join(&lines, text));
        }
        if trimmed.starts_with("name") || trimmed.starts_with("version") {
            anchor = Some(index);
        }
    }
    let Some(anchor) = anchor else {
        return Err(DescriptionError::NoPackageTable {
            path: manifest.to_owned(),
        });
    };
    lines.insert(anchor + 1, rendered);
    Ok(join(&lines, text))
}

/// Rejoins lines, preserving whether the input ended with a newline.
fn join(lines: &[String], original: &str) -> String {
    let mut text = lines.join("\n");
    if original.ends_with('\n') {
        text.push('\n');
    }
    text
}

/// Writes every member's description into its manifest.
///
/// Returns the number of manifests changed.
///
/// # Errors
///
/// Returns [`DescriptionError`] when a manifest cannot be read or written.
pub fn build(root: &Path, members: &[MemberDescription]) -> Result<usize, DescriptionError> {
    let mut changed = 0;
    for member in members {
        if member.header.is_empty() {
            continue;
        }
        let path = member
            .manifest
            .split('/')
            .fold(root.to_path_buf(), |accumulator, segment| {
                accumulator.join(segment)
            });
        let text = std::fs::read_to_string(&path).map_err(|source| DescriptionError::Io {
            path: path.display().to_string(),
            source,
        })?;
        let rewritten = rewrite_manifest(&member.manifest, &text, &member.header)?;
        if rewritten == text {
            continue;
        }
        std::fs::write(&path, &rewritten).map_err(|source| DescriptionError::Io {
            path: path.display().to_string(),
            source,
        })?;
        changed += 1;
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::{
        MemberDescription, check, first_doc_paragraph, rewrite_manifest, sentence_count,
        toml_string,
    };

    #[test]
    fn a_wrapped_paragraph_joins_to_one_line() {
        let source = "//! `aex-a` owns the thing: the first part,\n//! and the second part.\n//!\n//! # Invariants\n\npub fn f() {}\n";
        assert_eq!(
            first_doc_paragraph(source).as_deref(),
            Some("`aex-a` owns the thing: the first part, and the second part.")
        );
    }

    #[test]
    fn the_paragraph_stops_at_the_first_blank_doc_line() {
        let source = "//! One.\n//!\n//! Two.\n";
        assert_eq!(first_doc_paragraph(source).as_deref(), Some("One."));
    }

    #[test]
    fn an_attribute_above_the_header_does_not_hide_it() {
        let source = "#![allow(clippy::pedantic)]\n//! One.\n";
        assert_eq!(first_doc_paragraph(source).as_deref(), Some("One."));
    }

    #[test]
    fn a_file_without_a_header_yields_nothing() {
        assert_eq!(first_doc_paragraph("pub fn f() {}\n"), None);
    }

    #[test]
    fn a_version_number_is_not_a_sentence_boundary() {
        assert_eq!(sentence_count("`aex-a` pins 0.1.0 exactly."), 1);
        assert_eq!(sentence_count("One. Two."), 2);
        assert_eq!(sentence_count("No boundary here"), 0);
    }

    #[test]
    fn a_missing_description_is_a_violation() {
        let members = vec![MemberDescription {
            name: "aex-a".to_owned(),
            manifest: "crates/aex-a/Cargo.toml".to_owned(),
            declared: None,
            header: "`aex-a` owns the thing.".to_owned(),
        }];
        let violations = check(&members);
        assert_eq!(violations.len(), 1);
        assert!(violations[0].detail.contains("declares no"));
    }

    #[test]
    fn a_drifted_description_is_a_violation() {
        let members = vec![MemberDescription {
            name: "aex-a".to_owned(),
            manifest: "crates/aex-a/Cargo.toml".to_owned(),
            declared: Some("something else".to_owned()),
            header: "`aex-a` owns the thing.".to_owned(),
        }];
        let violations = check(&members);
        assert_eq!(violations.len(), 1);
        assert!(violations[0].detail.contains("describes itself as"));
    }

    #[test]
    fn a_two_sentence_paragraph_is_a_violation() {
        let members = vec![MemberDescription {
            name: "aex-a".to_owned(),
            manifest: "crates/aex-a/Cargo.toml".to_owned(),
            declared: Some("One. Two.".to_owned()),
            header: "One. Two.".to_owned(),
        }];
        let violations = check(&members);
        assert_eq!(violations.len(), 1);
        assert!(violations[0].detail.contains("2-sentence"));
    }

    #[test]
    fn a_matching_description_is_silent() {
        let members = vec![MemberDescription {
            name: "aex-a".to_owned(),
            manifest: "crates/aex-a/Cargo.toml".to_owned(),
            declared: Some("`aex-a` owns the thing.".to_owned()),
            header: "`aex-a` owns the thing.".to_owned(),
        }];
        assert!(check(&members).is_empty());
    }

    #[test]
    fn the_field_is_inserted_after_version() {
        let text = "[package]\nname = \"aex-a\"\nversion = \"0.1.0\"\npublish = false\n";
        let rewritten =
            rewrite_manifest("crates/aex-a/Cargo.toml", text, "`aex-a` owns it.").expect("rewrites");
        assert_eq!(
            rewritten,
            "[package]\nname = \"aex-a\"\nversion = \"0.1.0\"\ndescription = \"`aex-a` owns it.\"\npublish = false\n"
        );
    }

    #[test]
    fn an_existing_field_is_replaced_in_place() {
        let text = "[package]\nname = \"aex-a\"\ndescription = \"old\"\npublish = false\n";
        let rewritten =
            rewrite_manifest("crates/aex-a/Cargo.toml", text, "new").expect("rewrites");
        assert!(rewritten.contains("description = \"new\""));
        assert!(!rewritten.contains("old"));
    }

    #[test]
    fn a_later_tables_description_is_not_touched() {
        let text = "[package]\nname = \"aex-a\"\nversion = \"0.1.0\"\n\n[dependencies.thing]\ndescription = \"not ours\"\n";
        let rewritten = rewrite_manifest("crates/aex-a/Cargo.toml", text, "ours").expect("rewrites");
        assert!(rewritten.contains("description = \"not ours\""));
        assert!(rewritten.contains("description = \"ours\""));
    }

    #[test]
    fn quotes_and_backslashes_are_escaped() {
        assert_eq!(toml_string(r#"a "b" \c"#), r#""a \"b\" \\c""#);
    }
}
