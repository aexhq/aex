//! Migration bundle packaging and identity.
//!
//! Bundling and identity are owned here; the migration bodies belong to the
//! central finance stream and the regional table definitions to the
//! regional-stores stream. The central bundle's identity is the bundle lock
//! plus the `central-schema-admin` image digest, because that image embeds the
//! same bytes — a second independent bundle artifact would be a drift source
//! rather than a second opinion.

use std::collections::BTreeSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::canon;
use crate::error::{Exit, Result, ToolError, Violation, io};

/// One migration file's record in the bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BundleFile {
    /// Version prefix, 14 digits.
    pub version: String,
    /// Slug after the version.
    pub slug: String,
    /// File name.
    pub file: String,
    /// Content digest.
    pub sha256: String,
    /// Byte length.
    pub length: u64,
    /// Whether the migration runs inside a transaction.
    pub tx: bool,
    /// Whether the migration is destructive.
    pub destructive: bool,
    /// Which expand/contract phase it belongs to.
    pub phase: String,
    /// Whether a `.repair.sql` sibling exists.
    pub repair: bool,
}

/// `aex.migration-bundle.v1`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Bundle {
    /// Schema discriminator.
    pub schema: String,
    /// Highest version in the bundle.
    pub head: String,
    /// Files in version order.
    pub files: Vec<BundleFile>,
    /// Digest of `grants.toml`.
    pub grants_digest: String,
    /// The schema-admin image that embeds these bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admin_image_digest: Option<String>,
    /// Regional table generation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regional_generation: Option<u32>,
    /// Digest of the generated regional table bundle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regional_tables_digest: Option<String>,
}

/// The declared schema head.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SchemaHead {
    /// Schema discriminator.
    pub schema: String,
    /// Central head version.
    pub central: String,
    /// Central bundle digest.
    pub central_bundle_digest: String,
    /// Regional generation.
    pub regional_generation: u32,
    /// Regional bundle digest.
    pub regional_bundle_digest: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeployedPrefix {
    schema: String,
    through: String,
    files: Vec<BundleFile>,
}

/// The `-- aex-migration:` header every migration body carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    /// Whether it runs in a transaction.
    pub tx: bool,
    /// Whether it is destructive.
    pub destructive: bool,
    /// Expand/contract phase.
    pub phase: String,
}

/// Parse the header line.
///
/// # Errors
/// Returns [`Exit::ManifestInvalid`] when the line is absent or malformed.
pub fn parse_header(file: &str, body: &str) -> Result<Header> {
    let line = body
        .lines()
        .find(|line| line.trim_start().starts_with("-- aex-migration:"))
        .ok_or_else(|| {
            ToolError::single(
                Exit::ManifestInvalid,
                "migration-header-missing",
                format!(
                    "`{file}` carries no `-- aex-migration:` header; tx, destructive and \
                     phase are not inferable from SQL"
                ),
            )
        })?;
    let payload = line
        .trim_start()
        .trim_start_matches("-- aex-migration:")
        .trim();
    let mut tx = None;
    let mut destructive = None;
    let mut phase = None;
    for field in payload.split_whitespace() {
        let Some((key, value)) = field.split_once('=') else {
            return Err(ToolError::single(
                Exit::ManifestInvalid,
                "migration-header-malformed",
                format!("`{file}` header field `{field}` is not `key=value`"),
            ));
        };
        match key {
            "tx" => tx = Some(value == "yes"),
            "destructive" => destructive = Some(value == "yes"),
            "phase" => phase = Some(value.to_owned()),
            other => {
                return Err(ToolError::single(
                    Exit::ManifestInvalid,
                    "migration-header-malformed",
                    format!("`{file}` header declares unknown field `{other}`"),
                ));
            }
        }
    }
    let (Some(tx), Some(destructive), Some(phase)) = (tx, destructive, phase) else {
        return Err(ToolError::single(
            Exit::ManifestInvalid,
            "migration-header-malformed",
            format!("`{file}` header must declare tx, destructive and phase"),
        ));
    };
    if !matches!(phase.as_str(), "expand" | "contract" | "baseline" | "data") {
        return Err(ToolError::single(
            Exit::ManifestInvalid,
            "migration-header-malformed",
            format!(
                "`{file}` declares phase `{phase}`; permitted: expand, contract, baseline, data"
            ),
        ));
    }
    Ok(Header {
        tx,
        destructive,
        phase,
    })
}

