//! OTLP responses.
//!
//! **There is no OTLP partial success.** `partial_success` is the empty message
//! on every success path, asserted by a test over all three signals, because a
//! batch either commits whole or is refused whole. A partial success would claim
//! a completeness the authority never recorded.

use prost::Message as _;

use crate::error::OtlpSignal;
use crate::proto::{collector_logs, collector_metrics, collector_trace};

/// The header carrying the admitted batch id.
pub const BATCH_ID_HEADER: &str = "aex-telemetry-batch-id";

/// The header carrying the admission receipt id.
pub const RECEIPT_ID_HEADER: &str = "aex-telemetry-receipt-id";

/// The success body for one signal, protobuf-encoded.
#[must_use]
pub fn success_protobuf(signal: OtlpSignal) -> Vec<u8> {
    match signal {
        OtlpSignal::Logs => collector_logs::ExportLogsServiceResponse::default().encode_to_vec(),
        OtlpSignal::Traces => {
            collector_trace::ExportTraceServiceResponse::default().encode_to_vec()
        }
        OtlpSignal::Metrics => {
            collector_metrics::ExportMetricsServiceResponse::default().encode_to_vec()
        }
    }
}

/// The success body for one signal, OTLP/JSON-encoded.
///
/// The empty object is the protobuf-JSON rendering of a message with no set
/// fields, which is exactly what an empty `partial_success` means.
#[must_use]
pub const fn success_json(_signal: OtlpSignal) -> &'static str {
    "{}"
}

#[cfg(test)]
mod tests {
    use super::{success_json, success_protobuf};
    use crate::error::OtlpSignal;
    use crate::proto::{collector_logs, collector_metrics, collector_trace};
    use prost::Message as _;

    #[test]
    fn partial_success_is_the_empty_message_on_every_success_path() {
        for signal in OtlpSignal::ALL {
            let bytes = success_protobuf(*signal);
            assert!(
                bytes.is_empty(),
                "`{}` success body must encode to zero bytes",
                signal.as_str()
            );
            assert_eq!(success_json(*signal), "{}");
        }

        let logs = collector_logs::ExportLogsServiceResponse::decode(
            success_protobuf(OtlpSignal::Logs).as_slice(),
        )
        .expect("decodes");
        assert!(logs.partial_success.is_none());

        let traces = collector_trace::ExportTraceServiceResponse::decode(
            success_protobuf(OtlpSignal::Traces).as_slice(),
        )
        .expect("decodes");
        assert!(traces.partial_success.is_none());

        let metrics = collector_metrics::ExportMetricsServiceResponse::decode(
            success_protobuf(OtlpSignal::Metrics).as_slice(),
        )
        .expect("decodes");
        assert!(metrics.partial_success.is_none());
    }
}
