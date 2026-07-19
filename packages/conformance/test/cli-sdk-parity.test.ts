import { describe, expect, it } from "vitest";
import { Aex } from "../../sdk/dist/index.js";
import type { KeysClient, OrgsClient, SessionFiles, SessionStartOptions, WorkspacesClient } from "../../sdk/dist/index.js";
import { CLI_VERB_NAMES, FILES_SUBVERBS, START_FLAGS as SESSION_FLAGS, findVerbSpec } from "../../cli/dist/index.js";
import {
  CLI_PARITY_BARE_LIST,
  CLI_PARITY_NOT_SURFACED,
  CLI_PARITY_PROVIDER_KEY_FLAG,
  CLI_SDK_PARITY_MANIFEST,
  CONTROL_PLANE_VERB_BY_CLIENT
} from "../src/index.js";

/**
 * COMPILE-TIME class-killer: these typed copies of the manifest maps fail to
 * BUILD the moment a `SessionStartOptions` / `SessionFiles` key is added,
 * removed, or renamed without a corresponding manifest entry — so a new SDK
 * capability can never silently break CLI parity. The runtime `toEqual`
 * below pins the (loosely-typed) published manifest to these typed copies so
 * the two can't drift.
 */
const SESSION_OPTION_COVERAGE = {
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
  fileCapture: CLI_PARITY_NOT_SURFACED,
  builtinTools: CLI_PARITY_NOT_SURFACED,
  outputMode: CLI_PARITY_NOT_SURFACED,
  responseFormat: CLI_PARITY_NOT_SURFACED,
  approvalGate: CLI_PARITY_NOT_SURFACED,
  deleteAfter: CLI_PARITY_NOT_SURFACED,
  stream: CLI_PARITY_NOT_SURFACED
} satisfies Record<keyof SessionStartOptions, string>;

const FILES_COVERAGE = {
  list: CLI_PARITY_BARE_LIST,
  read: "read",
  download: "download",
  link: "link",
  find: "find",
  last: CLI_PARITY_NOT_SURFACED,
  first: CLI_PARITY_NOT_SURFACED,
  findOne: CLI_PARITY_NOT_SURFACED,
  fetch: CLI_PARITY_NOT_SURFACED
} satisfies Record<keyof SessionFiles, string>;

/**
 * COMPILE-TIME class-killers for the CONTROL-PLANE surface. `client.orgs` /
 * `client.workspaces` / `client.keys` are INSTANCE FIELDS, so they never appear
 * on `Aex.prototype` and {@link publicAexMethods} can't see them — these typed
 * copies fail to BUILD the moment an `OrgsClient` / `WorkspacesClient` /
 * `KeysClient` method is added, removed, or renamed without a manifest entry.
 */
const ORGS_COVERAGE = {
  create: "create",
  list: "list",
  members: "members",
  invite: "invite"
} satisfies Record<keyof OrgsClient, string>;

const WORKSPACES_COVERAGE = {
  create: "create",
  list: "list",
  delete: "delete"
} satisfies Record<keyof WorkspacesClient, string>;

const KEYS_COVERAGE = {
  create: "create",
  list: "list",
  delete: "delete"
} satisfies Record<keyof KeysClient, string>;

/** Public `Aex` client methods, reflected off the prototype (drop ctor + internals). */
function publicAexMethods(): string[] {
  return Object.getOwnPropertyNames(Aex.prototype)
    .filter((name) => name !== "constructor" && !name.startsWith("_"))
    .sort();
}

/** Public methods of a control-plane client, reflected off its instance prototype. */
function publicInstanceMethods(instance: object): string[] {
  return Object.getOwnPropertyNames(Object.getPrototypeOf(instance))
    .filter((name) => name !== "constructor" && !name.startsWith("_"))
    .sort();
}

// A REAL client instance so the control-plane surface is reflected off the live
// objects the SDK hands users, not a hand-written list. A non-self-describing
// opaque key keeps the explicit baseUrl and makes ZERO network calls in the ctor.
const controlPlaneClient = new Aex({ apiKey: "aexu_parity_probe", baseUrl: "https://parity.invalid" });

const CONTROL_PLANE_SURFACES = [
  { section: "orgs", instance: controlPlaneClient.orgs, coverage: ORGS_COVERAGE },
  { section: "workspaces", instance: controlPlaneClient.workspaces, coverage: WORKSPACES_COVERAGE },
  { section: "keys", instance: controlPlaneClient.keys, coverage: KEYS_COVERAGE }
] as const satisfies ReadonlyArray<{
  readonly section: keyof typeof CONTROL_PLANE_VERB_BY_CLIENT;
  readonly instance: object;
  readonly coverage: Readonly<Record<string, string>>;
}>;

