//! Migration bundle parsing and native `SQLx` construction.
//!
//! # The header
//!
//! The first line of every migration is
//! `-- aex-migration: tx=<yes|no> destructive=<no|yes> phase=<phase>`, three
//! unordered `key=value` fields separated by spaces. The identity is the
//! **filename** — `<14 digits>_<slug>.sql` — and is deliberately not repeated
//! inside the body, because a body that restates its own name is a second place
//! for the name to be wrong.
//!
//! `aex-release-tool migration bundle` is the release gate for exactly this
//! line and for [`PHASES`]. This parser matches it field for field; a file that
//! this crate admits and the gate refuses, or the reverse, is the drift both
//! exist to prevent.

use std::path::Path;

use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use sqlx::migrate::Migrator;

const EMBEDDED_BUNDLE_LOCK: &str = include_str!("../../../migrations/central/bundle.lock.json");
const EMBEDDED_GRANTS: &str = include_str!("../../../migrations/central/grants.toml");

// A future non-transactional migration must add its compile-time repair here.
// `MigrationBundle::embedded` refuses a lock row with `repair=true` until this
// table contains the same version, so the production repair path can never
// fall back to a source-tree file.
const EMBEDDED_REPAIRS: &[(i64, &str)] = &[];

/// The phase vocabulary, identical to the release gate's.
///
/// A backfill is a *command* (`central-schema-admin backfill`) driven by
/// `schema_admin.backfill_cursor`, not a phase; the migration that carries one
/// is `phase=data`.
pub const PHASES: [&str; 4] = ["expand", "contract", "baseline", "data"];

/// A parsed migration file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationFile {
    /// Fourteen-digit UTC version.
    pub version: i64,
    /// Stable slug.
    pub slug: String,
    /// Whether `SQLx` wraps it in a transaction.
    pub transactional: bool,
    /// Whether a backup evidence gate is required.
    pub destructive: bool,
    /// One of [`PHASES`].
    pub phase: String,
}

/// A validated linear chain.
#[derive(Debug, Clone)]
pub struct MigrationBundle {
    files: Vec<MigrationFile>,
}

impl MigrationBundle {
    /// Loads the exact bundle lock compiled into the executable.
    ///
    /// # Errors
    /// Rejects malformed lock bytes, an invalid identity, a nonlinear chain,
    /// or a non-transactional row whose repair was not also embedded.
    pub fn embedded() -> Result<Self, BundleError> {
        let lock: EmbeddedBundleLock =
            serde_json::from_str(EMBEDDED_BUNDLE_LOCK).map_err(BundleError::Lock)?;
        if lock.schema != "aex.migration-bundle.v1"
            || lock.files.is_empty()
            || lock.grants_digest != sha256(normalized(EMBEDDED_GRANTS).as_bytes())
        {
            return Err(BundleError::Identity);
        }
        let migrator = embedded_sqlx_migrator();
        if migrator.migrations.len() != lock.files.len() {
            return Err(BundleError::Identity);
        }
        let mut files = Vec::with_capacity(lock.files.len());
        for (row, migration) in lock.files.into_iter().zip(migrator.migrations.iter()) {
            let version = row
                .version
                .parse::<i64>()
                .map_err(|_| BundleError::Filename)?;
            let sql = normalized(migration.sql.as_str());
            let header = parse_header(
                sql.lines().next().ok_or(BundleError::Header)?,
                version,
                &row.slug,
            )?;
            if row.version.len() != 14
                || !row.version.bytes().all(|byte| byte.is_ascii_digit())
                || row.file != format!("{}_{}.sql", row.version, row.slug)
                || migration.version != version
                || row.sha256 != sha256(sql.as_bytes())
                || row.length != sql.len() as u64
                || !PHASES.contains(&row.phase.as_str())
                || header.transactional != row.tx
                || header.destructive != row.destructive
                || header.phase != row.phase
                || migration.no_tx == row.tx
            {
                return Err(BundleError::Identity);
            }
            if row.repair == row.tx
                || (row.repair && embedded_repair(version).is_none())
                || files
                    .last()
                    .is_some_and(|previous: &MigrationFile| previous.version >= version)
            {
                return Err(BundleError::NonLinear);
            }
            files.push(MigrationFile {
                version,
                slug: row.slug,
                transactional: row.tx,
                destructive: row.destructive,
                phase: row.phase,
            });
        }
        let expected_head = files.last().map_or(0, |file| file.version);
        if lock.head.parse::<i64>().ok() != Some(expected_head) {
            return Err(BundleError::Identity);
        }
        Ok(Self { files })
    }

