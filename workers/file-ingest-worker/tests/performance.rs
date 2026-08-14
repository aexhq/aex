//! Bounded-work performance contract for URL file ingestion.

use std::time::Duration;

use file_ingest_worker::{FETCH_TIMEOUT, MAX_FILE_BYTES};

#[test]
fn one_stream_record_has_finite_body_and_wall_clock_bounds() {
    assert_eq!(MAX_FILE_BYTES, 64 * 1024 * 1024);
    assert_eq!(FETCH_TIMEOUT, Duration::from_secs(120));
}
