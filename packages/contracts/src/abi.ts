/* eslint-disable */
/**
 * GENERATED from contracts/abi/v1/abi.json by packages/contracts/scripts/gen.mjs (tools/gen.sh). DO NOT EDIT.
 */

/**
 * Lower-case hex SHA-256.
 *
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "Sha256Hex".
 */
export type Sha256Hex = string;
/**
 * Brain-minted, unique per request on one connection. Responses echo it.
 *
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "RequestId".
 */
export type RequestId = string;
/**
 * Brain-minted, durable, unique per tool-call attempt. Not the provider's tool_call id.
 *
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "OperationId".
 */
export type OperationId = string;
/**
 * Brain-minted lane identifier. "0" is the root lane and always exists.
 *
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "LaneId".
 */
export type LaneId = string;
/**
 * Groups the parallel tool calls of one assistant message.
 *
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "BatchId".
 */
export type BatchId = string;
/**
 * Minted by the hand per incarnation (per boot with fresh state). Lanes, operations and spill files are scoped to a generation.
 *
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "GenerationId".
 */
export type GenerationId = string;
/**
 * Changes on every VM boot, including resume from a released state. Same generation_id + same boot_id on reconnect means in-guest state survived.
 *
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "BootId".
 */
export type BootId = string;
/**
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "SessionId".
 */
export type SessionId = string;
/**
 * Identifies one workspace sync manifest. Brain-minted.
 *
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "ManifestId".
 */
export type ManifestId = string;
/**
 * Identifies one sync pack object (tar+zstd of changed files). Brain-minted.
 *
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "PackId".
 */
export type PackId = string;
/**
 * Milliseconds on the guest's monotonic clock. Jumps across a restore; compare only within one boot_id.
 *
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "MonotonicMs".
 */
export type MonotonicMs = number;
/**
 * Milliseconds since the Unix epoch, guest wall clock.
 *
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "WallMs".
 */
export type WallMs = number;
/**
 * Every operation owns two byte streams. Typed tools write their human-readable result to stdout and diagnostics to stderr.
 *
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "Stream".
 */
export type Stream = "stdout" | "stderr";
/**
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "LaneMode".
 */
export type LaneMode = "persistent" | "ephemeral";
/**
 * completed = the tool ran to its end (a non-zero exit_code is still `completed`; the failure is data for the model). failed = the hand could not run it. interrupted = outcome unknown; never replayed.
 *
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "Outcome".
 */
export type Outcome = "completed" | "failed" | "cancelled" | "deadline_exceeded" | "interrupted";
/**
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "ErrorCode".
 */
export type ErrorCode =
  | "malformed_request"
  | "protocol_unsupported"
  | "unauthorized"
  | "tool_manifest_mismatch"
  | "generation_mismatch"
  | "fence_stale"
  | "tool_not_found"
  | "tool_input_invalid"
  | "tool_output_invalid"
  | "lane_gone"
  | "lane_busy"
  | "lane_limit_exceeded"
  | "lane_not_closable"
  | "operation_not_found"
  | "operation_idempotency_conflict"
  | "operation_output_evicted"
  | "path_not_found"
  | "path_outside_scope"
  | "checksum_mismatch"
  | "transfer_failed"
  | "too_large"
  | "resource_exhausted"
  | "restore_failed"
  | "internal";
/**
 * missing = the hand knows the id but lost its record (treated as interrupted).
 *
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "OperationStatus".
 */
export type OperationStatus = "running" | "terminal" | "missing";
/**
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "PutSource".
 */
export type PutSource =
  | {
      kind: "url";
      /**
       * Short-lived presigned GET minted by the trusted side.
       */
      get_url: string;
      bytes: number;
      /**
       * Lower-case hex SHA-256.
       */
      sha256: string;
    }
  | {
      kind: "inline";
      /**
       * Small files only (limits.max_inline_put_bytes decoded).
       */
      data_base64: string;
    };
