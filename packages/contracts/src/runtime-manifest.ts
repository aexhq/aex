import type { ProviderName } from "./submission.js";

/**
 * Runtime manifest: the per-session description of where
 * aex places things inside the agent container, plus the merged
 * env-var bag delivered via `RUNTIME.env` / `RUNTIME.json`.
 *
 * The hosted API computes a manifest at startSessionRecord-response time
 * from the validated submission via {@link buildRuntimeManifest} and
 * echoes it on the wire as
 * `SessionRecord.runtimeManifest`, so caller code (anyone rendering catalog markdown
 * pre-submission, or resolving aex's in-container path strings) doesn't
 * have to guess.
 * The managed runtime materialises the actual `RUNTIME.env` / `RUNTIME.json`
 * files in-container from the same envVars inputs, so the
 * SDK-side view and the in-container view describe the same layout.
 *
 * Manifest values are derived, never persisted separately — the source
 * of truth for the customer half remains `submission.environment.envVars`
 * on the session row; the aex half is constant for a given SDK version.
 */

/**
 * The in-container paths the agent and skill code reference at
 * runtime. All fields are absolute, all reflect Anthropic Managed
 * Agents' session-mount rebase rule (every `mount_path` lands under
 * `/mnt/session/uploads/` regardless of the leading slash).
 *
 * `skillsRoot` is the location of Skills API-registered bundles, which
 * the runtime auto-discovers under `/workspace/skills/<name>/` —
 * empirically a separate root from the session-resource mounts, NOT
 * under `/mnt/session/uploads/`.
 */
export interface RuntimeManifest {
  readonly provider: ProviderName | string;
  /** Where skill bundles auto-discover in the managed runner. */
  readonly skillsRoot: string;
  /** Parent dir of File mounts: `<filesRoot>/<f_id>/<rel-path>`. */
  readonly filesRoot: string;
  /** Parent dir of non-SKILL.md asset mounts: `<assetsRoot>/<skl_id>/<rel-path>`. */
  readonly assetsRoot: string;
  /** Absolute path of the in-container aex starttime bridge (invoke via `bun`). */
  readonly aexCli: string;
  /** Absolute path of the in-container aex starttime index. */
  readonly indexJson: string;
  /** Absolute path of the always-mounted aex starttime contract README. */
  readonly readme: string;
  /** Absolute path of the machine-readable manifest mirror. */
  readonly runtimeJson: string;
  /** Absolute path of the POSIX-shell-sourceable runtime env file. */
  readonly runtimeEnv: string;
  /**
   * Merged env-var bag: aex-set runtime keys (with reserved
   * `AEX_` prefix) plus customer-supplied `environment.envVars`.
   * Both `RUNTIME.env` and `RUNTIME.json` are rendered from this
   * exact map; `__KEY__` substitution in agent-facing markdown
   * resolves against this exact map.
   */
  readonly envVars: Readonly<Record<string, string>>;
  /**
   * The resolved in-container mount DIRECTORY of each submitted `File`, so the
   * caller can learn where a handed file landed. `mountPath` is the validated
   * directory the archive unzipped into (the SDK default is `/workspace`); a
   * single file lands at `<mountPath>/<realFilename>`, a folder lands its entries
   * under `<mountPath>/`. Empty when the session carried no files.
   */
  readonly mountedFiles: readonly MountedFileManifest[];
}

/** One submitted `File`'s resolved mount directory (surfaced on the Session record). */
export interface MountedFileManifest {
  /** The file's storage slug (`FileRef.name`). */
  readonly name: string;
  /** Absolute container directory the file unzipped into (defaults to `/workspace`). */
  readonly mountPath: string;
}

/**
 * Managed-runner container paths. Kept here so the BFF, hosted API, and
 * in-container bridge render identical values; runtime bootstrap constants are
 * validated against these by a regression test
 * (`packages/contracts/test/runtime-manifest.test.ts`).
 */
const RUNTIME_PATHS = Object.freeze({
  skillsRoot: "/workspace/skills",
  filesRoot: "/mnt/session/uploads/aex/files",
  assetsRoot: "/mnt/session/uploads/aex/assets",
  aexCli: "/mnt/session/uploads/aex/aex",
  indexJson: "/mnt/session/uploads/aex/index.json",
  readme: "/mnt/session/uploads/aex/SKILLS.md",
  runtimeJson: "/mnt/session/uploads/aex/RUNTIME.json",
  runtimeEnv: "/mnt/session/uploads/aex/RUNTIME.env"
} as const);

