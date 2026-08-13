//! Integration-profile evidence for the capability-separated AWS adapter.

use aex_session_telemetry_aws::{
    SessionTelemetryDeleter, SessionTelemetryReader, SessionTelemetryWriter,
};

#[test]
fn aws_capabilities_are_separate_types() {
    fn names<T>() -> &'static str {
        core::any::type_name::<T>()
    }

    assert_ne!(
        names::<SessionTelemetryWriter>(),
        names::<SessionTelemetryReader>()
    );
    assert_ne!(
        names::<SessionTelemetryReader>(),
        names::<SessionTelemetryDeleter>()
    );
}