    /// Reads and validates all forward `.sql` migrations.
    ///
    /// # Errors
    /// Rejects malformed filenames/headers, duplicate or non-increasing versions,
    /// and non-transactional files without a sibling repair file.
    pub fn load(path: &Path) -> Result<Self, BundleError> {
        let mut paths = std::fs::read_dir(path)
            .map_err(BundleError::Io)?
            .map(|entry| entry.map(|value| value.path()).map_err(BundleError::Io))
            .collect::<Result<Vec<_>, _>>()?;
        paths.retain(|file| {
            file.extension().is_some_and(|extension| extension == "sql")
                && !file
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(".repair.sql"))
        });
        paths.sort();
        let mut files = Vec::with_capacity(paths.len());
        for file in paths {
            let name = file
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or(BundleError::Filename)?;
            let (version_text, slug_with_ext) =
                name.split_once('_').ok_or(BundleError::Filename)?;
            if version_text.len() != 14 || !version_text.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(BundleError::Filename);
            }
            let slug = slug_with_ext
                .strip_suffix(".sql")
                .ok_or(BundleError::Filename)?;
            let version = version_text
                .parse::<i64>()
                .map_err(|_| BundleError::Filename)?;
            let content = std::fs::read_to_string(&file).map_err(BundleError::Io)?;
            let first = content.lines().next().ok_or(BundleError::Header)?;
            let parsed = parse_header(first, version, slug)?;
            if !parsed.transactional {
                let repair = path.join(format!("{version_text}_{slug}.repair.sql"));
                if !content
                    .lines()
                    .any(|line| line.trim() == "-- no-transaction")
                    || !repair.exists()
                {
                    return Err(BundleError::MissingRepair);
                }
            }
            if files
                .last()
                .is_some_and(|previous: &MigrationFile| previous.version >= version)
            {
                return Err(BundleError::NonLinear);
            }
            files.push(parsed);
        }
        if files.is_empty() {
            return Err(BundleError::Empty);
        }
        Ok(Self { files })
    }

    /// Highest bundled migration.
    #[must_use]
    pub fn head(&self) -> i64 {
        self.files.last().map_or(0, |file| file.version)
    }

    /// Ordered version list.
    #[must_use]
    pub fn versions(&self) -> Vec<i64> {
        self.files.iter().map(|file| file.version).collect()
    }

    /// Whether any bundled migration declares itself destructive.
    ///
    /// A destructive bundle needs recorded backup evidence and an explicit
    /// release gate; the runner refuses one without both.
    #[must_use]
    pub fn has_destructive(&self) -> bool {
        self.files.iter().any(|file| file.destructive)
    }

    /// The parsed file for `version`, when the bundle holds one.
    #[must_use]
    pub fn file(&self, version: i64) -> Option<&MigrationFile> {
        self.files.iter().find(|file| file.version == version)
    }

    /// The deterministic confirmation token for one repair.
    ///
    /// Derived from the version rather than minted, so the token a failed
    /// precondition prints is the token the operator can be told to type, and
    /// nothing else opens the repair path.
    #[must_use]
    pub fn repair_token(version: i64) -> String {
        let digest = blake3::hash(format!("aex:schema-repair:{version}").as_bytes());
        format!("rpr_{}", &digest.to_hex()[..16])
    }

    /// The committed sibling repair file for one non-transactional migration.
    ///
    /// # Errors
    ///
    /// Returns [`BundleError::MissingRepair`] when the version is not bundled,
    /// is transactional, or its repair was not compiled into this executable.
    /// A transactional migration is re-entrant by rerunning the exact artifact
    /// and therefore has no repair path at all.
    pub fn repair_sql(&self, version: i64) -> Result<&'static str, BundleError> {
        let file = self.file(version).ok_or(BundleError::MissingRepair)?;
        if file.transactional {
            return Err(BundleError::MissingRepair);
        }
        embedded_repair(version).ok_or(BundleError::MissingRepair)
    }
}

/// Constructs `SQLx`'s native migrator with fail-closed history settings.
///
/// `sqlx::migrate!` validates and embeds the committed directory at compile
/// time, so the production image needs no sibling source-tree files.
#[must_use]
pub fn native_migrator() -> Migrator {
    let mut migrator = embedded_sqlx_migrator();
    migrator.create_schema("schema_admin");
    migrator.dangerous_set_table_name("schema_admin._sqlx_migrations");
    migrator.set_ignore_missing(false);
    migrator.set_locking(true);
    migrator
}

fn embedded_sqlx_migrator() -> Migrator {
    sqlx::migrate!("../../migrations/central")
}

fn normalized(text: &str) -> std::borrow::Cow<'_, str> {
    if text.contains("\r\n") {
        std::borrow::Cow::Owned(text.replace("\r\n", "\n"))
    } else {
        std::borrow::Cow::Borrowed(text)
    }
}

fn sha256(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut text = String::with_capacity(71);
    text.push_str("sha256:");
    for byte in Sha256::digest(bytes) {
        write!(&mut text, "{byte:02x}").expect("writing to a String cannot fail");
    }
    text
}

