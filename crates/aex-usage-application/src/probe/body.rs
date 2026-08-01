//! The response-body wrapper every egress boundary uses.
//!
//! [`CountingBody`] wraps an [`http_body::Body`] and counts the bytes it
//! actually yields, then closes its [`EgressCounter`] exactly once when the body
//! ends. Two properties fall out of the shape rather than out of discipline:
//!
//! - **Counted after compression.** The wrapper sits on the encoded body, so it
//!   sees the bytes as written, not the bytes before an encoder touched them.
//! - **One crossing, one fact.** The counter is moved out on the first end, so
//!   a second end — a body polled again after completion, or a wrapper cloned by
//!   mistake — has nothing left to close.
//!
//! A body that ends by error or is dropped mid-stream closes nothing. That is
//! deliberate: an interrupted crossing has no authoritative byte count, and
//! inventing one would bill an unmeasured quantity. The drop is counted so the
//! gap is visible.
//!
//! Pin projection goes through `pin-project-lite`, because `unsafe_code` is
//! forbidden workspace-wide and a hand-rolled projection would need it.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};

use bytes::Buf;
use http_body::{Body, Frame, SizeHint};
use pin_project_lite::pin_project;

use super::clock::WallClock;
use super::egress::EgressCounter;
use super::to_timestamp;

pin_project! {
    /// Counts the encoded bytes one body writes and closes its crossing once.
    #[derive(Debug)]
    pub struct CountingBody<B> {
        #[pin]
        inner: B,
        counter: Option<EgressCounter>,
        clock: Arc<dyn WallClock>,
        failures: Arc<AtomicU64>,
    }
}

impl<B> CountingBody<B> {
    /// Wraps a body in a counter for one crossing.
    ///
    /// `failures` counts closes that could not produce a fact — a saturated sink
    /// or an unrepresentable instant. It is never silently zero, because a
    /// crossing that was measured and then lost is money.
    pub fn new(
        inner: B,
        counter: EgressCounter,
        clock: Arc<dyn WallClock>,
        failures: Arc<AtomicU64>,
    ) -> Self {
        Self {
            inner,
            counter: Some(counter),
            clock,
            failures,
        }
    }

    /// Bytes counted so far, or `None` once the crossing has been closed.
    #[must_use]
    pub fn counted(&self) -> Option<u64> {
        self.counter.as_ref().map(EgressCounter::counted)
    }

    /// Whether the crossing has already been closed.
    #[must_use]
    pub const fn is_closed(&self) -> bool {
        self.counter.is_none()
    }
}

