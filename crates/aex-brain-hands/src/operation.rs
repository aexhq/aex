//! Brain-side operation construction and result incorporation.
//!
//! `git`, `package_install` and `code_run` are **argv constructors over the one
//! guest `Exec` primitive**, not guest primitives of their own. That leaves exactly
//! one process path to audit inside the VM. There is deliberately no guest-side git
//! subcommand allowlist and none is claimed: `shell_exec` runs as root and could
//! run any git command anyway, so an allowlist would be a boundary root steps over.
//!
//! What the constructors *do* own is real — env hardening applied to every command
//! they build.

use std::collections::BTreeMap;

use aex_hands_protocol::operation::TerminalMetadata;
use aex_hands_protocol::rpc::{HandsOperationId, ResultChunk};
use aex_wire::ids::ContentHash;

/// The default wall bound for a foreground exec, in milliseconds.
pub const EXEC_WALL_MS: u64 = 600_000;

/// The default wall bound for a detached exec, in milliseconds.
pub const DETACHED_WALL_MS: u64 = 86_400_000;

/// The default wall bound for `code_run`, in milliseconds.
pub const CODE_RUN_WALL_MS: u64 = 120_000;

/// The maximum wall bound for `code_run`, in milliseconds.
pub const CODE_RUN_MAX_WALL_MS: u64 = 600_000;

/// The wall bound for `git` and `package_install`, in milliseconds.
pub const TOOLCHAIN_WALL_MS: u64 = 1_800_000;

/// The largest number of packages one install may name.
pub const MAX_PACKAGES: usize = 64;

/// The largest `code_run` body, in bytes.
pub const MAX_CODE_BYTES: usize = 1_048_576;

/// Which interpreter a `code_run` uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CodeLanguage {
    /// `python3`.
    Python,
    /// `node`.
    JavaScript,
    /// `bash`.
    Bash,
}

impl CodeLanguage {
    /// Every offered language.
    pub const ALL: [Self; 3] = [Self::Python, Self::JavaScript, Self::Bash];

    /// The interpreter binary.
    #[must_use]
    pub const fn interpreter(self) -> &'static str {
        match self {
            Self::Python => "python3",
            Self::JavaScript => "node",
            Self::Bash => "bash",
        }
    }

    /// The file extension the body is written with.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Python => "py",
            Self::JavaScript => "js",
            Self::Bash => "sh",
        }
    }
}

/// Which package manager a `package_install` drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PackageManager {
    /// The system package manager.
    Dnf,
    /// Python packages.
    Pip,
    /// Node packages.
    Npm,
    /// Rust crates.
    Cargo,
}

impl PackageManager {
    /// Every offered manager.
    pub const ALL: [Self; 4] = [Self::Dnf, Self::Pip, Self::Npm, Self::Cargo];

    /// The argv prefix this manager installs with.
    #[must_use]
    pub fn install_argv(self) -> Vec<String> {
        match self {
            Self::Dnf => vec!["dnf".to_owned(), "install".to_owned(), "-y".to_owned()],
            Self::Pip => vec!["pip".to_owned(), "install".to_owned()],
            Self::Npm => vec!["npm".to_owned(), "install".to_owned()],
            Self::Cargo => vec!["cargo".to_owned(), "install".to_owned()],
        }
    }
}

/// A constructed command, ready to become an `Exec` operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstructedCommand {
    /// The argument vector.
    pub argv: Vec<String>,
    /// Environment additions, already hardened.
    pub env: BTreeMap<String, String>,
    /// The wall bound.
    pub max_wall_ms: u64,
}

/// Why a command could not be constructed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConstructError {
    /// A git invocation named no subcommand.
    #[error("a git invocation needs at least one argument")]
    EmptyGitArgs,
    /// Too many packages.
    #[error("{count} packages exceeds the ceiling of {MAX_PACKAGES}")]
    TooManyPackages {
        /// How many arrived.
        count: usize,
    },
    /// The code body is too large.
    #[error("the code body is {bytes} bytes, the ceiling is {MAX_CODE_BYTES}")]
    CodeTooLarge {
        /// How large it was.
        bytes: usize,
    },
    /// The requested wall bound is above the tool's maximum.
    #[error("a wall bound of {requested} ms exceeds this tool's maximum of {maximum} ms")]
    WallBoundTooLarge {
        /// What was asked for.
        requested: u64,
        /// The maximum.
        maximum: u64,
    },
}

