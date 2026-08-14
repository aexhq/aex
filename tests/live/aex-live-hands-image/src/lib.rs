//! Live-test companion package for the `hands-image` deployable.
//!
//! Primary live concerns: boot the launch image, credential/root/network isolation,
//! package manifest and hostile workload.
//!
//! This package is `publish = false`, is never linked into a production artifact, uses the
//! least privileged test identity, and is selected only by the live lane. A test here must
//! fail on a missing prerequisite; it must never self-skip.
