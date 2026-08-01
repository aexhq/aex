//! Live-test companion package for the `runtime-control-worker` deployable.
//!
//! Primary live concerns: exact generation and fence, the busy/idle 180-second boundary,
//! provider unknown outcomes and the eight-hour cap.
//!
//! This package is `publish = false`, is never linked into a production artifact, uses the
//! least privileged test identity, and is selected only by the live lane. A test here must
//! fail on a missing prerequisite; it must never self-skip.