/**
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "PersistSource".
 */
export type PersistSource =
  | {
      kind: "path";
      /**
       * Absolute guest path inside the sync scope, resolved through symlinks.
       */
      path: string;
    }
  | {
      kind: "operation_stream";
      operation_id: OperationId;
      stream: Stream;
    };
/**
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "SyncReason".
 */
export type SyncReason = "turn_end" | "interval" | "before_release" | "explicit";
/**
 * One brain->hand call, tagged by `op`; `args` is that op's request type.
 *
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "RequestCall".
 */
export type RequestCall =
  | {
      op: "hello";
      args: HelloRequest;
    }
  | {
      op: "start";
      args: StartRequest;
    }
  | {
      op: "poll";
      args: PollRequest;
    }
  | {
      op: "cancel";
      args: CancelRequest;
    }
  | {
      op: "release";
      args: ReleaseRequest;
    }
  | {
      op: "lane_close";
      args: LaneCloseRequest;
    }
  | {
      op: "put";
      args: PutRequest;
    }
  | {
      op: "persist";
      args: PersistRequest;
    }
  | {
      op: "sync";
      args: SyncRequest;
    };
/**
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "ResponseResult".
 */
export type ResponseResult =
  | {
      status: "ok";
      reply: Reply;
    }
  | {
      status: "error";
      error: AbiError1;
    };
/**
 * One successful reply, tagged by `op`; `body` is that op's response type.
 *
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "Reply".
 */
export type Reply =
  | {
      op: "hello";
      body: HelloResponse;
    }
  | {
      op: "start";
      body: StartResponse;
    }
  | {
      op: "poll";
      body: PollResponse;
    }
  | {
      op: "cancel";
      body: CancelResponse;
    }
  | {
      op: "release";
      body: ReleaseResponse;
    }
  | {
      op: "lane_close";
      body: LaneCloseResponse;
    }
  | {
      op: "put";
      body: PutResponse;
    }
  | {
      op: "persist";
      body: PersistResponse;
    }
  | {
      op: "sync";
      body: SyncResponse;
    };
/**
 * Hand -> brain frame, tagged by `kind`.
 *
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "HandFrame".
 */
export type HandFrame =
  | {
      kind: "response";
      frame: Response;
    }
  | {
      kind: "hand_status";
      frame: HandStatusEvent;
    };
/**
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "SyncEntry".
 */
export type SyncEntry =
  | {
      kind: "file";
      /**
       * Absolute guest path.
       */
      path: string;
      size: number;
      mtime_ns: number;
      mode: number;
      sha256: Sha256Hex;
      /**
       * Identifies one sync pack object (tar+zstd of changed files). Brain-minted.
       */
      pack_id: string;
    }
  | {
      kind: "symlink";
      path: string;
      target: string;
    }
  | {
      kind: "dir";
      path: string;
      mode: number;
    };

/**
 * Wire contract between the brain (LLM harness) and a hand (tool executor in a microVM). One multiplexed WebSocket per hand carries JSON text frames: the brain sends `Request` frames, the hand sends `HandFrame` frames (responses correlated by `id`, plus unsolicited `hand_status` events). Major version must match; unknown fields are ignored. Semantics live in contracts/abi/v1/README.md.
 */
export interface AexBrainHandABIV1 {
  [k: string]: unknown | undefined;
}
/**
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "ProtocolVersion".
 */
export interface ProtocolVersion {
  /**
   * Must match exactly between brain and hand.
   */
  major: number;
  /**
   * Informational. Additive changes only.
   */
  minor: number;
}
/**
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "Cursor".
 */
export interface Cursor {
  stream: Stream;
  /**
   * Byte offset into the full stream as produced (not as retained).
   */
  offset: number;
}
/**
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "OutputSlice".
 */
