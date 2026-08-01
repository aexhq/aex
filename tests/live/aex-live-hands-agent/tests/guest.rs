//! The guest agent inside a real `MicroVM`.
//!
//! Every case here needs a hypervisor. None can be earned in this run (OD-07), so
//! each **fails loudly** when the live lane selects it.
//!
//! What already passes off-VM in the default lane: the frame codec and its hostile
//! corpus, the journal ordering rules, replay classification including pid
//! recycling, the start identity table, output bounds, the cancel ladder and the
//! whole tool executor matrix over injected ports. What needs a real guest is
//! everything that depends on a kernel actually doing the thing.

/// The message every unavailable live case fails with.
const UNAVAILABLE: &str =
    "live evidence requires a real MicroVM guest; this run deploys nothing (OD-07)";

#[test]
fn a_crash_between_the_metadata_fsync_and_the_fork_replays_as_interrupted() {
    panic!("{UNAVAILABLE}: kill PID 1 between the real fsync and the real fork");
}

#[test]
fn a_double_forked_escapee_is_reported_honestly_rather_than_claimed_killed() {
    panic!("{UNAVAILABLE}: a real setsid escapee against the real cancel ladder");
}

#[test]
fn a_recycled_pid_is_never_adopted_by_replay() {
    panic!("{UNAVAILABLE}: exhaust the pid space so a real pid is genuinely reused");
}

#[test]
fn a_command_that_deletes_the_agent_binary_fails_only_its_own_operation() {
    panic!("{UNAVAILABLE}: rm /opt/aex/hands-agent as guest root and observe the blast radius");
}

#[test]
fn a_command_that_rewrites_the_journal_fails_only_its_own_operation() {
    panic!("{UNAVAILABLE}: tamper with /var/lib/aex/hands as guest root");
}

#[test]
fn no_spawn_environment_contains_an_aws_variable() {
    panic!("{UNAVAILABLE}: read the real environ of every spawned process group");
}

#[test]
fn a_disk_full_condition_terminalizes_honestly() {
    panic!("{UNAVAILABLE}: fill the real ext4 volume and observe the terminal record");
}

#[test]
fn a_browser_operation_on_a_base_variant_is_refused_before_any_process_starts() {
    panic!("{UNAVAILABLE}: assert capability_unavailable with a real process table to check");
}

#[test]
fn the_endpoint_connection_cap_counts_streams_or_connections() {
    panic!("{UNAVAILABLE}: drive shape.max_connections + 8 concurrent h2 streams and see");
}
