//! Evidence for the production exporter: redaction has already happened before
//! anything is rendered, one record is one valid bounded line, one batch is one
//! write, a record past the ceiling is refused by name rather than truncated,
//! and a sink failure is a counted drop.
//!
//! Every assertion about the rendered bytes goes through `serde_json`, which is
//! a test-only dependency. The exporter renders by hand so the ceiling can be
//! enforced without a serialiser in the production graph; holding that rendering
//! to an independent parser is what makes the hand-rolled path safe.

use std::sync::Arc;
use std::time::Duration;

use aex_platform_telemetry::{
    FlushOutcome, Handle, JsonLinesExporter, LineSink, LogStream, Record, Settings, StatsSink,
};
use aex_telemetry_schema::generated::{
    AEX_CUSTOMER_EMAIL, AEX_DEPLOYABLE, AEX_ERROR_CLASS, AEX_ISOLATION_COUNT, AEX_OUTCOME,
    AEX_PLANE, AEX_REGION, AEX_SESSION_ID, EVENT_AEX_PROCESS_STARTED,
    METRIC_AEX_OPERATION_DURATION,
};
use parking_lot::Mutex;

/// Whether a test sink accepts writes or refuses them.
///
/// Named rather than a boolean so a test reads as the fault it injects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SinkFault {
    None,
    EveryWrite,
}

/// A sink that keeps every block it is handed, or refuses every write.
struct CapturingSink {
    blocks: Mutex<Vec<Vec<u8>>>,
    fault: SinkFault,
}

impl CapturingSink {
    fn new(fault: SinkFault) -> Self {
        Self {
            blocks: Mutex::new(Vec::new()),
            fault,
        }
    }

    fn blocks(&self) -> Vec<Vec<u8>> {
        self.blocks.lock().clone()
    }

    /// Every written line, with the block boundaries removed.
    fn lines(&self) -> Vec<String> {
        self.blocks()
            .iter()
            .flat_map(|block| lines_of(block))
            .collect()
    }
}

impl LineSink for CapturingSink {
    fn write_block(&self, block: &[u8]) -> std::io::Result<()> {
        match self.fault {
            SinkFault::EveryWrite => Err(std::io::Error::other("this sink refuses every write")),
            SinkFault::None => {
                self.blocks.lock().push(block.to_vec());
                Ok(())
            }
        }
    }
}

fn lines_of(block: &[u8]) -> Vec<String> {
    let text = String::from_utf8(block.to_vec()).expect("a rendered block is valid UTF-8");
    assert!(
        text.ends_with('\n'),
        "every block ends with a line terminator"
    );
    text.lines().map(ToOwned::to_owned).collect()
}

fn parse(line: &str) -> serde_json::Value {
    serde_json::from_str(line).unwrap_or_else(|error| panic!("`{line}` is not valid JSON: {error}"))
}

fn started(plane: &str) -> Record {
    Record::event(EVENT_AEX_PROCESS_STARTED).with(AEX_PLANE, plane.to_owned())
}

fn installed(sink: &Arc<CapturingSink>, settings: &Settings) -> (Handle, Arc<JsonLinesExporter>) {
    let exporter = Arc::new(JsonLinesExporter::new(Arc::clone(sink) as Arc<dyn LineSink>));
    let handle = Handle::install(settings, Some(exporter.clone()));
    (handle, exporter)
}

#[test]
fn a_non_public_attribute_is_gone_before_a_line_is_rendered() {
    let sink = Arc::new(CapturingSink::new(SinkFault::None));
    let (handle, _) = installed(&sink, &Settings::lambda());
    handle.emit(
        Record::event(EVENT_AEX_PROCESS_STARTED)
            .with(AEX_PLANE, "dev")
            .with(AEX_REGION, "eu-west-1")
            .with(AEX_DEPLOYABLE, "brain-mux")
            .with(AEX_SESSION_ID, "018f6a0e-0000-7000-8000-000000000000")
            .with(AEX_CUSTOMER_EMAIL, "someone@example.test")
            .with("aex.never.declared", "whatever"),
    );
    assert_eq!(handle.redacted_attributes(), 3);
    assert_eq!(
        handle.flush(Duration::from_secs(5)),
        FlushOutcome::Drained { exported: 1 }
    );

    let lines = sink.lines();
    assert_eq!(lines.len(), 1);
    let rendered = &lines[0];
    for absent in [
        AEX_SESSION_ID,
        AEX_CUSTOMER_EMAIL,
        "aex.never.declared",
        "018f6a0e-0000-7000-8000-000000000000",
        "someone@example.test",
        "whatever",
    ] {
        assert!(
            !rendered.contains(absent),
            "`{absent}` reached the rendered line: {rendered}"
        );
    }
    let attributes = &parse(rendered)["attributes"];
    assert_eq!(attributes["aex.plane"], "dev");
    assert_eq!(attributes["aex.region"], "eu-west-1");
    assert_eq!(attributes["aex.deployable"], "brain-mux");
    assert_eq!(
        attributes
            .as_object()
            .expect("attributes is an object")
            .len(),
        3,
        "only the three public attributes survive"
    );
}