export function runtimePaths(): typeof RUNTIME_PATHS {
  return RUNTIME_PATHS;
}

export interface BuildRuntimeManifestInput {
  readonly provider: ProviderName | string;
  /**
   * Customer-supplied `environment.envVars` from the validated
   * submission. Keys with the reserved `AEX_` prefix are
   * filtered out defensively — the strict submission parser already
   * rejects them, but defence-in-depth means a malformed snapshot
   * (or a future bypass) can't poison the manifest.
   */
  readonly customerEnvVars?: Readonly<Record<string, string>> | undefined;
  /**
   * The validated submission's `files` refs. Each resolves to one
   * {@link MountedFileManifest} entry surfacing the resolved mount directory
   * (the SDK default is `/workspace`). Absent / non-array ⇒ no mounted files.
   */
  readonly files?: readonly { readonly name?: unknown; readonly mountPath?: unknown }[] | undefined;
}

/**
 * Reserved env-var prefix for aex-set runtime keys. Mirrors the
 * constant in `submission.ts`; duplicated here so this module is
 * self-contained and can be tree-shaken by SDK consumers that don't
 * need the submission parser.
 */
const AEX_PREFIX = "AEX_";

/**
 * Default mount DIRECTORY for a `File` with no explicit `mountPath`. Mirrors
 * `DEFAULT_FILE_MOUNT_PATH` in `session-config.ts`; duplicated here so this module
 * stays self-contained (tree-shakeable) — the same reason {@link AEX_PREFIX}
 * is inlined rather than imported from the submission parser.
 */
const DEFAULT_FILE_MOUNT_PATH = "/workspace";

/**
 * Build the runtime manifest for a single submission. Pure function:
 * same input → same output → safe to call from the BFF response path
 * and from the hosted bootstrap path with identical results.
 */
export function buildRuntimeManifest(input: BuildRuntimeManifestInput): RuntimeManifest {
  const paths = runtimePaths();
  const aexEnvVars: Record<string, string> = {
    AEX_PROVIDER: input.provider,
    AEX_CLI: paths.aexCli,
    AEX_SKILLS_ROOT: paths.skillsRoot,
    AEX_FILES_ROOT: paths.filesRoot,
    AEX_ASSETS_ROOT: paths.assetsRoot,
    AEX_INDEX_JSON: paths.indexJson,
    AEX_README: paths.readme,
    AEX_RUNTIME_JSON: paths.runtimeJson,
    AEX_RUNTIME_ENV: paths.runtimeEnv
  };
  const customerEnvVars: Record<string, string> = {};
  for (const [key, value] of Object.entries(input.customerEnvVars ?? {})) {
    if (key.startsWith(AEX_PREFIX)) {
      // Defensive filter; the strict parser rejects this at submit
      // time. If a stored snapshot somehow carries a reserved key
      // we drop it rather than letting it shadow our value.
      continue;
    }
    customerEnvVars[key] = value;
  }
  const envVars = Object.freeze({ ...aexEnvVars, ...customerEnvVars });
  const mountedFiles: MountedFileManifest[] = [];
  for (const f of Array.isArray(input.files) ? input.files : []) {
    if (typeof f.name !== "string" || f.name.length === 0) continue;
    const mountPath =
      typeof f.mountPath === "string" && f.mountPath.length > 0 ? f.mountPath : DEFAULT_FILE_MOUNT_PATH;
    mountedFiles.push(Object.freeze({ name: f.name, mountPath }));
  }
  return Object.freeze({
    provider: input.provider,
    skillsRoot: paths.skillsRoot,
    filesRoot: paths.filesRoot,
    assetsRoot: paths.assetsRoot,
    aexCli: paths.aexCli,
    indexJson: paths.indexJson,
    readme: paths.readme,
    runtimeJson: paths.runtimeJson,
    runtimeEnv: paths.runtimeEnv,
    envVars,
    mountedFiles: Object.freeze(mountedFiles)
  });
}
