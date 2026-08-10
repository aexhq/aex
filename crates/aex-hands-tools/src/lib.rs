//! `aex-hands-tools` owns the guest-side tool executors: filesystem, development, workspace
//! materialize/persist and browser.
//!
//! # Invariants
//!
//! - every executor is a function over an injected filesystem or process port, so the whole
//!   matrix runs off-VM
//! - a path is validated against the guest root before it is used; traversal, NUL and
//!   relative form are refused
//! - argv is never interpreted: there is no shell, and every argument is passed verbatim
//! - the deny-by-default environment sets no `AWS_` variable and deletes every proxy
//!   variable unconditionally
//! - the revision check is stateless: Brain supplies the digest it read and the guest
//!   compares
//!
//! # What the containment check is, and is not
//!
//! [`observation::is_contained`] is a **structural tool contract, not a security
//! boundary**. H-BOUNDARY grants the customer real root and `shell_exec` reaches the whole
//! filesystem by design. Structured file tools stay inside the workspace root so revision
//! checking and persist have meaning, not to keep anyone out — claiming otherwise would be
//! the fictional boundary Area 4 rejects.
//!
//! # Not this crate's job
//!
//! - the protocol and its frames (`aex-hands-protocol`, `aex-hands-agent`)
//! - the supervisor, the journal and cancellation (`aex-hands-agent`)
//! - tool descriptors, JSON Schemas and public names (`aex-brain-tool-catalog`)
//! - the `git`, `package_install` and `code_run` argv constructors, which are Brain-side
//!   (`aex-brain-hands`) so the guest has exactly one process primitive to audit

pub mod command;
pub mod filesystem;
pub mod observation;
pub mod port;

pub use command::{
    CommandError, ENV_ALLOWLIST, ENV_PROXY_VARS, GIT_LOCAL_STATE_VARS, MAX_ARG_BYTES, MAX_ARGS,
    MAX_ENV_PAIRS, MAX_ENV_VALUE_BYTES, MAX_STDIN_BYTES, SpawnSpec, build_env, build_spawn,
    is_inheritable, is_valid_env_name, strip_git_local_state,
};
pub use filesystem::{
    EditError, EditOutcome, LineRange, ListEntry, ListOutcome, MatchMode, ReadOutcome, edit_file,
    list_dir, read_file, stat_path, write_file,
};
pub use observation::{SearchOutcome, SkipReason, is_contained, search_tree};
pub use port::{DirEntry, EntryKind, FsError, GuestFs, GuestProc, Meta, Pgid, ProcError, digest};