export interface OutputSlice {
  stream: Stream;
  /**
   * Byte offset of the first byte of `data_base64` in the full stream.
   */
  offset: number;
  /**
   * Standard base64 (RFC 4648 §4, with padding). Bounded by `max_bytes` of the request and `limits.max_slice_bytes`.
   */
  data_base64: string;
  /**
   * True when this slice reaches the current end of the stream. Only meaningful as 'no more bytes right now'; the stream may still grow while the operation is running.
   */
  eof: boolean;
}
/**
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "StreamInfo".
 */
export interface StreamInfo {
  stream: Stream;
  /**
   * Total bytes the child has written to this stream so far.
   */
  produced_bytes: number;
  /**
   * Bytes before this offset are evicted (bounds.max_retained_bytes). A poll below this offset returns `operation_output_evicted`; absent is not zero.
   */
  retained_from: number;
  /**
   * Guest path of the file holding the retained bytes, readable by the agent's own tools (e.g. `grep`). Deleted on `release`.
   */
  spill_path?: string;
  /**
   * Over the full stream. Present only once the operation is terminal and nothing was evicted.
   */
  sha256?: string;
}
/**
 * Per-operation limits enforced by the hand. Missing fields take `limits.default_bounds`.
 *
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "Bounds".
 */
export interface Bounds {
  /**
   * Relative deadline on the guest monotonic clock. null = no hand-side timeout.
   */
  timeout_ms?: number | null;
  /**
   * SIGTERM -> grace -> SIGKILL.
   */
  grace_ms?: number;
  /**
   * Per stream. Beyond this the oldest bytes are evicted from the spill file (tail retention).
   */
  max_retained_bytes?: number;
}
/**
 * The complete bounds a hand applies when `start.bounds` omits a field.
 *
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "EffectiveBounds".
 */
export interface EffectiveBounds {
  timeout_ms: number | null;
  grace_ms: number;
  max_retained_bytes: number;
}
/**
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "LaneRef".
 */
export interface LaneRef {
  id: LaneId;
  mode: LaneMode;
  /**
   * Brain-minted lane identifier. "0" is the root lane and always exists.
   */
  parent?: string;
}
/**
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "LaneSummary".
 */
export interface LaneSummary {
  id: LaneId;
  mode: LaneMode;
  parent?: LaneId;
  state: "live" | "closed";
  /**
   * The attached operation currently holding the lane, if any.
   */
  inflight?: string;
  created_at_monotonic_ms?: MonotonicMs;
}
/**
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "ToolSpec".
 */
export interface ToolSpec {
  name: string;
  description: string;
  /**
   * JSON Schema 2020-12 for `start.input`.
   */
  input_schema: {};
  /**
   * JSON Schema 2020-12 for `TerminalInfo.output`.
   */
  output_schema: {};
  /**
   * Hint for the brain: whether stdout is UTF-8 text. Default text.
   */
  streams?: "text" | "binary";
}
/**
 * The sealed tool set. Digest = SHA-256 over the RFC 8785 (JCS) canonical JSON of this object. Tools are sorted by name.
 *
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "ToolManifest".
 */
export interface ToolManifest {
  version: "1";
  tools: ToolSpec[];
}
/**
 * Declared by the hand in `hello`, not assumed by the brain.
 *
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "Limits".
 */
export interface Limits {
  max_lanes: number;
  max_concurrent_operations: number;
  /**
   * Largest WebSocket text frame either side will send.
   */
  max_frame_bytes: number;
  /**
   * Largest decoded slice returned per stream per response.
   */
  max_slice_bytes: number;
  /**
   * Cap on `wait_ms` for start/poll.
   */
  max_poll_wait_ms: number;
  max_inline_put_bytes: number;
  /**
   * Per persisted item.
   */
  max_persist_bytes: number;
  default_bounds: EffectiveBounds;
}
/**
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "Paths".
 */
export interface Paths {
  /**
   * Default cwd. Synced.
   */
  workspace: string;
  /**
   * $HOME of the agent user. Synced.
   */
  home: string;
  /**
   * Where operation output files live. Not synced.
   */
  spill_dir: string;
}
/**
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "Clock".
 */