/// Builds a `git` invocation.
///
/// The fifteen `GIT_*` local-state variables are removed from the environment every
/// time, so a variable left over from a prior operation cannot silently redirect
/// this one's repository.
///
/// # Errors
///
/// Returns [`ConstructError::EmptyGitArgs`] when no subcommand is named.
pub fn git(
    subcommand: &[String],
    inherited: &BTreeMap<String, String>,
) -> Result<ConstructedCommand, ConstructError> {
    if subcommand.is_empty() {
        return Err(ConstructError::EmptyGitArgs);
    }
    let mut env = inherited.clone();
    aex_hands_tools::strip_git_local_state(&mut env);
    let mut argv = vec!["git".to_owned()];
    argv.extend_from_slice(subcommand);
    Ok(ConstructedCommand {
        argv,
        env,
        max_wall_ms: TOOLCHAIN_WALL_MS,
    })
}

/// Builds a `package_install` invocation.
///
/// # Errors
///
/// Returns [`ConstructError::TooManyPackages`] above the ceiling.
pub fn package_install(
    manager: PackageManager,
    packages: &[String],
    flags: &[String],
    inherited: &BTreeMap<String, String>,
) -> Result<ConstructedCommand, ConstructError> {
    if packages.len() > MAX_PACKAGES {
        return Err(ConstructError::TooManyPackages {
            count: packages.len(),
        });
    }
    let mut env = inherited.clone();
    aex_hands_tools::strip_git_local_state(&mut env);
    let mut argv = manager.install_argv();
    argv.extend_from_slice(flags);
    argv.extend_from_slice(packages);
    Ok(ConstructedCommand {
        argv,
        env,
        max_wall_ms: TOOLCHAIN_WALL_MS,
    })
}

/// Builds a `code_run` invocation.
///
/// The body is written to `<root>/.aex/code-exec/<id>.<ext>` and the interpreter is
/// pointed at the file. It is never passed on stdin: a program that reads its own
/// stdin would consume its own source.
///
/// # Errors
///
/// Returns [`ConstructError`] when the body is too large or the wall bound is above
/// the tool's maximum.
pub fn code_run(
    language: CodeLanguage,
    code: &str,
    root: &str,
    id: &str,
    max_wall_ms: Option<u64>,
    inherited: &BTreeMap<String, String>,
) -> Result<ConstructedCommand, ConstructError> {
    if code.len() > MAX_CODE_BYTES {
        return Err(ConstructError::CodeTooLarge { bytes: code.len() });
    }
    let requested = max_wall_ms.unwrap_or(CODE_RUN_WALL_MS);
    if requested > CODE_RUN_MAX_WALL_MS {
        return Err(ConstructError::WallBoundTooLarge {
            requested,
            maximum: CODE_RUN_MAX_WALL_MS,
        });
    }
    let mut env = inherited.clone();
    aex_hands_tools::strip_git_local_state(&mut env);
    let file = format!(
        "{}/.aex/code-exec/{id}.{}",
        root.trim_end_matches('/'),
        language.extension()
    );
    Ok(ConstructedCommand {
        argv: vec![language.interpreter().to_owned(), file],
        env,
        max_wall_ms: requested,
    })
}

/// Why a result could not be incorporated.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IncorporateError {
    /// The chunk does not continue where the assembly left off.
    #[error("chunk starts at {offset} but the assembly is at {expected}")]
    OutOfOrder {
        /// Where the chunk starts.
        offset: u64,
        /// Where the assembly expected it to start.
        expected: u64,
    },
    /// The chunk would take the assembly past the declared length.
    #[error("chunk would take the body to {total} bytes, past the declared {declared}")]
    Overrun {
        /// Where the body would end up.
        total: u64,
        /// The effective declared length.
        declared: u64,
    },
    /// The assembled body's length did not match the declared length.
    #[error("length mismatch: declared {declared}, assembled {assembled}")]
    LengthMismatch {
        /// What the guest declared.
        declared: u64,
        /// What Brain assembled.
        assembled: u64,
    },
    /// The assembled body's digest did not match the declared digest.
    #[error("digest mismatch: declared {declared}, computed {computed}")]
    DigestMismatch {
        /// What the guest declared.
        declared: ContentHash,
        /// What Brain computed.
        computed: ContentHash,
    },
}

