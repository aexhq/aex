//! Command construction: argv bounds and the deny-by-default guest environment.
//!
//! There is exactly **one** process primitive in the guest, so there is exactly one
//! thing to audit. `git`, `package_install` and `code_run` are Brain-side argv
//! constructors over this primitive; the guest carries no subcommand allowlist and
//! none is claimed, because `shell_exec` runs as root and could run any of them
//! anyway. Claiming a boundary that root can step over is the fictional boundary
//! Area 4 rejects.
//!
//! What the environment build *does* own is real: a deny-by-default allowlist, the
//! unconditional deletion of every proxy variable, and the guarantee that no `AWS_`
//! variable is ever set.

use std::collections::BTreeMap;

use aex_hands_protocol::operation::{EnvName, EnvValue, GuestPath};

/// The largest single argument, in bytes.
pub const MAX_ARG_BYTES: usize = 4_096;

/// The largest number of arguments.
pub const MAX_ARGS: usize = 4_096;

/// The largest number of caller-supplied environment pairs.
pub const MAX_ENV_PAIRS: usize = 64;

/// The largest environment value, in bytes.
pub const MAX_ENV_VALUE_BYTES: usize = 32_768;

/// The largest `stdin` payload, in bytes.
pub const MAX_STDIN_BYTES: usize = 1_048_576;

/// The variables a guest command may inherit. Everything else is dropped.
///
/// Deny-by-default, because the supervisor's own environment is not something a
/// customer command should be able to read by accident, and because an inherited
/// variable is the easiest way for one operation to silently reconfigure the next.
pub const ENV_ALLOWLIST: [&str; 11] = [
    "PATH",
    "LANG",
    "LANGUAGE",
    "TZ",
    "TERM",
    "COLORTERM",
    "TMPDIR",
    "TMP",
    "TEMP",
    "SSL_CERT_FILE",
    "PIP_CERT",
];

/// Allowlisted prefixes, for the `LC_*` locale family.
pub const ENV_ALLOWED_PREFIXES: [&str; 1] = ["LC_"];

/// Additional certificate-bundle variables that are allowed verbatim.
pub const ENV_CA_BUNDLES: [&str; 2] = ["REQUESTS_CA_BUNDLE", "CURL_CA_BUNDLE"];

/// Proxy variables, deleted unconditionally.
///
/// A customer who wants a proxy sets one themselves; inheriting the supervisor's
/// would silently route a customer's traffic through infrastructure they cannot see.
pub const ENV_PROXY_VARS: [&str; 8] = [
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "no_proxy",
];

/// The fifteen `GIT_*` local-state variables, removed case-insensitively from
/// every command the git route builds.
///
/// Without this, a prior operation could leave `GIT_DIR` set and silently redirect
/// a later operation's repository — a real cross-operation state leak, not a
/// theoretical one.
pub const GIT_LOCAL_STATE_VARS: [&str; 15] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_CONFIG",
    "GIT_CONFIG_GLOBAL",
    "GIT_CONFIG_SYSTEM",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_GRAFT_FILE",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_PREFIX",
    "GIT_REPLACE_REF_BASE",
    "GIT_SHALLOW_FILE",
];

/// Why a command could not be built.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CommandError {
    /// `argv` was empty.
    #[error("a command needs at least a program name")]
    EmptyArgv,
    /// Too many arguments.
    #[error("{count} arguments exceeds the ceiling of {MAX_ARGS}")]
    TooManyArgs {
        /// How many arrived.
        count: usize,
    },
    /// One argument was too long.
    #[error("argument {index} is {bytes} bytes, the ceiling is {MAX_ARG_BYTES}")]
    ArgTooLong {
        /// Which argument.
        index: usize,
        /// How long it was.
        bytes: usize,
    },
    /// An argument contained a NUL, which `execve` cannot carry.
    #[error("argument {index} contains a NUL byte")]
    ArgContainsNul {
        /// Which argument.
        index: usize,
    },
    /// Too many environment pairs.
    #[error("{count} environment pairs exceeds the ceiling of {MAX_ENV_PAIRS}")]
    TooManyEnvPairs {
        /// How many arrived.
        count: usize,
    },
    /// An environment name is outside the grammar.
    #[error("`{name}` is not a valid environment variable name")]
    InvalidEnvName {
        /// The rejected name.
        name: String,
    },
    /// An environment value is too long or contains a NUL.
    #[error("the value of `{name}` is not a valid environment value")]
    InvalidEnvValue {
        /// Which variable.
        name: String,
    },
    /// A caller tried to set an `AWS_` variable.
    #[error("`{name}` is refused: no AWS variable is ever set in a guest command")]
    AwsVariableRefused {
        /// The rejected name.
        name: String,
    },
    /// `stdin` was too large.
    #[error("stdin is {bytes} bytes, the ceiling is {MAX_STDIN_BYTES}")]
    StdinTooLarge {
        /// How large it was.
        bytes: usize,
    },
}