export interface Clock {
  monotonic_ms: MonotonicMs;
  wall_ms: WallMs;
}
/**
 * Observation only; customer-controlled data; never billing authority (I9).
 *
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "Usage".
 */
export interface Usage {
  wall_ms: number;
  cpu_ms?: number;
  max_rss_bytes?: number;
}
/**
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "TerminalInfo".
 */
export interface TerminalInfo {
  outcome: Outcome;
  /**
   * Process exit code for command-like tools; null when killed by a signal. Typed tools use 0 = ok, 1 = tool-level failure explained on stderr.
   */
  exit_code?: number | null;
  /**
   * Terminating signal name (e.g. "SIGKILL") when applicable.
   */
  signal?: string | null;
  /**
   * Typed result validated against the tool's `output_schema`. Present when outcome = completed.
   */
  output?: {
    [k: string]: unknown | undefined;
  };
  error?: AbiError;
  ended_at_monotonic_ms: MonotonicMs;
  usage: Usage;
}
/**
 * Present when outcome = failed.
 */
export interface AbiError {
  code: ErrorCode;
  message: string;
  /**
   * True only when repeating the identical request later is safe and may succeed (e.g. resource_exhausted). Never true for opaque work.
   */
  retryable: boolean;
  /**
   * Code-specific structured detail, e.g. {"schema_path": ...} for tool_input_invalid.
   */
  details?: {};
}
/**
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "OperationView".
 */
export interface OperationView {
  operation_id: OperationId;
  tool: string;
  lane_id: LaneId;
  detach: boolean;
  status: OperationStatus;
  started_at_monotonic_ms: MonotonicMs;
  terminal?: TerminalInfo1;
  streams: StreamInfo[];
  /**
   * Echoed verbatim from `start`.
   */
  correlation?: {};
}
/**
 * Present iff status = terminal.
 */
export interface TerminalInfo1 {
  outcome: Outcome;
  /**
   * Process exit code for command-like tools; null when killed by a signal. Typed tools use 0 = ok, 1 = tool-level failure explained on stderr.
   */
  exit_code?: number | null;
  /**
   * Terminating signal name (e.g. "SIGKILL") when applicable.
   */
  signal?: string | null;
  /**
   * Typed result validated against the tool's `output_schema`. Present when outcome = completed.
   */
  output?: {
    [k: string]: unknown | undefined;
  };
  error?: AbiError;
  ended_at_monotonic_ms: MonotonicMs;
  usage: Usage;
}
/**
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "AbiError".
 */
export interface AbiError1 {
  code: ErrorCode;
  message: string;
  /**
   * True only when repeating the identical request later is safe and may succeed (e.g. resource_exhausted). Never true for opaque work.
   */
  retryable: boolean;
  /**
   * Code-specific structured detail, e.g. {"schema_path": ...} for tool_input_invalid.
   */
  details?: {};
}
/**
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "SyncScope".
 */
export interface SyncScope {
  /**
   * Absolute directories included in workspace sync (default: workspace and home).
   *
   * @minItems 1
   */
  roots: [string, ...string[]];
  /**
   * Gitignore-style patterns relative to each root.
   */
  exclude?: string[];
}
/**
 * How a fresh hand re-materialises the workspace. All URLs are short-lived presigned GETs minted by the trusted side; the hand holds no credential (I8).
 *
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "RestoreSource".
 */
export interface RestoreSource {
  manifest_id: ManifestId;
  manifest_get_url: string;
  packs: {
    pack_id: PackId;
    get_url: string;
  }[];
}
/**
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "RestoreReport".
 */
export interface RestoreReport {
  manifest_id: ManifestId;
  files: number;
  bytes: number;
  duration_ms: number;
}
/**
 * First request on every connection. Seals the tool manifest (I1), authenticates the brain to the hand, restores the workspace on a fresh generation, re-attaches on reconnect.
 *
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "HelloRequest".
 */
