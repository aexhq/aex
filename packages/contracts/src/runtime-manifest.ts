/**
 * Runtime manifest: the per-run, per-provider description of where
 * antpath places things inside the agent container, plus the merged
 * env-var bag delivered via `RUNTIME.env` / `RUNTIME.json`.
 *
 * The hosted API computes a manifest at submitRun-response time
 * (from the validated submission + the chosen provider) via
 * {@link buildRuntimeManifest} and echoes it on the wire as
 * `Run.runtimeManifest`, so caller code (anyone rendering catalog markdown
 * pre-submission, or resolving antpath's in-container path strings) doesn't
 * have to guess.
 * The managed runtime materialises the actual `RUNTIME.env` / `RUNTIME.json`
 * files in-container from the same provider + envVars inputs, so the
 * SDK-side view and the in-container view describe the same layout.
 *
 * Manifest values are derived, never persisted separately — the source
 * of truth for the customer half remains `submission.environment.envVars`
 * on the run row; the antpath half is constant for a given
 * provider+SDK-version pair.
 */

/**
 * Set of providers whose runtime contract antpath models. Today only
 * `"anthropic"` ships; the field is on the manifest so forward-compat
 * consumers can branch on it without us having to silently change
 * what `runtimeManifest` means when we add a second provider.
 */
export type RuntimeProvider = "anthropic";

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
  readonly provider: RuntimeProvider;
  /** Where Skills-API-registered bundles auto-discover (Anthropic). */
  readonly skillsRoot: string;
  /** Parent dir of File mounts: `<filesRoot>/<f_id>/<rel-path>`. */
  readonly filesRoot: string;
  /** Parent dir of non-SKILL.md asset mounts: `<assetsRoot>/<skl_id>/<rel-path>`. */
  readonly assetsRoot: string;
  /** Absolute path the agent writes output files into for capture. */
  readonly outputsRoot: string;
  /** Absolute path of the in-container antpath runtime bridge (invoke via `node`). */
  readonly antpathCli: string;
  /** Absolute path of the per-run proxy-endpoints manifest. */
  readonly indexJson: string;
  /** Absolute path of the always-mounted antpath runtime contract README. */
  readonly readme: string;
  /** Absolute path of the machine-readable manifest mirror. */
  readonly runtimeJson: string;
  /** Absolute path of the POSIX-shell-sourceable runtime env file. */
  readonly runtimeEnv: string;
  /**
   * Merged env-var bag: antpath-set runtime keys (with reserved
   * `ANTPATH_` prefix) plus customer-supplied `environment.envVars`.
   * Both `RUNTIME.env` and `RUNTIME.json` are rendered from this
   * exact map; `__KEY__` substitution in agent-facing markdown
   * resolves against this exact map.
   */
  readonly envVars: Readonly<Record<string, string>>;
}

/**
 * Managed-runner container paths. Kept here so the BFF, worker, and
 * in-container bridge render identical values; runtime bootstrap constants are
 * validated against these by a regression test
 * (`packages/contracts/test/runtime-manifest.test.ts`).
 */
const ANTHROPIC_PATHS = Object.freeze({
  skillsRoot: "/workspace/skills",
  filesRoot: "/mnt/session/uploads/antpath/files",
  assetsRoot: "/mnt/session/uploads/antpath/assets",
  outputsRoot: "/mnt/session/outputs",
  antpathCli: "/mnt/session/uploads/antpath/antpath",
  indexJson: "/mnt/session/uploads/antpath/index.json",
  readme: "/mnt/session/uploads/antpath/SKILLS.md",
  runtimeJson: "/mnt/session/uploads/antpath/RUNTIME.json",
  runtimeEnv: "/mnt/session/uploads/antpath/RUNTIME.env"
} as const);

/**
 * Container paths exposed for a given provider. Today only
 * `"anthropic"` is recognised; calling with anything else throws so
 * forward-compat surfaces the missing provider entry instead of
 * silently emitting Anthropic paths.
 */
export function runtimePathsFor(provider: RuntimeProvider): typeof ANTHROPIC_PATHS {
  if (provider === "anthropic") {
    return ANTHROPIC_PATHS;
  }
  throw new Error(`Unknown runtime provider: ${provider as string}`);
}

export interface BuildRuntimeManifestInput {
  readonly provider: RuntimeProvider;
  /**
   * Customer-supplied `environment.envVars` from the validated
   * submission. Keys with the reserved `ANTPATH_` prefix are
   * filtered out defensively — the strict submission parser already
   * rejects them, but defence-in-depth means a malformed snapshot
   * (or a future bypass) can't poison the manifest.
   */
  readonly customerEnvVars?: Readonly<Record<string, string>> | undefined;
}

/**
 * Reserved env-var prefix for antpath-set runtime keys. Mirrors the
 * constant in `submission.ts`; duplicated here so this module is
 * self-contained and can be tree-shaken by SDK consumers that don't
 * need the submission parser.
 */
const ANTPATH_PREFIX = "ANTPATH_";

/**
 * Build the runtime manifest for a single submission. Pure function:
 * same input → same output → safe to call from the BFF response path
 * and from the worker bootstrap path with identical results.
 */
export function buildRuntimeManifest(input: BuildRuntimeManifestInput): RuntimeManifest {
  const paths = runtimePathsFor(input.provider);
  const antpathEnvVars: Record<string, string> = {
    ANTPATH_PROVIDER: input.provider,
    ANTPATH_CLI: paths.antpathCli,
    ANTPATH_OUTPUTS: paths.outputsRoot,
    ANTPATH_SKILLS_ROOT: paths.skillsRoot,
    ANTPATH_FILES_ROOT: paths.filesRoot,
    ANTPATH_ASSETS_ROOT: paths.assetsRoot,
    ANTPATH_INDEX_JSON: paths.indexJson,
    ANTPATH_README: paths.readme,
    ANTPATH_RUNTIME_JSON: paths.runtimeJson,
    ANTPATH_RUNTIME_ENV: paths.runtimeEnv
  };
  const customerEnvVars: Record<string, string> = {};
  for (const [key, value] of Object.entries(input.customerEnvVars ?? {})) {
    if (key.startsWith(ANTPATH_PREFIX)) {
      // Defensive filter; the strict parser rejects this at submit
      // time. If a stored snapshot somehow carries a reserved key
      // we drop it rather than letting it shadow our value.
      continue;
    }
    customerEnvVars[key] = value;
  }
  const envVars = Object.freeze({ ...antpathEnvVars, ...customerEnvVars });
  return Object.freeze({
    provider: input.provider,
    skillsRoot: paths.skillsRoot,
    filesRoot: paths.filesRoot,
    assetsRoot: paths.assetsRoot,
    outputsRoot: paths.outputsRoot,
    antpathCli: paths.antpathCli,
    indexJson: paths.indexJson,
    readme: paths.readme,
    runtimeJson: paths.runtimeJson,
    runtimeEnv: paths.runtimeEnv,
    envVars
  });
}