/// Everything needed to start one process group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnSpec {
    /// The argument vector. There is no shell.
    pub argv: Vec<String>,
    /// The working directory.
    pub cwd: GuestPath,
    /// The complete environment, already filtered.
    pub env: BTreeMap<String, String>,
    /// Standard input, when there is any.
    pub stdin: Option<Vec<u8>>,
}

/// Whether a name matches the environment-name grammar
/// `^[A-Za-z_][A-Za-z0-9_]{0,127}$`.
#[must_use]
pub fn is_valid_env_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 128 {
        return false;
    }
    let mut bytes = name.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == b'_') {
        return false;
    }
    bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// Whether an inherited variable survives the allowlist.
#[must_use]
pub fn is_inheritable(name: &str) -> bool {
    if ENV_PROXY_VARS.contains(&name) {
        return false;
    }
    if name.starts_with("AWS_") {
        return false;
    }
    ENV_ALLOWLIST.contains(&name)
        || ENV_CA_BUNDLES.contains(&name)
        || ENV_ALLOWED_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
}

/// Builds the environment for one guest command.
///
/// `inherited` is the supervisor's own environment. Everything outside the
/// allowlist is dropped, every proxy variable is deleted, `HOME` is the guest root,
/// and no `AWS_` variable survives or may be added.
///
/// # Errors
///
/// See [`CommandError`].
pub fn build_env(
    inherited: &BTreeMap<String, String>,
    requested: &[(EnvName, EnvValue)],
    root: &GuestPath,
) -> Result<BTreeMap<String, String>, CommandError> {
    if requested.len() > MAX_ENV_PAIRS {
        return Err(CommandError::TooManyEnvPairs {
            count: requested.len(),
        });
    }
    let mut env: BTreeMap<String, String> = inherited
        .iter()
        .filter(|(name, _)| is_inheritable(name))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    env.insert("HOME".to_owned(), root.as_str().to_owned());

    for (name, value) in requested {
        let name = &name.0;
        if !is_valid_env_name(name) {
            return Err(CommandError::InvalidEnvName { name: name.clone() });
        }
        if name.starts_with("AWS_") {
            return Err(CommandError::AwsVariableRefused { name: name.clone() });
        }
        let text = value.expose();
        if text.len() > MAX_ENV_VALUE_BYTES || text.contains('\0') {
            return Err(CommandError::InvalidEnvValue { name: name.clone() });
        }
        env.insert(name.clone(), text.to_owned());
    }
    Ok(env)
}

/// Removes the fifteen `GIT_*` local-state variables, case-insensitively.
pub fn strip_git_local_state(env: &mut BTreeMap<String, String>) {
    env.retain(|name, _| {
        !GIT_LOCAL_STATE_VARS
            .iter()
            .any(|forbidden| name.eq_ignore_ascii_case(forbidden))
    });
}

