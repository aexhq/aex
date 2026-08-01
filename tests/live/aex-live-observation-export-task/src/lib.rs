//! Live-test companion package for the `observation-export-task` deployable.
//!
//! Primary live concerns: multi-GB bounded memory, multipart resume, abort and hash, task
//! loss, cancel and delete races.
//!
//! What this suite owes, and nothing else can answer:
//!
//! - **A multi-GB export has a flat memory profile.** The reservation model is
//!   proven from the plan today; the live suite must measure resident memory
//!   across a multi-GB generation and show it does not grow with page count.
//! - **Resume is exact.** Killing the task mid-upload and restarting it must
//!   produce byte-identical output and the identical manifest hash, with
//!   `ListParts` read to exhaustion and a truncated listing treated as a hard
//!   integrity failure.
//! - **Losing the publication fence exits zero** and aborts the multipart
//!   upload, leaving no orphaned upload for a lifecycle rule to clean up.
//!
//! This package is `publish = false`, is never linked into a production artifact, uses the
//! least privileged test identity, and is selected only by the live lane. A test here must
//! fail on a missing prerequisite; it must never self-skip.
