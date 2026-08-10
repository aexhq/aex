//! The executor's performance contract, against a real plane.
//!
//! It has one, and it is not about throughput. The executor sits on a path the
//! model is blocked on, and its caller holds an activation, a lease and a
//! network-lane permit for the whole wait. What has to be measured is therefore
//! the *added* latency and the point at which admission starts queueing — not
//! requests per second.
//!
//! None of it can be earned in this run (OD-07), so each case fails loudly when
//! the live lane selects it rather than self-skipping.

const UNAVAILABLE: &str =
    "live evidence requires a deployed plane and credentials; this run deploys nothing (OD-07)";

#[test]
fn the_in_vpc_round_trip_costs_single_digit_milliseconds() {
    panic!(
        "{UNAVAILABLE}: measure the added latency of the hop against the in-process baseline. The \
         placement record's 1-3 ms figure is inferred and unmeasured, and it is the number the \
         whole out-of-process decision was priced on"
    );
}

#[test]
fn admission_does_not_queue_under_the_concurrency_the_brain_fleet_can_produce() {
    panic!(
        "{UNAVAILABLE}: drive 64 concurrent calls -- two tasks x 128 lane units / weight 4 -- and \
         assert nothing waited on the in-flight semaphore. If it queues, the executor has become \
         the thing that makes the fixed 2 x 16 activation ceiling bind"
    );
}

#[test]
fn the_ceiling_transaction_stays_inside_its_share_of_the_deadline() {
    panic!(
        "{UNAVAILABLE}: measure the two-item TransactWriteItems against the real table under load. \
         It runs on every call before the vendor request, so its tail is added to every tool call \
         a customer makes"
    );
}
