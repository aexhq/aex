//! `aex-release-tool` — the release graph, admission, manifest and evidence
//! library used by CI and by the private deployment repository.
//!
//! The binary is a thin argument shell over this library, so every behaviour is
//! unit-testable without spawning a process.
//!
//! Three properties hold throughout and are worth stating once:
//!
//! * **One canonicalizer.** Every digest is `sha256` over RFC 8785 JCS bytes.
//! * **No warning path.** A check either reports zero violations and exits `0`,
//!   or reports every violation it found and exits its classification code.
//!   There is no `--force`.
//! * **Nothing is built at release time.** The tool prints build commands and
//!   verifies bytes somebody else produced; it never invokes a compiler.

pub mod admit;
pub mod artifact;
pub mod canon;
pub mod certify;
pub mod describe;
pub mod error;
pub mod evidence;
pub mod graph;
pub mod janitor;
pub mod ledger;
pub mod manifest;
pub mod meta;
pub mod migration;
pub mod pack;
pub mod policy;
pub mod private_path;
pub mod publication;
pub mod release_contract;
pub mod schemas;
pub mod selftest;
pub mod test_registry;
pub mod verification;