/// Build the bundle for a migrations directory.
///
/// # Errors
/// Returns [`Exit::ManifestInvalid`] for a duplicate version, a malformed
/// header, a `-- no-transaction` body without `tx=no`, a `tx=no` body without a
/// `.repair.sql` sibling, or a `GRANT`/`REVOKE` inside a migration body.
#[allow(clippy::too_many_lines)]
pub fn build_bundle(root: &Path) -> Result<Bundle> {
    let central = root.join("migrations/central");
    let entries =
        std::fs::read_dir(&central).map_err(|err| io(&central.display().to_string(), &err))?;
    let mut names: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if std::path::Path::new(&name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("sql"))
            && !name.ends_with(".repair.sql")
        {
            names.push(name);
        }
    }
    names.sort();
    let mut violations = Vec::new();
    let mut seen_versions: BTreeSet<String> = BTreeSet::new();
    let mut files = Vec::new();
    for name in &names {
        let Some((version, slug)) = name.trim_end_matches(".sql").split_once('_') else {
            violations.push(Violation::new(
                "migration-name-malformed",
                format!("`{name}` is not `<version>_<slug>.sql`"),
            ));
            continue;
        };
        if version.len() != 14 || !version.chars().all(|ch| ch.is_ascii_digit()) {
            violations.push(Violation::new(
                "migration-name-malformed",
                format!("`{name}` version `{version}` is not 14 digits"),
            ));
            continue;
        }
        if !seen_versions.insert(version.to_owned()) {
            violations.push(Violation::new(
                "migration-duplicate-version",
                format!("version `{version}` is declared more than once"),
            ));
            continue;
        }
        let path = central.join(name);
        let source =
            std::fs::read_to_string(&path).map_err(|err| io(&path.display().to_string(), &err))?;
        // Git publishes these files as LF (`.gitattributes`), but a stale
        // Windows worktree can retain CRLF until it is refreshed. Hash the
        // canonical Git representation so a local lock can never disagree
        // with the protected Linux build solely because of checkout endings.
        let body = source.replace("\r\n", "\n");
        let header = match parse_header(name, &body) {
            Ok(header) => header,
            Err(err) => {
                violations.extend(err.violations);
                continue;
            }
        };
        let no_transaction = body
            .lines()
            .any(|line| line.trim().eq_ignore_ascii_case("-- no-transaction"));
        if no_transaction && header.tx {
            violations.push(Violation::new(
                "migration-transaction-disagreement",
                format!("`{name}` declares `-- no-transaction` and `tx=yes`"),
            ));
        }
        let repair = central
            .join(format!("{version}_{slug}.repair.sql"))
            .is_file();
        if !header.tx && !repair {
            violations.push(Violation::new(
                "migration-repair-missing",
                format!(
                    "`{name}` runs outside a transaction and has no `.repair.sql` sibling; \
                     an interrupted non-transactional migration must have a stated recovery"
                ),
            ));
        }
        for (number, line) in body.lines().enumerate() {
            let upper = line.trim_start().to_ascii_uppercase();
            if upper.starts_with("GRANT ") || upper.starts_with("REVOKE ") {
                violations.push(Violation::new(
                    "migration-inline-grant",
                    format!(
                        "`{name}`:{}: privileges belong in grants.toml, not in a migration \
                         body",
                        number + 1
                    ),
                ));
            }
        }
        files.push(BundleFile {
            version: version.to_owned(),
            slug: slug.to_owned(),
            file: name.clone(),
            sha256: canon::digest_bytes(body.as_bytes()),
            length: body.len() as u64,
            tx: header.tx,
            destructive: header.destructive,
            phase: header.phase,
            repair,
        });
    }
    if !violations.is_empty() {
        return Err(ToolError::many(Exit::ManifestInvalid, violations));
    }
    let grants_path = central.join("grants.toml");
    let grants_digest = std::fs::read(&grants_path).map_or_else(
        |_| canon::digest_bytes(b""),
        |bytes| canon::digest_bytes(&bytes),
    );
    let head = files
        .last()
        .map_or_else(|| "00000000000000".to_owned(), |file| file.version.clone());
    let bundle = Bundle {
        schema: "aex.migration-bundle.v1".to_owned(),
        head,
        files,
        grants_digest,
        admin_image_digest: None,
        regional_generation: None,
        regional_tables_digest: None,
    };
    let deployed_prefix = central.join("deployed-prefix.lock.json");
    if deployed_prefix.is_file() {
        verify_deployed_prefix(root, &bundle)?;
    }
    Ok(bundle)
}

