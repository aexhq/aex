// CLI ↔ SDK capability parity manifest.
//
// The CLI is implemented against the public contracts layer, but it must still
// expose the same user-facing capability surface as the SDK: every public SDK
// capability (an `Aex` client method, a session-start option, a files-accessor
// method) must surface as a registered CLI verb / flag / sub-verb. This
// manifest enumerates that surface and maps each capability onto its CLI
// counterpart; the conformance `cli-sdk-parity` test then asserts, against the
// REAL SDK reflection + the REAL CLI verb registry, that the mapping is
// complete and honest — turning "matches the SDK's public surface" from a
// comment into a CI-enforced invariant.
//
// A capability the CLI intentionally does NOT surface (a purely programmatic
// convenience, or an option with no scriptable analogue) maps to
// {@link CLI_PARITY_NOT_SURFACED} so adding a NEW capability still forces a
// conscious, reviewed manifest entry rather than silently drifting.

/** A session-option / accessor method the CLI deliberately does not expose. */
export const CLI_PARITY_NOT_SURFACED = "(not surfaced in CLI)";

/** The bare `aex files <id>` list form (no sub-verb token). */
export const CLI_PARITY_BARE_LIST = "(bare: aex files <id>)";

/** The dynamic per-provider `--<provider>-api-key` flag family. */
export const CLI_PARITY_PROVIDER_KEY_FLAG = "--<provider>-api-key";

/**
 * The instance-field control-plane clients and their `aex <verb>` counterparts.
 * `client.orgs` / `client.workspaces` / `client.keys` are INSTANCE FIELDS set in
 * the `Aex` constructor — they never touch the prototype that {@link
 * CliSdkParityManifest.aexMethods} reflects — so their method surface is pinned
 * separately here.
 */
export interface ControlPlaneSubverbManifest {
  /** `OrgsClient` method → `aex orgs` sub-verb. */
  readonly orgs: Readonly<Record<string, string>>;
  /** `WorkspacesClient` method → `aex workspaces` sub-verb. */
  readonly workspaces: Readonly<Record<string, string>>;
  /** `KeysClient` method → `aex keys` sub-verb. */
  readonly keys: Readonly<Record<string, string>>;
}

/** The CLI verb that surfaces each control-plane client, keyed by manifest section. */
export const CONTROL_PLANE_VERB_BY_CLIENT = {
  orgs: "orgs",
  workspaces: "workspaces",
  keys: "keys"
} as const satisfies Readonly<Record<keyof ControlPlaneSubverbManifest, string>>;

export interface CliSdkParityManifest {
  /** Public `Aex` client method → the CLI verb that surfaces it. */
  readonly aexMethods: Readonly<Record<string, string>>;
  /** `SessionStartOptions` key → the CLI `start` flag that supplies it. */
  readonly sessionOptionFlags: Readonly<Record<string, string>>;
  /** `SessionFiles` accessor method → the CLI `files` sub-verb that surfaces it. */
  readonly filesSubverbs: Readonly<Record<string, string>>;
  /**
   * Control-plane instance-field client method → CLI sub-verb. Because
   * `client.orgs` / `client.workspaces` / `client.keys` are instance fields, the
   * prototype-reflection gate that pins {@link aexMethods} can never see their
   * methods. This section makes the CI gate go RED if an SDK control-plane method
   * is added without the matching `aex <verb> <sub-verb>` (or vice-versa).
   */
  readonly controlPlaneSubverbs: ControlPlaneSubverbManifest;
}

export const CLI_SDK_PARITY_MANIFEST: CliSdkParityManifest = {
  aexMethods: {
    start: "start",
    whoami: "whoami",
    billing: "billing",
    billingCheckout: "billing",
    billingPortal: "billing",
    billingLedger: "billing",
    webhookSigningSecret: "webhooks"
  },
  sessionOptionFlags: {
    provider: "--provider",
    model: "--model",
    system: "--system",
    assets: "--file|--skill|--tool|--instructions",
    mcpServers: "--mcp",
    metadata: "--metadata",
    idempotencyKey: "--idempotency-key",
    apiKeys: CLI_PARITY_PROVIDER_KEY_FLAG,
    environment: "--config",
    runtime: "--runtime|--runtime-size",
    overrides: "--session-timeout",
    webhook: "--webhook",
    message: "--prompt",
    messageIdempotencyKey: "--idempotency-key",
    // Programmatic-only / no scriptable analogue:
    fileCapture: CLI_PARITY_NOT_SURFACED,
    builtinTools: CLI_PARITY_NOT_SURFACED,
    outputMode: CLI_PARITY_NOT_SURFACED,
    responseFormat: CLI_PARITY_NOT_SURFACED,
    approvalGate: CLI_PARITY_NOT_SURFACED,
    deleteAfter: CLI_PARITY_NOT_SURFACED,
    stream: CLI_PARITY_NOT_SURFACED
  },
  filesSubverbs: {
    list: CLI_PARITY_BARE_LIST,
    read: "read",
    download: "download",
    link: "link",
    find: "find",
    // Covered by the surfaced methods above (conveniences over list/find/link):
    last: CLI_PARITY_NOT_SURFACED,
    first: CLI_PARITY_NOT_SURFACED,
    findOne: CLI_PARITY_NOT_SURFACED,
    fetch: CLI_PARITY_NOT_SURFACED
  },
  controlPlaneSubverbs: {
    orgs: {
      create: "create",
      list: "list",
      members: "members",
      invite: "invite"
    },
    workspaces: {
      create: "create",
      list: "list",
      delete: "delete"
    },
    keys: {
      create: "create",
      list: "list",
      delete: "delete"
    }
  }
};
