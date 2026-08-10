//! Live-test companion package for the `tool-executor` deployable.
//!
//! Primary live concerns: that the service has no address other than the private
//! name Cloud Map publishes; that the organization ceiling's conditional update
//! actually refuses against a real table; that a Brain-minted envelope verifies
//! end to end against the deployed key set; and that the task role can reach the
//! vendor and nothing else.
//!
//! This package is `publish = false`, is never linked into a production artifact,
//! uses the least privileged test identity, and is selected only by the live
//! lane. A test here must fail on a missing prerequisite; it must never
//! self-skip.
