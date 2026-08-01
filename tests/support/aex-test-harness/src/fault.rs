//! Fault injection through ports, never through the product.
//!
//! Every fault in `15-test-architecture.md` section 7 acts through one of three
//! places: a port the system under test already depends on ([`Clock`],
//! [`ScriptedPort`]), the process or cloud lifecycle (the live lanes' own
//! tooling), or a network proxy in front of a dependency ([`Proxy`]). None of
//! them is a hook inside a product binary, which is why no product package may
//! declare a feature named `fault`, `test-hooks`, `chaos`, `debug-endpoints` or
//! `bypass-*`.

use std::collections::VecDeque;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};

/// The clock port a domain or application crate depends on.
///
/// Product code takes this by reference so expiry, TTL and backoff boundaries
/// are reachable without sleeping.
pub trait Clock: std::fmt::Debug + Send + Sync {
    /// The current instant.
    fn now(&self) -> OffsetDateTime;
}

/// A clock the test moves by hand.
#[derive(Debug)]
pub struct ScriptedClock {
    now: Mutex<OffsetDateTime>,
}

impl ScriptedClock {
    /// A clock starting at `start`.
    #[must_use]
    pub fn starting_at(start: OffsetDateTime) -> Self {
        Self {
            now: Mutex::new(start),
        }
    }

    /// Moves the clock forward.
    ///
    /// # Panics
    ///
    /// Panics when the clock mutex was poisoned by a panicking test.
    pub fn advance(&self, by: Duration) {
        let mut now = self.now.lock().expect("the scripted clock is not poisoned");
        *now += by;
    }
}

impl Clock for ScriptedClock {
    fn now(&self) -> OffsetDateTime {
        *self.now.lock().expect("the scripted clock is not poisoned")
    }
}

/// Why a scripted port could not answer.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FaultError {
    /// The system under test made more calls than the script programmed.
    ///
    /// This is a failure rather than a default answer: a silent default would
    /// turn "the code called the store one more time than we modelled" into a
    /// pass.
    #[error(
        "scripted port `{port}` was called {call} time(s) but only {programmed} response(s) were programmed"
    )]
    ScriptExhausted {
        /// The port's name.
        port: String,
        /// The one-based call number that found the script empty.
        call: usize,
        /// How many responses the script held.
        programmed: usize,
    },
}

/// A port that answers from a programmed sequence.
#[derive(Debug)]
pub struct ScriptedPort<T, E> {
    name: String,
    programmed: usize,
    responses: Mutex<VecDeque<Result<T, E>>>,
    calls: Mutex<usize>,
}

impl<T, E> ScriptedPort<T, E> {
    /// A port named `name` that answers with `responses`, in order.
    #[must_use]
    pub fn new(name: impl Into<String>, responses: Vec<Result<T, E>>) -> Self {
        Self {
            name: name.into(),
            programmed: responses.len(),
            responses: Mutex::new(responses.into()),
            calls: Mutex::new(0),
        }
    }

    /// The next programmed answer.
    ///
    /// # Errors
    ///
    /// Returns [`FaultError::ScriptExhausted`] when the script has no answer
    /// left, naming the port and the call number.
    ///
    /// # Panics
    ///
    /// Panics when the port's mutexes were poisoned by a panicking test.
    pub fn call(&self) -> Result<Result<T, E>, FaultError> {
        let mut calls = self
            .calls
            .lock()
            .expect("the scripted port is not poisoned");
        *calls += 1;
        let call = *calls;
        let mut responses = self
            .responses
            .lock()
            .expect("the scripted port is not poisoned");
        responses
            .pop_front()
            .ok_or_else(|| FaultError::ScriptExhausted {
                port: self.name.clone(),
                call,
                programmed: self.programmed,
            })
    }

    /// How many times the port was called.
    ///
    /// # Panics
    ///
    /// Panics when the port's mutex was poisoned.
    #[must_use]
    pub fn calls(&self) -> usize {
        *self
            .calls
            .lock()
            .expect("the scripted port is not poisoned")
    }

    /// Whether every programmed response was consumed.
    ///
    /// A test that programs five failures and consumes two proved less than it
    /// claimed; assert this at the end of the case.
    ///
    /// # Panics
    ///
    /// Panics when the port's mutex was poisoned.
    #[must_use]
    pub fn is_drained(&self) -> bool {
        self.responses
            .lock()
            .expect("the scripted port is not poisoned")
            .is_empty()
    }
}