impl<B> Body for CountingBody<B>
where
    B: Body,
    B::Data: Buf,
{
    type Data = B::Data;
    type Error = B::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.project();
        let polled = this.inner.poll_frame(context);
        match &polled {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(data) = frame.data_ref()
                    && let Some(counter) = this.counter.as_mut()
                {
                    let written = data.remaining() as u64;
                    if counter.count(written).is_err() {
                        this.failures.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            Poll::Ready(None) => {
                // The body ended cleanly, which is the only condition that
                // produces a fact. `take` is what makes a second end a no-op.
                if let Some(counter) = this.counter.take() {
                    match to_timestamp(this.clock.now()) {
                        Ok(at) => {
                            if counter.close(at).is_err() {
                                this.failures.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                        Err(_) => {
                            this.failures.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            }
            // An errored or pending body closes nothing: an interrupted
            // crossing has no authoritative byte count.
            Poll::Ready(Some(Err(_))) | Poll::Pending => {}
        }
        polled
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

#[cfg(test)]
mod tests {
    use super::CountingBody;
    use crate::probe::egress::EgressCounter;
    use crate::probe::sink::{BoundedFactSink, FactSink, OverflowLedger};
    use crate::probe::testing::{FixedClock, probe_context};
    use aex_usage_domain::fact::{Attribution, FactKind};
    use aex_usage_domain::measurement::{BoundaryId, CounterEpoch, Evidence};
    use bytes::Bytes;
    use http_body_util::BodyExt;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn sink() -> Arc<BoundedFactSink> {
        Arc::new(BoundedFactSink::new(16, Arc::new(OverflowLedger::new())).expect("capacity"))
    }

    fn counter(sink: &Arc<BoundedFactSink>, dropped: &Arc<AtomicU64>, seq: u64) -> EgressCounter {
        EgressCounter::open(
            BoundaryId::REGIONAL_HTTP,
            probe_context(),
            Attribution::default(),
            CounterEpoch::new(1),
            seq,
            Arc::clone(sink),
            Arc::clone(dropped),
        )
    }

    fn wrapped(
        chunks: Vec<&'static str>,
        sink: &Arc<BoundedFactSink>,
        dropped: &Arc<AtomicU64>,
        failures: &Arc<AtomicU64>,
    ) -> CountingBody<http_body_util::StreamBody<BodyStream>> {
        let stream = BodyStream {
            chunks: chunks
                .into_iter()
                .map(|chunk| Bytes::from_static(chunk.as_bytes()))
                .collect(),
        };
        CountingBody::new(
            http_body_util::StreamBody::new(stream),
            counter(sink, dropped, 1),
            FixedClock::new(1_000),
            Arc::clone(failures),
        )
    }

    /// A minimal frame stream, so the test drives the body rather than a client.
    struct BodyStream {
        chunks: Vec<Bytes>,
    }

    impl futures::Stream for BodyStream {
        type Item = Result<http_body::Frame<Bytes>, std::convert::Infallible>;

        fn poll_next(
            mut self: std::pin::Pin<&mut Self>,
            _context: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Option<Self::Item>> {
            std::task::Poll::Ready(if self.chunks.is_empty() {
                None
            } else {
                Some(Ok(http_body::Frame::data(self.chunks.remove(0))))
            })
        }
    }

    #[tokio::test]
    async fn a_completed_body_produces_exactly_one_fact_for_the_bytes_it_wrote() {
        let sink = sink();
        let dropped = Arc::new(AtomicU64::new(0));
        let failures = Arc::new(AtomicU64::new(0));
        let body = wrapped(vec!["hello", " ", "world"], &sink, &dropped, &failures);

        let collected = body.collect().await.expect("collects").to_bytes();
        assert_eq!(collected.len(), 11);

        let batch = sink.take_batch(16);
        assert_eq!(batch.len(), 1, "one crossing is one fact");
        let FactKind::Measured(measurement) = &batch[0].kind else {
            panic!("a closed crossing is a measured fact");
        };
        assert_eq!(measurement.quantity().get(), 11);
        match measurement.evidence() {
            Evidence::BoundaryCounter { end, .. } => assert_eq!(*end, 11),
            other => panic!("expected a counter, got {other:?}"),
        }
        assert_eq!(dropped.load(Ordering::Relaxed), 0);
        assert_eq!(failures.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn an_empty_body_still_closes_its_crossing_at_zero() {
        let sink = sink();
        let dropped = Arc::new(AtomicU64::new(0));
        let failures = Arc::new(AtomicU64::new(0));
        let body = wrapped(vec![], &sink, &dropped, &failures);

        body.collect().await.expect("collects");
        let batch = sink.take_batch(16);
        assert_eq!(batch.len(), 1);
        assert_eq!(
            batch[0]
                .kind
                .measurement()
                .expect("measured")
                .quantity()
                .get(),
            0,
            "a zero-byte crossing is a measured zero, not a missing fact"
        );
        assert_eq!(dropped.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn a_body_dropped_mid_stream_emits_nothing_and_is_counted() {
        let sink = sink();
        let dropped = Arc::new(AtomicU64::new(0));
        let failures = Arc::new(AtomicU64::new(0));
        {
            let mut body = wrapped(vec!["one", "two"], &sink, &dropped, &failures);
            let first = std::pin::Pin::new(&mut body).frame().await;
            assert!(first.is_some(), "the first frame arrived");
            assert_eq!(body.counted(), Some(3));
            assert!(!body.is_closed());
        }
        assert!(
            sink.take_batch(16).is_empty(),
            "an interrupted crossing has no authoritative byte count"
        );
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn a_body_polled_past_its_end_does_not_close_a_second_time() {
        let sink = sink();
        let dropped = Arc::new(AtomicU64::new(0));
        let failures = Arc::new(AtomicU64::new(0));
        let mut body = wrapped(vec!["abc"], &sink, &dropped, &failures);

        while std::pin::Pin::new(&mut body).frame().await.is_some() {}
        assert!(body.is_closed());
        assert!(body.counted().is_none());
        // Poll again past the end.
        assert!(std::pin::Pin::new(&mut body).frame().await.is_none());

        assert_eq!(
            sink.take_batch(16).len(),
            1,
            "closing twice would be two facts for one crossing"
        );
        assert_eq!(dropped.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn a_saturated_sink_at_close_is_counted_never_silently_dropped() {
        let full = Arc::new(BoundedFactSink::new(1, Arc::new(OverflowLedger::new())).expect("cap"));
        let dropped = Arc::new(AtomicU64::new(0));
        let failures = Arc::new(AtomicU64::new(0));
        full.offer(crate::probe::testing::draft_fixture(9))
            .expect("fills the sink");

        let body = CountingBody::new(
            http_body_util::Full::new(Bytes::from_static(b"payload")),
            counter(&full, &dropped, 2),
            FixedClock::new(1_000),
            Arc::clone(&failures),
        );
        let _ = body.collect().await.expect("collects");

        assert_eq!(
            failures.load(Ordering::Relaxed),
            1,
            "a measured crossing that could not be recorded has to be visible"
        );
    }

    #[tokio::test]
    async fn the_wrapper_passes_the_body_through_unchanged() {
        let sink = sink();
        let dropped = Arc::new(AtomicU64::new(0));
        let failures = Arc::new(AtomicU64::new(0));
        let body = wrapped(vec!["a", "bc", "def"], &sink, &dropped, &failures);
        let bytes = body.collect().await.expect("collects").to_bytes();
        assert_eq!(&bytes[..], b"abcdef");
    }
}