#[test]
fn every_exported_record_is_exactly_one_valid_json_line() {
    let sink = Arc::new(CapturingSink::new(SinkFault::None));
    let (handle, exporter) = installed(&sink, &Settings::lambda());
    handle.emit(started("dev"));
    handle.emit(
        Record::span(aex_telemetry_schema::generated::SPAN_AEX_PROVIDER_CALL)
            .with(AEX_OUTCOME, true)
            .with(AEX_ISOLATION_COUNT, 4_i64),
    );
    handle.emit(Record::metric(METRIC_AEX_OPERATION_DURATION, 17));
    assert_eq!(
        handle.flush(Duration::from_secs(5)),
        FlushOutcome::Drained { exported: 3 }
    );

    let lines = sink.lines();
    assert_eq!(lines.len(), 3, "one line per record");
    assert_eq!(exporter.written(), 3);
    assert_eq!(exporter.refused(), 0);

    let event = parse(&lines[0]);
    assert_eq!(event["kind"], "event");
    assert_eq!(event["name"], EVENT_AEX_PROCESS_STARTED);
    assert!(event["value"].is_null(), "a non-metric carries no value");
    assert_eq!(event["attributes"]["aex.plane"], "dev");

    let span = parse(&lines[1]);
    assert_eq!(span["kind"], "span");
    assert_eq!(span["attributes"][AEX_OUTCOME], true);
    assert_eq!(span["attributes"][AEX_ISOLATION_COUNT], 4);

    let metric = parse(&lines[2]);
    assert_eq!(metric["kind"], "metric");
    assert_eq!(metric["name"], METRIC_AEX_OPERATION_DURATION);
    assert_eq!(metric["value"], 17);

    for line in &lines {
        assert!(
            line.len() <= JsonLinesExporter::DEFAULT_LINE_CEILING_BYTES,
            "a written line is inside the ceiling"
        );
    }
}

#[test]
fn text_that_would_break_the_line_is_escaped_rather_than_split() {
    let sink = Arc::new(CapturingSink::new(SinkFault::None));
    let (handle, _) = installed(&sink, &Settings::lambda());
    let hostile = "first\nsecond\ttab \"quoted\" back\\slash \u{1}control ünïcode";
    handle.emit(Record::event(EVENT_AEX_PROCESS_STARTED).with(AEX_ERROR_CLASS, hostile));
    assert_eq!(
        handle.flush(Duration::from_secs(5)),
        FlushOutcome::Drained { exported: 1 }
    );

    let lines = sink.lines();
    assert_eq!(lines.len(), 1, "an embedded newline never splits a record");
    assert_eq!(parse(&lines[0])["attributes"][AEX_ERROR_CLASS], hostile);
}

#[test]
fn one_batch_is_one_locked_write() {
    let sink = Arc::new(CapturingSink::new(SinkFault::None));
    let settings = Settings::lambda()
        .with_queue_capacity(64)
        .with_batch_size(16);
    let (handle, _) = installed(&sink, &settings);
    for _ in 0..64 {
        handle.emit(started("prd"));
    }
    assert_eq!(
        handle.flush(Duration::from_secs(5)),
        FlushOutcome::Drained { exported: 64 }
    );

    let blocks = sink.blocks();
    assert_eq!(blocks.len(), 4, "64 records at a batch size of 16");
    for block in &blocks {
        assert_eq!(lines_of(block).len(), 16, "one write carries a whole batch");
    }
}

