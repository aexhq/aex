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

use std::path::{Path, PathBuf};

use sqlx::migrate::Migrator;

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
    /// Returns [`BundleError::MissingRepair`] when the version is not bundled or
    /// is transactional, and [`BundleError::Io`] when the sibling file cannot be
    /// read. A transactional migration is re-entrant by rerunning the exact
    /// artifact and therefore has no repair path at all.
    pub fn repair_sql(&self, path: &Path, version: i64) -> Result<String, BundleError> {
        let file = self.file(version).ok_or(BundleError::MissingRepair)?;
        if file.transactional {
            return Err(BundleError::MissingRepair);
        }
        let repair = path.join(format!("{version:014}_{}.repair.sql", file.slug));
        std::fs::read_to_string(repair).map_err(BundleError::Io)
    }
}

/// Constructs `SQLx`'s native migrator with fail-closed history settings.
///
/// # Errors
/// Returns `SQLx`'s source error if the committed directory cannot be parsed.
pub async fn native_migrator() -> Result<Migrator, sqlx::migrate::MigrateError> {
    let mut migrator = Migrator::new(bundle_path()).await?;
    migrator.create_schema("schema_admin");
    migrator.dangerous_set_table_name("schema_admin._sqlx_migrations");
    migrator.set_ignore_missing(false);
    migrator.set_locking(true);
    Ok(migrator)
}

/// Committed migration directory.
#[must_use]
pub fn bundle_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../migrations/central")
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
    use super::{PHASES, parse_header};

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