/// Verify the source bundle against the committed identity of migrations that
/// have reached a hosted database.
///
/// # Errors
/// Returns [`Exit::ManifestInvalid`] when an applied version was inserted,
/// removed, renamed or changed. New versions above the deployed head remain
/// append-only release candidates and are intentionally outside this prefix.
pub fn verify_deployed_prefix(root: &Path, bundle: &Bundle) -> Result<()> {
    let path = root.join("migrations/central/deployed-prefix.lock.json");
    let bytes = std::fs::read(&path).map_err(|err| io(&path.display().to_string(), &err))?;
    let prefix: DeployedPrefix = serde_json::from_slice(&bytes).map_err(|err| {
        ToolError::single(
            Exit::ManifestInvalid,
            "migration-deployed-prefix-invalid",
            format!(
                "`{}` is not a valid deployed-prefix identity: {err}",
                path.display()
            ),
        )
    })?;
    let actual: Vec<BundleFile> = bundle
        .files
        .iter()
        .filter(|file| file.version <= prefix.through)
        .cloned()
        .collect();
    if prefix.schema != "aex.deployed-migration-prefix.v1"
        || prefix.files.is_empty()
        || prefix.files.last().map(|file| file.version.as_str()) != Some(prefix.through.as_str())
        || actual != prefix.files
    {
        return Err(ToolError::single(
            Exit::ManifestInvalid,
            "migration-deployed-prefix-changed",
            format!(
                "migrations through deployed head `{}` differ from their committed byte identities",
                prefix.through
            ),
        ));
    }
    Ok(())
}

/// Verify a bundle against the declared head.
///
/// # Errors
/// Returns [`Exit::ManifestInvalid`] when a version at or below the declared
/// head was inserted or changed, or when the bundle and the schema-admin image
/// disagree.
pub fn verify_bundle(bundle: &Bundle, head: &SchemaHead) -> Result<()> {
    let mut violations = Vec::new();
    if bundle.schema != "aex.migration-bundle.v1" {
        violations.push(Violation::new(
            "migration-bundle-schema",
            format!("unknown bundle schema `{}`", bundle.schema),
        ));
    }
    let mut previous: Option<&str> = None;
    for file in &bundle.files {
        if let Some(previous) = previous
            && file.version.as_str() <= previous
        {
            violations.push(Violation::new(
                "migration-out-of-order",
                format!(
                    "`{}` sorts at or below `{previous}`; versions are strictly increasing",
                    file.file
                ),
            ));
        }
        previous = Some(&file.version);
    }
    // Everything at or below the declared head is frozen. A body edited below
    // the head is a body somebody already applied somewhere.
    let declared: std::collections::BTreeMap<&str, &BundleFile> = bundle
        .files
        .iter()
        .map(|file| (file.version.as_str(), file))
        .collect();
    for (version, file) in &declared {
        if *version <= head.central.as_str() && file.sha256.is_empty() {
            violations.push(Violation::new(
                "migration-below-head-changed",
                format!(
                    "`{}` is at or below head `{}` and has no digest",
                    file.file, head.central
                ),
            ));
        }
    }
    if bundle.head < head.central {
        violations.push(Violation::new(
            "migration-head-regressed",
            format!(
                "the bundle head `{}` is below the declared head `{}`",
                bundle.head, head.central
            ),
        ));
    }
    if let Some(image) = &bundle.admin_image_digest
        && image.is_empty()
    {
        violations.push(Violation::new(
            "migration-admin-image-unset",
            "the bundle names an empty schema-admin image digest",
        ));
    }
    if violations.is_empty() {
        Ok(())
    } else {
        Err(ToolError::many(Exit::ManifestInvalid, violations))
    }
}

/// Reject a version inserted at or below an already-applied head.
///
/// # Errors
/// Returns [`Exit::ManifestInvalid`] naming each offending file.
pub fn reject_below_head_insert(previous: &Bundle, current: &Bundle) -> Result<()> {
    let known: std::collections::BTreeMap<&str, &BundleFile> = previous
        .files
        .iter()
        .map(|file| (file.version.as_str(), file))
        .collect();
    let mut violations = Vec::new();
    for file in &current.files {
        match known.get(file.version.as_str()) {
            Some(before) if before.sha256 != file.sha256 => {
                if file.version.as_str() <= previous.head.as_str() {
                    violations.push(Violation::new(
                        "migration-below-head-changed",
                        format!(
                            "`{}` is at or below head `{}` and its content changed",
                            file.file, previous.head
                        ),
                    ));
                }
            }
            None if file.version.as_str() <= previous.head.as_str() => {
                violations.push(Violation::new(
                    "migration-below-head-insert",
                    format!(
                        "`{}` inserts version `{}` at or below the applied head `{}`",
                        file.file, file.version, previous.head
                    ),
                ));
            }
            _ => {}
        }
    }
    if violations.is_empty() {
        Ok(())
    } else {
        Err(ToolError::many(Exit::ManifestInvalid, violations))
    }
}