/// The resumable assembly of one terminal body.
///
/// Brain asks only for bytes it has not incorporated, verifies `body_len` and
/// `digest` over the assembled whole, and only then commits the tool result. A mux
/// crash mid-pull resumes from the last incorporated offset, because the offset is
/// the only state there is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultAssembly {
    /// Which operation.
    operation: HandsOperationId,
    /// The guest's declared terminal metadata.
    terminal: TerminalMetadata,
    /// The bytes incorporated so far.
    body: Vec<u8>,
    /// The largest body Brain will assemble, whatever the guest declares.
    max_body_bytes: u64,
}

impl ResultAssembly {
    /// Opens an assembly, or resumes one from bytes already held.
    #[must_use]
    pub fn resume(
        operation: HandsOperationId,
        terminal: TerminalMetadata,
        already: Vec<u8>,
        max_body_bytes: u64,
    ) -> Self {
        Self {
            operation,
            terminal,
            body: already,
            max_body_bytes,
        }
    }

    /// Which operation this assembles.
    #[must_use]
    pub const fn operation(&self) -> HandsOperationId {
        self.operation
    }

    /// The offset the next pull should ask from.
    #[must_use]
    pub fn next_offset(&self) -> u64 {
        self.body.len() as u64
    }

    /// Whether the whole declared body has arrived.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.next_offset() >= self.terminal.body_len
    }

    /// Incorporates one chunk.
    ///
    /// # Errors
    ///
    /// Returns [`IncorporateError`] for a chunk that does not continue the
    /// assembly, or one that would overrun the declared length. Neither is held.
    pub fn incorporate(&mut self, chunk: &ResultChunk) -> Result<(), IncorporateError> {
        let expected = self.next_offset();
        if chunk.offset != expected {
            return Err(IncorporateError::OutOfOrder {
                offset: chunk.offset,
                expected,
            });
        }
        let total = expected.saturating_add(chunk.bytes.len() as u64);
        let declared = self.terminal.body_len.min(self.max_body_bytes);
        if total > declared {
            return Err(IncorporateError::Overrun { total, declared });
        }
        self.body.extend_from_slice(&chunk.bytes);
        Ok(())
    }

    /// Verifies the assembled whole and yields it.
    ///
    /// # Errors
    ///
    /// Returns [`IncorporateError::LengthMismatch`] or
    /// [`IncorporateError::DigestMismatch`]. The error keeps the diagnostic values
    /// and the body is dropped rather than journalled: a body that fails its own
    /// digest is not a tool result, it is evidence.
    pub fn finish(self) -> Result<Vec<u8>, IncorporateError> {
        let assembled = self.body.len() as u64;
        if assembled != self.terminal.body_len {
            return Err(IncorporateError::LengthMismatch {
                declared: self.terminal.body_len,
                assembled,
            });
        }
        let computed = ContentHash::from_bytes(*blake3::hash(&self.body).as_bytes());
        if computed != self.terminal.digest {
            return Err(IncorporateError::DigestMismatch {
                declared: self.terminal.digest,
                computed,
            });
        }
        Ok(self.body)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CODE_RUN_MAX_WALL_MS, CodeLanguage, ConstructError, IncorporateError, MAX_CODE_BYTES,
        MAX_PACKAGES, PackageManager, ResultAssembly, TOOLCHAIN_WALL_MS, code_run, git,
        package_install,
    };
    use aex_hands_protocol::operation::{OperationExit, TerminalMetadata, TerminalState};
    use aex_hands_protocol::rpc::{HandsOperationId, ResultChunk};
    use aex_wire::ids::{ContentHash, Uuid7};
    use aex_wire::types::Timestamp;
    use std::collections::BTreeMap;

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("a bounded instant")
    }

    fn operation() -> HandsOperationId {
        HandsOperationId(Uuid7::compose(1, [1; 10]))
    }

    fn digest(bytes: &[u8]) -> ContentHash {
        ContentHash::from_bytes(*blake3::hash(bytes).as_bytes())
    }

    fn inherited() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("PATH".to_owned(), "/usr/bin".to_owned()),
            ("GIT_DIR".to_owned(), "/tmp/evil.git".to_owned()),
            ("git_work_tree".to_owned(), "/tmp/evil".to_owned()),
            ("GIT_AUTHOR_NAME".to_owned(), "kept".to_owned()),
        ])
    }

    #[test]
    fn a_git_command_strips_the_local_state_variables_and_keeps_the_rest() {
        let built = git(&["status".to_owned()], &inherited()).expect("the command builds");
        assert_eq!(built.argv, vec!["git", "status"]);
        assert!(!built.env.contains_key("GIT_DIR"));
        assert!(
            !built.env.contains_key("git_work_tree"),
            "the removal is case-insensitive"
        );
        assert_eq!(
            built.env.get("GIT_AUTHOR_NAME").map(String::as_str),
            Some("kept")
        );
        assert_eq!(built.env.get("PATH").map(String::as_str), Some("/usr/bin"));
        assert_eq!(built.max_wall_ms, TOOLCHAIN_WALL_MS);
    }

    #[test]
    fn there_is_no_guest_side_git_subcommand_allowlist() {
        // Every subcommand builds, including the destructive ones. Claiming
        // otherwise would be a boundary root steps over with `shell_exec`.
        for subcommand in ["push", "reset", "clean", "filter-branch", "daemon"] {
            let built =
                git(&[subcommand.to_owned()], &BTreeMap::new()).expect("every subcommand builds");
            assert_eq!(built.argv[1], subcommand);
        }
        assert_eq!(
            git(&[], &BTreeMap::new()),
            Err(ConstructError::EmptyGitArgs)
        );
    }

    #[test]
    fn every_package_manager_has_its_own_install_prefix() {
        let cases = [
            (PackageManager::Dnf, vec!["dnf", "install", "-y", "jq"]),
            (PackageManager::Pip, vec!["pip", "install", "jq"]),
            (PackageManager::Npm, vec!["npm", "install", "jq"]),
            (PackageManager::Cargo, vec!["cargo", "install", "jq"]),
        ];
        for (manager, expected) in cases {
            let built = package_install(manager, &["jq".to_owned()], &[], &BTreeMap::new())
                .expect("the command builds");
            assert_eq!(built.argv, expected, "{manager:?}");
        }
        assert_eq!(PackageManager::ALL.len(), 4);
    }

    #[test]
    fn a_package_install_above_the_ceiling_is_refused() {
        let many: Vec<String> = (0..=MAX_PACKAGES).map(|n| format!("p{n}")).collect();
        assert_eq!(
            package_install(PackageManager::Pip, &many, &[], &BTreeMap::new()),
            Err(ConstructError::TooManyPackages {
                count: MAX_PACKAGES + 1
            })
        );
    }

    #[test]
    fn code_run_points_the_interpreter_at_a_file_and_never_at_stdin() {
        for language in CodeLanguage::ALL {
            let built = code_run(
                language,
                "print(1)",
                "/workspace",
                "01J0",
                None,
                &BTreeMap::new(),
            )
            .expect("the command builds");
            assert_eq!(built.argv[0], language.interpreter());
            assert_eq!(
                built.argv[1],
                format!("/workspace/.aex/code-exec/01J0.{}", language.extension()),
                "the body is a file, so a program that reads stdin does not eat its own source"
            );
            assert_eq!(built.argv.len(), 2);
        }
    }

    #[test]
    fn code_run_refuses_an_oversized_body_or_an_oversized_wall_bound() {
        let big = "x".repeat(MAX_CODE_BYTES + 1);
        assert_eq!(
            code_run(
                CodeLanguage::Python,
                &big,
                "/workspace",
                "a",
                None,
                &BTreeMap::new()
            ),
            Err(ConstructError::CodeTooLarge {
                bytes: MAX_CODE_BYTES + 1
            })
        );
        assert_eq!(
            code_run(
                CodeLanguage::Python,
                "x",
                "/workspace",
                "a",
                Some(CODE_RUN_MAX_WALL_MS + 1),
                &BTreeMap::new()
            ),
            Err(ConstructError::WallBoundTooLarge {
                requested: CODE_RUN_MAX_WALL_MS + 1,
                maximum: CODE_RUN_MAX_WALL_MS
            })
        );
    }

    fn terminal(body: &[u8]) -> TerminalMetadata {
        TerminalMetadata {
            state: TerminalState::Succeeded,
            exit: OperationExit::Ok,
            started_at: at(0),
            ended_at: at(1),
            body_len: body.len() as u64,
            digest: digest(body),
            truncated: false,
            failure: None,
        }
    }

    fn chunk(offset: u64, bytes: &[u8]) -> ResultChunk {
        ResultChunk {
            operation: operation(),
            offset,
            bytes: bytes.to_vec(),
            last: false,
        }
    }

    #[test]
    fn a_body_reassembles_across_chunk_boundaries() {
        let body = b"the whole deliverable";
        let mut assembly =
            ResultAssembly::resume(operation(), terminal(body), Vec::new(), 4_194_304);
        for start in (0..body.len()).step_by(4) {
            let end = (start + 4).min(body.len());
            assembly
                .incorporate(&chunk(start as u64, &body[start..end]))
                .expect("each chunk continues the assembly");
        }
        assert!(assembly.is_complete());
        assert_eq!(assembly.finish().expect("the whole verifies"), body);
    }

    #[test]
    fn a_pull_resumes_from_the_last_incorporated_offset() {
        let body = b"0123456789";
        let mut assembly =
            ResultAssembly::resume(operation(), terminal(body), body[..4].to_vec(), 4_194_304);
        assert_eq!(assembly.next_offset(), 4, "a mux crash resumes from here");
        assembly
            .incorporate(&chunk(4, &body[4..]))
            .expect("the rest continues the assembly");
        assert_eq!(assembly.finish().expect("the whole verifies"), body);
    }

    #[test]
    fn an_out_of_order_chunk_is_refused_rather_than_stitched() {
        let body = b"0123456789";
        let mut assembly =
            ResultAssembly::resume(operation(), terminal(body), Vec::new(), 4_194_304);
        assert_eq!(
            assembly.incorporate(&chunk(4, b"4567")),
            Err(IncorporateError::OutOfOrder {
                offset: 4,
                expected: 0
            })
        );
        assert_eq!(assembly.next_offset(), 0, "nothing was incorporated");
    }

    #[test]
    fn a_chunk_that_overruns_the_declared_length_is_refused_before_it_is_held() {
        let body = b"0123";
        let mut assembly =
            ResultAssembly::resume(operation(), terminal(body), Vec::new(), 4_194_304);
        assert_eq!(
            assembly.incorporate(&chunk(0, b"0123456789")),
            Err(IncorporateError::Overrun {
                total: 10,
                declared: 4
            })
        );
        assert_eq!(assembly.next_offset(), 0);
    }

    #[test]
    fn a_deliberate_length_or_digest_mismatch_journals_nothing_and_keeps_diagnostics() {
        let body = b"honest";
        let mut lying = terminal(body);
        lying.body_len = 3;
        let mut assembly = ResultAssembly::resume(operation(), lying, Vec::new(), 4_194_304);
        assembly
            .incorporate(&chunk(0, b"hon"))
            .expect("three bytes fit the lie");
        // The declared length was met, so the digest is what catches it.
        let Err(IncorporateError::DigestMismatch { declared, computed }) = assembly.finish() else {
            panic!("a forged digest must be refused");
        };
        assert_eq!(declared, digest(body));
        assert_eq!(computed, digest(b"hon"));

        // And a short assembly fails on length.
        let mut short = ResultAssembly::resume(operation(), terminal(body), Vec::new(), 4_194_304);
        short
            .incorporate(&chunk(0, b"hon"))
            .expect("a partial chunk is fine");
        assert_eq!(
            short.finish(),
            Err(IncorporateError::LengthMismatch {
                declared: 6,
                assembled: 3
            })
        );
    }

    #[test]
    fn brains_own_body_ceiling_bounds_a_guest_that_declares_a_huge_body() {
        let mut forged = terminal(b"");
        forged.body_len = 10 * 1024 * 1024 * 1024;
        let mut assembly = ResultAssembly::resume(operation(), forged, Vec::new(), 1_024);
        assert_eq!(
            assembly.incorporate(&chunk(0, &vec![b'x'; 2_048])),
            Err(IncorporateError::Overrun {
                total: 2_048,
                declared: 1_024
            }),
            "Brain's own ceiling wins over the guest's declaration"
        );
    }
}