/// One network fault in front of a dependency.
///
/// These are Toxiproxy toxics expressed as data. The harness never opens a
/// socket: the integration lane hands this description to the sidecar named in
/// `release/policy/test-images.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Toxic {
    /// Delay every packet.
    Latency {
        /// Added delay in milliseconds.
        latency_ms: u64,
        /// Random jitter in milliseconds.
        jitter_ms: u64,
    },
    /// Limit throughput.
    Bandwidth {
        /// Allowed rate in kilobytes per second.
        rate_kbps: u64,
    },
    /// Reset the connection.
    ResetPeer {
        /// Delay before the reset, in milliseconds.
        timeout_ms: u64,
    },
    /// Break the stream into small writes with a gap between them.
    SliceBytes {
        /// Average slice size in bytes.
        average_size: u64,
        /// Delay between slices, in microseconds.
        delay_us: u64,
    },
    /// Stop passing data and never close.
    Timeout {
        /// How long until data stops flowing, in milliseconds.
        timeout_ms: u64,
    },
}

/// A named proxy in front of one dependency, with its toxics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proxy {
    /// The proxy's name; the run prefixes it so two runs never collide.
    pub name: String,
    /// The address the system under test connects to.
    pub listen: String,
    /// The address the proxy forwards to.
    pub upstream: String,
    /// The toxics applied, in order.
    pub toxics: Vec<Toxic>,
}

impl Proxy {
    /// A proxy with no toxics yet.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        listen: impl Into<String>,
        upstream: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            listen: listen.into(),
            upstream: upstream.into(),
            toxics: Vec::new(),
        }
    }

    /// Adds a toxic.
    #[must_use]
    pub fn with(mut self, toxic: Toxic) -> Self {
        self.toxics.push(toxic);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::{Clock, FaultError, Proxy, ScriptedClock, ScriptedPort, Toxic};
    use time::{Duration, OffsetDateTime};

    #[test]
    fn a_scripted_clock_only_moves_when_told_to() {
        let start = OffsetDateTime::from_unix_timestamp(1_700_000_000).expect("a valid instant");
        let clock = ScriptedClock::starting_at(start);
        assert_eq!(clock.now(), start);
        clock.advance(Duration::minutes(90));
        assert_eq!(clock.now(), start + Duration::minutes(90));
    }

    #[test]
    fn a_scripted_port_answers_in_order_and_counts_calls() {
        let port: ScriptedPort<u8, &str> =
            ScriptedPort::new("store", vec![Ok(1), Err("throttled"), Ok(2)]);
        assert_eq!(port.call().expect("programmed"), Ok(1));
        assert_eq!(port.call().expect("programmed"), Err("throttled"));
        assert!(!port.is_drained());
        assert_eq!(port.call().expect("programmed"), Ok(2));
        assert!(port.is_drained());
        assert_eq!(port.calls(), 3);
    }

    #[test]
    fn an_over_called_port_fails_instead_of_inventing_an_answer() {
        let port: ScriptedPort<u8, &str> = ScriptedPort::new("store", vec![Ok(1)]);
        assert!(port.call().is_ok());
        let error = port.call().expect_err("the script is exhausted");
        assert_eq!(
            error,
            FaultError::ScriptExhausted {
                port: "store".to_owned(),
                call: 2,
                programmed: 1
            }
        );
        assert_eq!(
            error.to_string(),
            "scripted port `store` was called 2 time(s) but only 1 response(s) were programmed"
        );
    }

    #[test]
    fn a_proxy_description_round_trips_as_data() {
        let proxy = Proxy::new("pg", "127.0.0.1:15432", "127.0.0.1:5432")
            .with(Toxic::Latency {
                latency_ms: 250,
                jitter_ms: 50,
            })
            .with(Toxic::ResetPeer { timeout_ms: 1_000 });
        let json = serde_json::to_string(&proxy).expect("the proxy renders");
        let parsed: Proxy = serde_json::from_str(&json).expect("the proxy parses");
        assert_eq!(parsed, proxy);
        assert_eq!(parsed.toxics.len(), 2);
    }
}