describe("CLI ↔ SDK parity manifest", () => {
  it("covers every SessionStartOptions key (compile-time) and pins the published manifest", () => {
    expect(CLI_SDK_PARITY_MANIFEST.sessionOptionFlags).toEqual(SESSION_OPTION_COVERAGE);
  });

  it("covers every SessionFiles accessor method (compile-time) and pins the published manifest", () => {
    expect(CLI_SDK_PARITY_MANIFEST.filesSubverbs).toEqual(FILES_COVERAGE);
  });

  it("accounts for every public Aex client method — a new SDK method fails until mapped", () => {
    const reflected = publicAexMethods();
    const manifest = Object.keys(CLI_SDK_PARITY_MANIFEST.aexMethods).sort();
    expect(manifest).toEqual(reflected);
  });

  it("maps every Aex method to a REGISTERED CLI verb", () => {
    for (const [method, verb] of Object.entries(CLI_SDK_PARITY_MANIFEST.aexMethods)) {
      expect(CLI_VERB_NAMES, `Aex.${method} → CLI verb "${verb}"`).toContain(verb);
      expect(findVerbSpec(verb), `verb "${verb}" spec`).toBeDefined();
    }
  });

  it("maps every surfaced session-option to a REGISTERED `aex start` flag", () => {
    for (const [key, flag] of Object.entries(CLI_SDK_PARITY_MANIFEST.sessionOptionFlags)) {
      if (flag === CLI_PARITY_NOT_SURFACED || flag === CLI_PARITY_PROVIDER_KEY_FLAG) continue;
      for (const mappedFlag of flag.split("|")) {
        expect(SESSION_FLAGS, `session-option "${key}" -> flag "${mappedFlag}"`).toContain(mappedFlag);
      }
    }
  });

  it("maps every surfaced files accessor to a REGISTERED `aex files` sub-verb", () => {
    for (const [key, sub] of Object.entries(CLI_SDK_PARITY_MANIFEST.filesSubverbs)) {
      if (sub === CLI_PARITY_NOT_SURFACED || sub === CLI_PARITY_BARE_LIST) continue;
      expect(FILES_SUBVERBS, `files.${key} → sub-verb "${sub}"`).toContain(sub);
    }
  });

  it("every registered `aex files` sub-verb is reachable from an SDK accessor method", () => {
    const surfaced = new Set(Object.values(CLI_SDK_PARITY_MANIFEST.filesSubverbs));
    for (const sub of FILES_SUBVERBS) {
      expect(surfaced, `sub-verb "${sub}" has an SDK accessor`).toContain(sub);
    }
  });

  it("the `start` and `files` verbs declare the parity-mapped flags/sub-verbs", () => {
    const start = findVerbSpec("start");
    expect(start?.flags).toEqual(SESSION_FLAGS);
    const files = findVerbSpec("files");
    expect(files?.subverbs).toEqual([...FILES_SUBVERBS]);
  });

  // ── CONTROL-PLANE surface (instance-field clients) ────────────────────────
  // `client.orgs` / `client.workspaces` / `client.keys` are INSTANCE FIELDS, so
  // the `Aex.prototype` reflection above never sees them. These cases pin the
  // published manifest to the typed coverage copies (compile-time) AND reflect
  // the live client instances against the registered CLI sub-verbs, so the gate
  // goes RED on drift in either direction:
  //   • an SDK control-plane method added without a manifest entry / CLI sub-verb
  //     fails "reflected methods === manifest" (and the `satisfies` won't build);
  //   • a CLI sub-verb added without an SDK method fails the reverse check.
  it("pins the published control-plane manifest to the typed coverage copies", () => {
    expect(CLI_SDK_PARITY_MANIFEST.controlPlaneSubverbs.orgs).toEqual(ORGS_COVERAGE);
    expect(CLI_SDK_PARITY_MANIFEST.controlPlaneSubverbs.workspaces).toEqual(WORKSPACES_COVERAGE);
    expect(CLI_SDK_PARITY_MANIFEST.controlPlaneSubverbs.keys).toEqual(KEYS_COVERAGE);
  });

  for (const { section, instance, coverage } of CONTROL_PLANE_SURFACES) {
    const verb = CONTROL_PLANE_VERB_BY_CLIENT[section];

    it(`accounts for every public ${section} client method — a new method fails until mapped`, () => {
      const reflected = publicInstanceMethods(instance);
      const manifest = Object.keys(CLI_SDK_PARITY_MANIFEST.controlPlaneSubverbs[section]).sort();
      expect(manifest).toEqual(reflected);
      // The manifest section and the typed coverage copy must agree too.
      expect(Object.keys(coverage).sort()).toEqual(reflected);
    });

    it(`maps every ${section} method to a REGISTERED \`aex ${verb}\` sub-verb`, () => {
      expect(CLI_VERB_NAMES, `control-plane verb "${verb}"`).toContain(verb);
      const spec = findVerbSpec(verb);
      expect(spec, `verb "${verb}" spec`).toBeDefined();
      const subverbs = spec?.subverbs ?? [];
      for (const [method, sub] of Object.entries(CLI_SDK_PARITY_MANIFEST.controlPlaneSubverbs[section])) {
        expect(subverbs, `${section}.${method} → sub-verb "${sub}"`).toContain(sub);
      }
    });

    it(`every registered \`aex ${verb}\` sub-verb is reachable from a ${section} SDK method`, () => {
      const surfaced = new Set(Object.values(CLI_SDK_PARITY_MANIFEST.controlPlaneSubverbs[section]));
      const subverbs = findVerbSpec(verb)?.subverbs ?? [];
      for (const sub of subverbs) {
        expect(surfaced, `sub-verb "${verb} ${sub}" has an SDK method`).toContain(sub);
      }
    });
  }
});
