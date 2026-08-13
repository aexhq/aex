---
title: Spike — rig 0.41.0 streaming, errors, and retry
description: Working prototype against a local mock HTTP server proving how the RigProviderRouter would use rig 0.41.0, and the zero-reconnect verdict that supersedes decision 7.
keywords:
  - rig
  - spike
  - streaming
  - retry
audience: implementation agents and maintainers
status: accepted
last_verified: 2026-08-13
related:
  - references/model-provider-library-simplification-2026-08-13/design-2026-08-13.md
---

# Spike: rig 0.41.0 streaming, errors, and retry

Artifacts: `C:\Users\luowe\AppData\Local\Temp\opencode\spike-rig` (scratch crate,
std-only mock `TcpListener` server with `SO_LINGER=0` RST for the drop probe).
Toolchain `cargo +1.97.1`, clean build, **0 warnings**; aws-lc-rs prebuilt NASM
path worked without env overrides.

## Findings

1. **Client construction.** `ClientBuilder::api_key(impl Into<ApiKey>)`
   (`client/mod.rs:605`) — `&str`/`String` both work via
   `From<S: Into<String>> for BearerAuth`. Own reqwest client injection:
   `http_client(impl HttpClientExt)` (`client/mod.rs:659`). `Client` is
   `Clone` over `Arc<str>`/`Arc<HeaderMap>`/http client; measured clone
   ~48 ns. `build()` without injection constructs a fresh `reqwest::Client`
   each time — always inject one shared client.
2. **Streaming.** No "response started" event; the first `Ok(item)` (first
   text/tool delta) is the earliest signal. Usage arrives only on
   `Final(response)` / `stream.usage()`. Tool calls arrive as
   `ToolCallDelta { id, internal_call_id, content: Name|Delta }` then a
   complete `ToolCall` at `finish_reason: tool_calls`. Aggregated result in
   `stream.choice` (`OneOrMany<AssistantContent>`). DeepSeek request path
   observed as `POST /chat/completions` (no `/v1`).
3. **Error statuses.** Streaming and blocking both yield
   `CompletionError::HttpError(InvalidStatusCodeWithMessage(status, body))`
   with `provider_response_status()` / `provider_response_body()` — verified
   503 and 429 with JSON bodies. Non-2xx is rejected before SSE parsing.
4. **Mid-stream drop = zero reconnect.** Mock sent 2 chunks then RST. Observed:
   2 `Ok` items, 1 `Err(ProviderError(...))`, stream ends. No re-send ever
   reached the mock. The infinite-reconnect machinery
   (`DEFAULT_RETRY { max_retries: None }`, `http_client/retry.rs:74-79`) exists
   in `GenericEventSource` but is **unreachable through the public provider
   path**: the streamer breaks on the first error and calls
   `event_source.close()`. No public API exists to configure it, and none is
   needed.
5. **Zero-reconnect options.** (a) Own `CompletionModel` over public
   `CompletionRequest`/`RawStreaming*` types + `HttpClientExt::send_streaming`
   — feasible, ~250–350 LOC; (b) wrapper — adds nothing; (c) vendor-patch —
   rig is MIT, but unnecessary. **Verdict: none needed.**
6. **Blocking path.** `model.completion_request(prompt).send()` →
   `CompletionResponse { choice, usage, raw_response, message_id }`; same error
   funnel. DeepSeek leaves `message_id: None`.
7. **Dependency facts.** rig-core 0.41.0: reqwest 0.13.4, tokio 1.53.1,
   futures, eventsource-stream 0.2.3, serde, thiserror, tracing. rustls
   resolved via aws-lc-rs 1.18.0 — **`ring` not in the tree** (matches the
   workspace). License MIT. The `rig` facade adds rig-agent + companion
   crates; **rig-core alone suffices** for a raw streaming/error router.
8. **Auth header.** Wire capture shows `Authorization: Bearer <key>`;
   **no `set_sensitive` usage anywhere in rig-core** — accepted relaxation.

## Consequences for the design

- Decision 7's `max_retries = Some(0)` is dropped: rig's provider streams are
  already bounded (one error, then terminate).
- Router-level retry: capture the `CompletionRequest` (Clone+Serialize) before
  dispatch; on 429/503 or transport-drop errors, re-issue through a fresh
  model handle with equal-jitter backoff, ≤3 attempts, cancellable.
  ~50–80 LOC.
- Mid-stream drops surface as `ProviderError` with **no status** — retry
  policy treats them uniformly (bounded, then terminal).
- `.stream().await == Ok` does not mean the connection succeeded; HTTP errors
  arrive as stream items on first poll.

## Caveats

- DeepSeek default base in rig is `https://api.deepseek.com` (no `/v1`) — do
  not "correct" it.
- Keep the workspace on one reqwest version (0.13.4) so rig shares the graph.