fn embedded_repair(version: i64) -> Option<&'static str> {
    EMBEDDED_REPAIRS
        .iter()
        .find_map(|(candidate, sql)| (*candidate == version).then_some(*sql))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EmbeddedBundleLock {
    schema: String,
    head: String,
    files: Vec<EmbeddedMigrationFile>,
    grants_digest: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EmbeddedMigrationFile {
    version: String,
    slug: String,
    file: String,
    sha256: String,
    length: u64,
    tx: bool,
    destructive: bool,
    phase: String,
    repair: bool,
}

/// Parses one header line for the migration the filename already identifies.
fn parse_header(line: &str, version: i64, slug: &str) -> Result<MigrationFile, BundleError> {
    let body = line
        .trim_start()
        .strip_prefix("-- aex-migration:")
        .ok_or(BundleError::Header)?
        .trim();
    let mut transactional = None;
    let mut destructive = None;
    let mut phase = None;
    for field in body.split_whitespace() {
        let (key, value) = field.split_once('=').ok_or(BundleError::Header)?;
        let flag = || match value {
            "yes" => Ok(true),
            "no" => Ok(false),
            _ => Err(BundleError::Header),
        };
        match key {
            "tx" => transactional = Some(flag()?),
            "destructive" => destructive = Some(flag()?),
            "phase" if PHASES.contains(&value) => phase = Some(value.to_owned()),
            _ => return Err(BundleError::Header),
        }
    }
    let (Some(transactional), Some(destructive), Some(phase)) = (transactional, destructive, phase)
    else {
        return Err(BundleError::Header);
    };
    Ok(MigrationFile {
        version,
        slug: slug.to_owned(),
        transactional,
        destructive,
        phase,
    })
}

/// Invalid migration bundle.
#[derive(Debug, thiserror::Error)]
pub enum BundleError {
    /// Filesystem failure.
    #[error("migration bundle I/O failed: {0}")]
    Io(std::io::Error),
    /// The embedded canonical bundle lock is not valid JSON.
    #[error("embedded migration bundle lock is invalid: {0}")]
    Lock(serde_json::Error),
    /// The embedded lock's schema, head, or file identity is inconsistent.
    #[error("embedded migration bundle identity is inconsistent")]
    Identity,
    /// Filename is not `<14 digits>_<slug>.sql`.
    #[error("migration filename is invalid")]
    Filename,
    /// First line is not the exact AEX header.
    #[error("migration header is invalid")]
    Header,
    /// Chain is duplicate or non-increasing.
    #[error("migration chain is not strictly increasing")]
    NonLinear,
    /// No forward migrations exist.
    #[error("migration bundle is empty")]
    Empty,
    /// A non-transactional migration lacks its deterministic repair.
    #[error("non-transactional migration lacks -- no-transaction or sibling repair")]
    MissingRepair,
}

#[cfg(test)]
mod tests {
    use super::{MigrationBundle, PHASES, parse_header};

    #[test]
    fn every_embedded_migration_byte_matches_the_reviewed_bundle_lock() {
        let bundle = MigrationBundle::embedded()
            .expect("every SQLx migration byte matches its reviewed lock identity");
        assert_eq!(bundle.head(), 20_260_813_000_100);
    }

    #[test]
    fn the_header_carries_three_unordered_fields_and_no_identity() {
        let parsed = parse_header(
            "-- aex-migration: phase=expand destructive=yes tx=no",
            20_260_801_000_700,
            "slug",
        )
        .expect("the declared field set parses in any order");
        assert_eq!(parsed.version, 20_260_801_000_700);
        assert_eq!(parsed.slug, "slug");
        assert!(!parsed.transactional);
        assert!(parsed.destructive);
        assert_eq!(parsed.phase, "expand");
    }

    #[test]
    fn the_phase_vocabulary_is_exactly_the_release_gates() {
        // `aex-release-tool::migration::parse_header` admits these four and
        // nothing else. A file this crate accepts and the gate rejects would
        // pass review and fail the release.
        assert_eq!(PHASES, ["expand", "contract", "baseline", "data"]);
        for phase in PHASES {
            parse_header(
                &format!("-- aex-migration: tx=yes destructive=no phase={phase}"),
                1,
                "s",
            )
            .expect("every declared phase parses");
        }
        for rejected in ["backfill", "switch", "", "Expand"] {
            parse_header(
                &format!("-- aex-migration: tx=yes destructive=no phase={rejected}"),
                1,
                "s",
            )
            .expect_err("an undeclared phase is refused");
        }
    }

    #[test]
    fn a_missing_field_an_unknown_field_and_a_bare_word_are_all_refused() {
        for line in [
            "-- aex-migration: tx=yes destructive=no",
            "-- aex-migration: tx=yes destructive=no phase=expand speed=fast",
            "-- aex-migration: tx=yes destructive=no phase=expand extra",
            "-- aex-migration: tx=maybe destructive=no phase=expand",
            "-- something else",
        ] {
            parse_header(line, 1, "s").expect_err("an incomplete header is refused");
        }
    }
}
