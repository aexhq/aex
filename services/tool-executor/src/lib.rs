//! `tool-executor` runs platform-destination tool work outside `brain-mux`.
//!
//! One crate, one handler, one caller. The Brain sends a signed envelope and a
//! canonical argument document; this process resolves the tenant from the
//! envelope, admits the call against the organization's ceiling, and only then
//! touches the platform's vendor credential.
//!
//! # What the split actually buys
//!
//! Stated precisely, because it is easy to overclaim. It does **not** reduce the
//! Brain's authority: `brain-mux` still mints dispatch tickets for every tenant
//! and still says whose call this is. What it buys is custody — the vendor key
//! leaves the process that parses fetched pages, model output and tool results —
//! and a bound that survives a Brain bug, because [`spend`] re-checks the
//! organization ceiling itself and does not take the Brain's word for it.
//!
//! # Placement
//!
//! A tool runs here when its catalogue entry declares
//! `EgressClass::ManagedInternet`, and nowhere else. That is the whole rule:
//! *a tool that talks to a destination the model chose uses the session's
//! network; a tool that talks to a destination the platform fixed uses the
//! platform's.* Placement derives from that declaration and from nothing else —
//! not from the tool's name, not from configuration, not from a per-call
//! argument.
//!
//! # What is deliberately not here
//!
//! - **The approval gate.** It sits between the Brain and this process. A tool
//!   call that needed approval and did not get it never becomes a request.
//! - **Any public listener.** The only channel is a private one inside the VPC,
//!   and the security argument does not rest on that: [`admit`] is a
//!   cryptographic gate that holds even if the network gate does not.
//! - **Any customer credential path.** The one secret this process reads is the
//!   platform's own.

pub mod admit;
pub mod config;
pub mod handler;
pub mod run;
pub mod spend;
