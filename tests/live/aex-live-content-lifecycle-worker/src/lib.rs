//! Live-test companion package for the `content-lifecycle-worker` deployable.
//!
//! Primary live concerns: staged orphans, mark/sweep and root pins, conditional unversioned
//! S3 keys, deletion denial and restored-data unwrap.
//!
//! This package is `publish = false`, is never linked into a production artifact, uses the
//! least privileged test identity, and is selected only by the live lane. A test here must
//! fail on a missing prerequisite; it must never self-skip.
