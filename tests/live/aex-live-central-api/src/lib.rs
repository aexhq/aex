//! Live-test companion package for the `central-api` deployable.
//!
//! Primary live concerns: in-process credential verification behind the load
//! balancer — a forged secret against a real key row must be refused, and an
//! unreachable authority must answer `503` rather than `401`; the two
//! unauthenticated device-flow routes; the merged surface answering control,
//! auth and billing routes from one listener; and the drain — `/internal/readyz`
//! must go `503` before the listener stops.
//!
//! What is proven here and nowhere else is the removal of API Gateway. A caller
//! that supplies its own `aex.*` context map must gain nothing at all, because
//! the load balancer forwards headers verbatim and the gateway that used to
//! strip them is gone.
//!
//! This package is `publish = false`, is never linked into a production artifact, uses the
//! least privileged test identity, and is selected only by the live lane. A test here must
//! fail on a missing prerequisite; it must never self-skip.