export interface HelloRequest {
  protocol: ProtocolVersion;
  session_id: SessionId;
  /**
   * Per-session secret the hand was launched with. Mismatch = unauthorized and the connection is closed.
   */
  session_token: string;
  /**
   * Set on reconnect. If the hand's generation differs, the response still succeeds and the brain treats every prior operation as lost.
   */
  expected_generation_id?: string;
  /**
   * The digest sealed at session create. Omitted only on the very first hello of a session, when the brain adopts the hand's manifest. A hand that cannot serve this digest answers tool_manifest_mismatch.
   */
  tool_manifest_digest?: string;
  /**
   * Environment for lane 0 (inherited by every lane). Customer-supplied; never platform credentials.
   */
  env: {
    [k: string]: string | undefined;
  };
  sync: SyncScope;
  restore?: RestoreSource1;
  /**
   * Interval for unsolicited hand_status events.
   */
  heartbeat_ms: number;
}
/**
 * Present when the hand is a fresh generation of a session that has synced before. The hand restores before answering.
 */
export interface RestoreSource1 {
  manifest_id: ManifestId;
  manifest_get_url: string;
  packs: {
    pack_id: PackId;
    get_url: string;
  }[];
}
/**
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "HelloResponse".
 */
export interface HelloResponse {
  protocol: ProtocolVersion;
  generation_id: GenerationId;
  boot_id: BootId;
  tool_manifest_digest: Sha256Hex;
  /**
   * The manifest's tools, sorted by name.
   */
  tools: ToolSpec[];
  /**
   * Every lane the hand knows (live and closed tombstones). Non-empty only on reconnect to a surviving generation.
   */
  lanes: LaneSummary[];
  /**
   * Every non-released operation. Empty on a fresh generation.
   */
  operations: OperationView[];
  limits: Limits;
  paths: Paths;
  clock: Clock;
  restore?: RestoreReport1;
}
/**
 * Present when the request carried `restore`.
 */
export interface RestoreReport1 {
  manifest_id: ManifestId;
  files: number;
  bytes: number;
  duration_ms: number;
}
/**
 * Begin a tool call. Idempotent within a generation by (operation_id, call_hash): a replay returns the existing operation without running it again. Validation precedes side effects.
 *
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "StartRequest".
 */
export interface StartRequest {
  operation_id: OperationId;
  /**
   * Lower-case hex SHA-256.
   */
  call_hash: string;
  batch_id?: BatchId;
  tool: string;
  /**
   * Validated against the manifest input_schema before anything runs.
   */
  input: {
    [k: string]: unknown | undefined;
  };
  lane: LaneRef;
  /**
   * Working directory for this call. Default = paths.workspace. Per call; the lane's cwd is never mutated by the ABI.
   */
  cwd?: string;
  /**
   * true = background job: return as soon as the operation is recorded and spawned; the lane is not held; poll/cancel later. false = attached: holds the lane until terminal.
   */
  detach: boolean;
  /**
   * Attached only: wait up to this long for the operation to become terminal before answering, so short calls cost one round trip. Capped by limits.max_poll_wait_ms. Ignored when detach = true.
   */
  wait_ms: number;
  /**
   * Bytes of stdout/stderr (from offset 0) to include in the response slices. 0 = none.
   */
  max_bytes: number;
  bounds?: Bounds;
  /**
   * Opaque to the hand (e.g. agent_id, provider tool_call id). Echoed on every view.
   */
  correlation?: {};
}
/**
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "StartResponse".
 */
export interface StartResponse {
  view: OperationView;
  slices: OutputSlice[];
  /**
   * True when this start matched an existing (operation_id, call_hash) and nothing new was executed.
   */
  replayed: boolean;
}
/**
 * Status plus incremental output from byte cursors. Byte offsets are the authority: no gaps, no duplicates, up to retention. Waits up to wait_ms for the operation to become terminal or for any cursor to have new bytes.
 *
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "PollRequest".
 */
