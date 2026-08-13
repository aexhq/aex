//! Installation contract for the standard JSON subscriber.

#[test]
fn json_diagnostics_install_once_and_accept_structured_events() {
    aex_platform_diagnostics::install_json().expect("the first installation succeeds");
    tracing::info!(
        target: "aex::diagnostics",
        event_name = "process.started",
        deployable = "fixture",
        "process started"
    );
    assert!(
        aex_platform_diagnostics::install_json().is_err(),
        "a second global subscriber must be reported rather than silently replacing the first"
    );
}