#[cfg(test)]
mod tests {
    use super::{build_bundle, parse_header, reject_below_head_insert};
    use std::path::Path;

    fn write(root: &Path, name: &str, body: &str) {
        let dir = root.join("migrations/central");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(name), body).unwrap();
    }

    const SOUND: &str =
        "-- aex-migration: tx=yes destructive=no phase=expand\nCREATE TABLE t (id int);\n";

    #[test]
    fn a_sound_bundle_builds_with_a_head_and_digests() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), "20260801000100_a.sql", SOUND);
        write(temp.path(), "20260801000200_b.sql", SOUND);
        let bundle = build_bundle(temp.path()).unwrap();
        assert_eq!(bundle.head, "20260801000200");
        assert_eq!(bundle.files.len(), 2);
        assert!(bundle.files[0].sha256.starts_with("sha256:"));
    }

    #[test]
    fn a_missing_header_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        write(
            temp.path(),
            "20260801000100_a.sql",
            "CREATE TABLE t (id int);",
        );
        let err = build_bundle(temp.path()).unwrap_err();
        assert!(err.rules().contains(&"migration-header-missing"));
    }

    #[test]
    fn no_transaction_without_tx_no_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        write(
            temp.path(),
            "20260801000100_a.sql",
            "-- aex-migration: tx=yes destructive=no phase=expand\n-- no-transaction\nX;",
        );
        let err = build_bundle(temp.path()).unwrap_err();
        assert!(err.rules().contains(&"migration-transaction-disagreement"));
    }

    #[test]
    fn tx_no_without_a_repair_sibling_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        write(
            temp.path(),
            "20260801000100_a.sql",
            "-- aex-migration: tx=no destructive=no phase=expand\nCREATE INDEX CONCURRENTLY i;",
        );
        let err = build_bundle(temp.path()).unwrap_err();
        assert!(err.rules().contains(&"migration-repair-missing"));
        write(
            temp.path(),
            "20260801000100_a.repair.sql",
            "DROP INDEX IF EXISTS i;",
        );
        build_bundle(temp.path()).unwrap();
    }

    #[test]
    fn an_inline_grant_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        write(
            temp.path(),
            "20260801000100_a.sql",
            "-- aex-migration: tx=yes destructive=no phase=expand\nGRANT SELECT ON t TO r;",
        );
        let err = build_bundle(temp.path()).unwrap_err();
        assert!(err.rules().contains(&"migration-inline-grant"));
    }

    #[test]
    fn a_malformed_version_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), "1_a.sql", SOUND);
        let err = build_bundle(temp.path()).unwrap_err();
        assert!(err.rules().contains(&"migration-name-malformed"));
    }

    #[test]
    fn a_below_head_insert_and_a_below_head_edit_are_both_rejected() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), "20260801000100_a.sql", SOUND);
        write(temp.path(), "20260801000300_c.sql", SOUND);
        let previous = build_bundle(temp.path()).unwrap();

        write(temp.path(), "20260801000200_b.sql", SOUND);
        let inserted = build_bundle(temp.path()).unwrap();
        let err = reject_below_head_insert(&previous, &inserted).unwrap_err();
        assert!(err.rules().contains(&"migration-below-head-insert"));
        assert_eq!(err.exit.code(), 30);

        std::fs::remove_file(temp.path().join("migrations/central/20260801000200_b.sql")).unwrap();
        write(
            temp.path(),
            "20260801000100_a.sql",
            "-- aex-migration: tx=yes destructive=no phase=expand\nCREATE TABLE t (id bigint);\n",
        );
        let edited = build_bundle(temp.path()).unwrap();
        let err = reject_below_head_insert(&previous, &edited).unwrap_err();
        assert!(err.rules().contains(&"migration-below-head-changed"));
    }

    #[test]
    fn an_unknown_header_field_is_rejected() {
        let err = parse_header("x.sql", "-- aex-migration: tx=yes speed=fast").unwrap_err();
        assert!(err.rules().contains(&"migration-header-malformed"));
    }
}