export interface PollRequest {
  operation_id: OperationId;
  /**
   * Empty = status only.
   */
  cursors: Cursor[];
  /**
   * Total decoded bytes across all returned slices.
   */
  max_bytes: number;
  wait_ms: number;
}
/**
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "PollResponse".
 */
export interface PollResponse {
  view: OperationView;
  slices: OutputSlice[];
}
/**
 * SIGTERM to the operation's process group -> grace_ms -> SIGKILL -> terminal(cancelled). Cancelling a terminal operation is not an error (accepted = false).
 *
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "CancelRequest".
 */
export interface CancelRequest {
  operation_id: OperationId;
  /**
   * Overrides bounds.grace_ms.
   */
  grace_ms?: number;
}
/**
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "CancelResponse".
 */
export interface CancelResponse {
  accepted: boolean;
  view: OperationView1;
}
/**
 * The view at the time of the answer; may still be running during the grace period.
 */
export interface OperationView1 {
  operation_id: OperationId;
  tool: string;
  lane_id: LaneId;
  detach: boolean;
  status: OperationStatus;
  started_at_monotonic_ms: MonotonicMs;
  terminal?: TerminalInfo1;
  streams: StreamInfo[];
  /**
   * Echoed verbatim from `start`.
   */
  correlation?: {};
}
/**
 * The brain has durably committed these results; the hand deletes their spill files and forgets them. After release, poll returns operation_not_found and a replayed start would run again — so release only after commit.
 *
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "ReleaseRequest".
 */
export interface ReleaseRequest {
  /**
   * @minItems 1
   */
  operation_ids: [OperationId, ...OperationId[]];
}
/**
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "ReleaseResponse".
 */
export interface ReleaseResponse {
  released: OperationId[];
  /**
   * Ids the hand did not know (already released or never started). Not an error.
   */
  unknown: OperationId[];
}
/**
 * Destroy a lane and tombstone its id. Cancels its attached in-flight operation. Does not kill detached jobs started from it (they belong to the operation registry). Lane 0 is not closable.
 *
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "LaneCloseRequest".
 */
export interface LaneCloseRequest {
  lane_id: LaneId;
  grace_ms?: number;
}
/**
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "LaneCloseResponse".
 */
export interface LaneCloseResponse {
  closed: boolean;
  cancelled_operations: OperationId[];
}
/**
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "PutFile".
 */
export interface PutFile {
  /**
   * Absolute guest path inside the sync scope; parents are created. Symlinks are resolved before the scope check.
   */
  path: string;
  source: PutSource;
  /**
   * Unix permission bits. Default 0644.
   */
  mode?: number;
}
/**
 * Files in. Bytes never travel as a tool result (I7): the hand downloads from presigned URLs, or accepts small inline payloads.
 *
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "PutRequest".
 */
export interface PutRequest {
  /**
   * @minItems 1
   */
  files: [PutFile, ...PutFile[]];
}
/**
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "PutResponse".
 */
export interface PutResponse {
  written: {
    path: string;
    bytes: number;
    sha256: Sha256Hex;
  }[];
}
/**
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "PersistItem".
 */
export interface PersistItem {
  /**
   * Session-unique artifact name, chosen by the brain/customer.
   */
  name: string;
  source: PersistSource;
  /**
   * Short-lived presigned PUT for exactly this artifact object.
   */
  put_url: string;
  /**
   * Sent as Content-Type. Sniffed if absent.
   */
  media_type?: string;
}
/**
 * Files out. The hand uploads each item to its presigned URL and reports size + digest; the trusted side records the artifact. Bounded by limits.max_persist_bytes per item.
 *
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "PersistRequest".
 */
export interface PersistRequest {
  /**
   * @minItems 1
   */
  items: [PersistItem, ...PersistItem[]];
}
/**
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "PersistResponse".
 */