/// Validates and builds a spawn specification.
///
/// # Errors
///
/// See [`CommandError`].
pub fn build_spawn(
    argv: &[String],
    cwd: GuestPath,
    env: BTreeMap<String, String>,
    stdin: Option<Vec<u8>>,
) -> Result<SpawnSpec, CommandError> {
    if argv.is_empty() {
        return Err(CommandError::EmptyArgv);
    }
    if argv.len() > MAX_ARGS {
        return Err(CommandError::TooManyArgs { count: argv.len() });
    }
    for (index, argument) in argv.iter().enumerate() {
        if argument.len() > MAX_ARG_BYTES {
            return Err(CommandError::ArgTooLong {
                index,
                bytes: argument.len(),
            });
        }
        if argument.contains('\0') {
            return Err(CommandError::ArgContainsNul { index });
        }
    }
    if let Some(bytes) = &stdin
        && bytes.len() > MAX_STDIN_BYTES
    {
        return Err(CommandError::StdinTooLarge { bytes: bytes.len() });
    }
    Ok(SpawnSpec {
        argv: argv.to_vec(),
        cwd,
        env,
        stdin,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        CommandError, ENV_PROXY_VARS, GIT_LOCAL_STATE_VARS, MAX_ARG_BYTES, MAX_ARGS, MAX_ENV_PAIRS,
        MAX_STDIN_BYTES, build_env, build_spawn, is_valid_env_name, strip_git_local_state,
    };
    use aex_hands_protocol::operation::{EnvName, EnvValue, GuestPath, GuestRoot};
    use std::collections::BTreeMap;

    fn root() -> GuestPath {
        GuestPath::parse(&GuestRoot::workspace(), "/workspace").expect("the root parses")
    }

    fn inherited() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("PATH".to_owned(), "/usr/bin".to_owned()),
            ("LC_ALL".to_owned(), "C".to_owned()),
            (
                "HTTPS_PROXY".to_owned(),
                "http://egress.internal".to_owned(),
            ),
            ("http_proxy".to_owned(), "http://egress.internal".to_owned()),
            ("AWS_ACCESS_KEY_ID".to_owned(), "AKIAEXAMPLE".to_owned()),
            (
                "AWS_CONTAINER_CREDENTIALS_FULL_URI".to_owned(),
                "http://169.254.170.2".to_owned(),
            ),
            ("AEX_TOKEN_PEPPER".to_owned(), "secret".to_owned()),
            ("GIT_DIR".to_owned(), "/tmp/evil.git".to_owned()),
            ("HOME".to_owned(), "/root".to_owned()),
        ])
    }

    #[test]
    fn the_environment_is_deny_by_default() {
        let env = build_env(&inherited(), &[], &root()).expect("the build succeeds");
        assert_eq!(env.get("PATH").map(String::as_str), Some("/usr/bin"));
        assert_eq!(env.get("LC_ALL").map(String::as_str), Some("C"));
        assert!(
            !env.contains_key("AEX_TOKEN_PEPPER"),
            "a platform secret must never be inherited"
        );
        assert!(!env.contains_key("GIT_DIR"));
    }

    #[test]
    fn no_aws_variable_ever_reaches_a_spawn_environment() {
        let env = build_env(&inherited(), &[], &root()).expect("the build succeeds");
        assert!(
            env.keys().all(|name| !name.starts_with("AWS_")),
            "the spawn environment contained an AWS_ variable: {env:?}"
        );
        // Nor can a caller add one.
        let refused = build_env(
            &BTreeMap::new(),
            &[(
                EnvName("AWS_SECRET_ACCESS_KEY".to_owned()),
                EnvValue::new("x"),
            )],
            &root(),
        );
        assert_eq!(
            refused,
            Err(CommandError::AwsVariableRefused {
                name: "AWS_SECRET_ACCESS_KEY".to_owned()
            })
        );
    }

    #[test]
    fn every_proxy_variable_is_deleted_unconditionally() {
        let env = build_env(&inherited(), &[], &root()).expect("the build succeeds");
        for name in ENV_PROXY_VARS {
            assert!(!env.contains_key(name), "{name} survived");
        }
    }

    #[test]
    fn home_is_the_guest_root_and_not_the_supervisors() {
        let env = build_env(&inherited(), &[], &root()).expect("the build succeeds");
        assert_eq!(env.get("HOME").map(String::as_str), Some("/workspace"));
    }

    #[test]
    fn the_fifteen_git_local_state_variables_are_removed_case_insensitively() {
        let mut env = BTreeMap::new();
        for (index, name) in GIT_LOCAL_STATE_VARS.iter().enumerate() {
            // Alternate the casing so the removal cannot be an exact-match accident.
            let spelling = if index % 2 == 0 {
                (*name).to_owned()
            } else {
                name.to_lowercase()
            };
            env.insert(spelling, "/tmp/evil".to_owned());
        }
        env.insert("GIT_AUTHOR_NAME".to_owned(), "kept".to_owned());
        assert_eq!(env.len(), GIT_LOCAL_STATE_VARS.len() + 1);

        strip_git_local_state(&mut env);
        assert_eq!(
            env.keys().collect::<Vec<_>>(),
            vec!["GIT_AUTHOR_NAME"],
            "only the fifteen local-state variables are removed"
        );
    }

    #[test]
    fn the_environment_name_grammar_is_exact() {
        for good in ["PATH", "_x", "A1", "a_b_c", &"A".repeat(128)] {
            assert!(is_valid_env_name(good), "{good}");
        }
        for bad in ["", "1A", "a-b", "a b", "a=b", "a\0b", &"A".repeat(129)] {
            assert!(!is_valid_env_name(bad), "{bad}");
        }
    }

    #[test]
    fn a_hostile_environment_pair_is_refused_rather_than_sanitized() {
        let long = "x".repeat(32_769);
        let cases = [
            (
                vec![(EnvName("a-b".to_owned()), EnvValue::new("1"))],
                CommandError::InvalidEnvName {
                    name: "a-b".to_owned(),
                },
            ),
            (
                vec![(EnvName("A".to_owned()), EnvValue::new("x\0y"))],
                CommandError::InvalidEnvValue {
                    name: "A".to_owned(),
                },
            ),
            (
                vec![(EnvName("A".to_owned()), EnvValue::new(long))],
                CommandError::InvalidEnvValue {
                    name: "A".to_owned(),
                },
            ),
        ];
        for (requested, expected) in cases {
            assert_eq!(
                build_env(&BTreeMap::new(), &requested, &root()),
                Err(expected)
            );
        }

        let too_many: Vec<(EnvName, EnvValue)> = (0..=MAX_ENV_PAIRS)
            .map(|index| (EnvName(format!("V{index}")), EnvValue::new("1")))
            .collect();
        assert_eq!(
            build_env(&BTreeMap::new(), &too_many, &root()),
            Err(CommandError::TooManyEnvPairs {
                count: MAX_ENV_PAIRS + 1
            })
        );
    }

    #[test]
    fn hostile_argv_is_refused() {
        let env = BTreeMap::new();
        assert_eq!(
            build_spawn(&[], root(), env.clone(), None),
            Err(CommandError::EmptyArgv)
        );

        let too_many: Vec<String> = (0..=MAX_ARGS).map(|index| index.to_string()).collect();
        assert_eq!(
            build_spawn(&too_many, root(), env.clone(), None),
            Err(CommandError::TooManyArgs {
                count: MAX_ARGS + 1
            })
        );

        let long = vec!["sh".to_owned(), "x".repeat(MAX_ARG_BYTES + 1)];
        assert_eq!(
            build_spawn(&long, root(), env.clone(), None),
            Err(CommandError::ArgTooLong {
                index: 1,
                bytes: MAX_ARG_BYTES + 1
            })
        );

        let nul = vec!["sh".to_owned(), "a\0b".to_owned()];
        assert_eq!(
            build_spawn(&nul, root(), env.clone(), None),
            Err(CommandError::ArgContainsNul { index: 1 })
        );

        let big_stdin = vec![0u8; MAX_STDIN_BYTES + 1];
        assert_eq!(
            build_spawn(&["sh".to_owned()], root(), env, Some(big_stdin)),
            Err(CommandError::StdinTooLarge {
                bytes: MAX_STDIN_BYTES + 1
            })
        );
    }

    #[test]
    fn a_command_at_every_ceiling_is_accepted() {
        let argv: Vec<String> = std::iter::once("sh".to_owned())
            .chain((1..MAX_ARGS).map(|_| "x".repeat(MAX_ARG_BYTES)))
            .collect();
        assert_eq!(argv.len(), MAX_ARGS);
        let spec = build_spawn(
            &argv,
            root(),
            BTreeMap::new(),
            Some(vec![0u8; MAX_STDIN_BYTES]),
        )
        .expect("exactly at the ceiling is accepted");
        assert_eq!(spec.argv.len(), MAX_ARGS);
    }

    #[test]
    fn there_is_no_shell_and_no_argument_is_interpreted() {
        let argv = vec![
            "echo".to_owned(),
            "a; rm -rf /".to_owned(),
            "$(whoami)".to_owned(),
            "`id`".to_owned(),
            "|".to_owned(),
        ];
        let spec = build_spawn(&argv, root(), BTreeMap::new(), None).expect("argv is opaque");
        assert_eq!(
            spec.argv, argv,
            "the argument vector is passed through verbatim; there is nothing to interpret it"
        );
    }
}
