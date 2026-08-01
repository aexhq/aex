//! Live-test companion package for the `brain-mux` deployable.
//!
//! Primary live concerns: the provider, MCP and Hands paths, swarms, long
//! context/stream/tool waits, pressure, drain/failover and a 12/24-hour soak.
//!
//! This package is `publish = false`, is never linked into a production artifact, uses the
//! least privileged test identity, and is selected only by the live lane. A test here must
//! fail on a missing prerequisite; it must never self-skip.
