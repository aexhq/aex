//! Compiles the vendored `opentelemetry-proto` files with `prost-build`.
//!
//! The `.proto` tree under `proto/` is a checked-in vendor copy of one exact
//! upstream release; nothing here fetches, and no `opentelemetry` SDK crate is
//! involved. `src/proto_pin.rs`'s test digests the same file list, so a silent
//! upstream bump is a red test rather than a wire change.
//!
//! `gRPC` service definitions are deliberately *not* generated: OTLP reaches AEX
//! over HTTP/protobuf and `tonic` is absent from the workspace on purpose.

use std::path::{Path, PathBuf};

/// Every vendored file, in the order `proto_pin` digests them.
const PROTO_FILES: &[&str] = &[
    "opentelemetry/proto/collector/logs/v1/logs_service.proto",
    "opentelemetry/proto/collector/metrics/v1/metrics_service.proto",
    "opentelemetry/proto/collector/trace/v1/trace_service.proto",
    "opentelemetry/proto/common/v1/common.proto",
    "opentelemetry/proto/logs/v1/logs.proto",
    "opentelemetry/proto/metrics/v1/metrics.proto",
    "opentelemetry/proto/resource/v1/resource.proto",
    "opentelemetry/proto/trace/v1/trace.proto",
];

fn main() {
    let root =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets the manifest dir"))
            .join("proto");
    let inputs: Vec<PathBuf> = PROTO_FILES.iter().map(|name| root.join(name)).collect();
    for input in &inputs {
        println!("cargo:rerun-if-changed={}", display(input));
    }
    println!("cargo:rerun-if-changed=build.rs");

    let mut config = prost_build::Config::new();
    config.protoc_executable(
        protoc_bin_vendored::protoc_bin_path().expect("the vendored protoc supports this target"),
    );
    // Bytes rather than `Vec<u8>` for the identifier fields the decoder reads on
    // every record; the rest stay owned so a decoded batch is `'static`.
    config.bytes([
        ".opentelemetry.proto.trace.v1.Span.trace_id",
        ".opentelemetry.proto.trace.v1.Span.span_id",
        ".opentelemetry.proto.trace.v1.Span.parent_span_id",
        ".opentelemetry.proto.logs.v1.LogRecord.trace_id",
        ".opentelemetry.proto.logs.v1.LogRecord.span_id",
        ".opentelemetry.proto.metrics.v1.Exemplar.trace_id",
        ".opentelemetry.proto.metrics.v1.Exemplar.span_id",
    ]);
    config.disable_comments(["."]);
    // One include file so the generated cross-package `super::` paths resolve.
    config.include_file("_otlp.rs");
    config
        .compile_protos(&inputs, std::slice::from_ref(&root))
        .expect("the vendored opentelemetry-proto tree compiles");
}

fn display(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