#[test]
fn a_record_past_the_line_ceiling_is_refused_and_names_the_ceiling() {
    let sink = Arc::new(CapturingSink::new(SinkFault::None));
    let ceiling = JsonLinesExporter::MINIMUM_LINE_CEILING_BYTES;
    let exporter = Arc::new(
        JsonLinesExporter::with_line_ceiling(Arc::clone(&sink) as Arc<dyn LineSink>, ceiling)
            .expect("the minimum ceiling is accepted"),
    );
    let handle = Handle::install(&Settings::lambda(), Some(exporter.clone()));
    let oversized = "x".repeat(ceiling);
    handle.emit(started("dev"));
    handle.emit(Record::event(EVENT_AEX_PROCESS_STARTED).with(AEX_ERROR_CLASS, oversized.clone()));
    assert_eq!(
        handle.flush(Duration::from_secs(5)),
        FlushOutcome::Drained { exported: 2 }
    );

    let lines = sink.lines();
    assert_eq!(lines.len(), 2, "a refused record still leaves one line");
    assert_eq!(
        parse(&lines[0])["kind"],
        "event",
        "the small record is kept"
    );

    let refusal = parse(&lines[1]);
    assert_eq!(refusal["kind"], "refused");
    assert_eq!(refusal["name"], EVENT_AEX_PROCESS_STARTED);
    assert_eq!(refusal["line_ceiling_bytes"], ceiling);
    assert!(
        refusal["encoded_bytes"]
            .as_u64()
            .expect("the observed size is a number")
            > u64::try_from(ceiling).expect("the ceiling fits a JSON number"),
        "the refusal reports the size that broke the ceiling"
    );
    assert!(
        !lines[1].contains(&oversized),
        "a refused record is never truncated into the log"
    );
    assert!(
        lines[1].len() <= ceiling,
        "the refusal line is itself bounded"
    );
    assert_eq!(exporter.written(), 1);
    assert_eq!(exporter.refused(), 1);
}

#[test]
fn a_line_ceiling_below_the_minimum_is_refused_at_construction() {
    let sink = Arc::new(CapturingSink::new(SinkFault::None)) as Arc<dyn LineSink>;
    let error = JsonLinesExporter::with_line_ceiling(sink, 8)
        .expect_err("a ceiling that refuses every record is a configuration error");
    assert_eq!(error.requested, 8);
    assert_eq!(error.minimum, JsonLinesExporter::MINIMUM_LINE_CEILING_BYTES);
}

#[test]
fn a_sink_failure_is_a_counted_drop_and_never_a_host_failure() {
    let sink = Arc::new(CapturingSink::new(SinkFault::EveryWrite));
    let settings = Settings::lambda()
        .with_queue_capacity(32)
        .with_batch_size(8);
    let (handle, exporter) = installed(&sink, &settings);
    for _ in 0..32 {
        handle.emit(started("dev"));
    }
    assert_eq!(
        handle.flush(Duration::from_secs(5)),
        FlushOutcome::Drained { exported: 0 },
        "an undeliverable batch is discarded, never retried"
    );
    assert_eq!(handle.dropped(), 32);
    assert_eq!(handle.exported(), 0);
    assert_eq!(handle.pending(), 0);
    assert_eq!(
        exporter.written(),
        0,
        "nothing reached the log, so nothing is counted as written"
    );
}

#[test]
fn the_stats_line_is_one_bounded_json_line_and_enqueues_nothing() {
    let sink = Arc::new(CapturingSink::new(SinkFault::None));
    let (handle, exporter) = installed(&sink, &Settings::long_lived());
    handle.emit(started("dev"));
    exporter.publish(&handle.stats());

    assert_eq!(
        handle.pending(),
        1,
        "publishing state never enqueues a record about the queue"
    );
    let lines = sink.lines();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].len() <= JsonLinesExporter::DEFAULT_LINE_CEILING_BYTES);

    let stats = parse(&lines[0]);
    assert_eq!(stats["kind"], "telemetry_stats");
    assert_eq!(stats["pending"], 1);
    assert_eq!(
        stats["queue_capacity"],
        Settings::long_lived().queue_capacity
    );
    assert_eq!(stats["emitted"], 1);
    assert_eq!(stats["accepted"], 1);
    assert_eq!(stats["exported"], 0);
    assert_eq!(stats["dropped"], 0);
    assert_eq!(stats["redacted_attributes"], 0);
    assert_eq!(stats["flush_deadline_exceeded"], 0);
}

#[test]
fn a_process_stream_accepts_a_rendered_block() {
    LogStream::Stderr
        .write_block(b"")
        .expect("a process stream accepts a block");
}