export interface PersistResponse {
  persisted: {
    name: string;
    bytes: number;
    sha256: Sha256Hex;
    media_type: string;
  }[];
}
/**
 * Workspace sync: diff the sync scope against the last manifest, upload changed files as one pack plus a new manifest. Brain-driven (turn end, every sync_interval, before release/termination). Nothing is uploaded when nothing changed.
 *
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "SyncRequest".
 */
export interface SyncRequest {
  reason: SyncReason;
  /**
   * Identifies one workspace sync manifest. Brain-minted.
   */
  manifest_id: string;
  manifest_put_url: string;
  pack_id: PackId;
  pack_put_url: string;
  /**
   * true = pack every file (compaction), not only changed ones. The new manifest then references a single pack.
   */
  full: boolean;
}
/**
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "SyncResponse".
 */
export interface SyncResponse {
  /**
   * false = nothing differed from the last manifest; no upload happened and manifest_id is the previous one.
   */
  changed: boolean;
  manifest_id: ManifestId;
  files_total: number;
  bytes_total: number;
  files_added: number;
  files_modified: number;
  files_deleted: number;
  /**
   * Compressed pack bytes actually uploaded.
   */
  bytes_uploaded: number;
  /**
   * How many packs the new manifest points at. The brain compacts (full = true) when this grows large.
   */
  packs_referenced: number;
  duration_ms: number;
}
/**
 * Brain -> hand. Every request carries the brain's ownership fence; a fence lower than the highest the hand has accepted is refused with fence_stale (no side effect). generation_id is required on every request except hello; a mismatch is generation_mismatch.
 *
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "Request".
 */
export interface Request {
  id: RequestId;
  fence: number;
  generation_id?: GenerationId;
  call: RequestCall;
}
/**
 * Hand -> brain answer to one Request, correlated by `id`.
 *
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "Response".
 */
export interface Response {
  id: RequestId;
  result: ResponseResult;
}
/**
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "Pressure".
 */
export interface Pressure {
  mem_available_bytes: number;
  swap_used_bytes: number;
  /**
   * PSI memory 'some' avg10, when the guest kernel exposes it.
   */
  psi_some_avg10?: number;
}
/**
 * Hand -> brain, unsolicited: on every idle/busy transition, when a detached job ends, and every heartbeat_ms. The idle signal.
 *
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "HandStatusEvent".
 */
export interface HandStatusEvent {
  generation_id: GenerationId;
  boot_id: BootId;
  /**
   * Per boot, monotonic.
   */
  seq: number;
  at_monotonic_ms: MonotonicMs;
  at_wall_ms: WallMs;
  /**
   * Attached operations not yet terminal.
   */
  inflight: OperationId[];
  /**
   * Detached operations not yet terminal.
   */
  live_jobs: OperationId[];
  lanes_live: number;
  /**
   * Terminal but not yet released.
   */
  operations_retained: number;
  retained_bytes: number;
  /**
   * 0 while inflight or live_jobs is non-empty.
   */
  idle_for_ms: number;
  pressure?: Pressure;
}
/**
 * The object a sync writes to manifest_put_url. Restore = fetch this, then the referenced packs, then extract each file entry from its pack. Empty directories and symlinks are recreated from entries.
 *
 * This interface was referenced by `AexBrainHandABIV1`'s JSON-Schema
 * via the `definition` "SyncManifest".
 */
export interface SyncManifest {
  version: 1;
  manifest_id: ManifestId;
  parent_manifest_id?: ManifestId;
  created_at_wall_ms: WallMs;
  generation_id: GenerationId;
  roots: string[];
  /**
   * POSIX pax tar, zstd-compressed; entry names are absolute guest paths without the leading slash.
   */
  pack_format: "tar+zstd";
  packs: {
    pack_id: PackId;
    /**
     * Compressed size.
     */
    bytes: number;
    sha256?: Sha256Hex;
  }[];
  entries: SyncEntry[];
}
