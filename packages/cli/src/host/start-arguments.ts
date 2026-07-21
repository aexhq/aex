import {
  parseSessionTimeout,
  PROVIDERS,
  RUNTIME_KINDS,
  RUNTIME_SIZES,
  type ProviderName
} from "@aexhq/contracts";
import {
  collectRepeated,
  collectRepeatedKv,
  collectRepeatedKvList,
  parseDuration,
  suggest,
  takeBooleanFlag,
  takeOptionFlag
} from "./common.js";

export interface StartArguments {
  readonly explicitProvider?: ProviderName;
  readonly providerApiKeys: Partial<Record<ProviderName, string>>;
  readonly idempotencyKey?: string;
  readonly webhookUrl?: string;
  readonly runtimeSize?: string;
  readonly runtimeKind?: string;
  readonly sessionTimeout?: string;
  readonly follow: boolean;
  readonly followTimeoutMs: number | null;
  readonly configPath?: string;
  readonly model?: string;
  readonly system?: string;
  readonly prompts: readonly string[];
  readonly skills: readonly string[];
  readonly tools: readonly string[];
  readonly instructions: readonly string[];
  readonly files: readonly string[];
  readonly mcpEntries: Readonly<Record<string, string>>;
  readonly mcpAuthEntries: ReadonlyArray<readonly [string, string]>;
  readonly metadataEntries: Readonly<Record<string, string>>;
}

export type StartArgumentsResult =
  | { readonly ok: true; readonly value: StartArguments }
  | { readonly ok: false; readonly error: string };

/** Parse and validate only the flags owned by `aex start`. */
export function parseStartArguments(argv: readonly string[]): StartArgumentsResult {
  const state: { rest: readonly string[]; error: string | null } = { rest: argv, error: null };
  const option = (flag: string): string | undefined => {
    if (state.error) return undefined;
    const parsed = takeOptionFlag(state.rest, flag);
    state.rest = parsed.remaining;
    state.error = parsed.error;
    return parsed.value;
  };
  const repeated = (flag: string): readonly string[] => {
    if (state.error) return [];
    const parsed = collectRepeated(state.rest, flag);
    state.rest = parsed.remaining;
    state.error = parsed.error;
    return parsed.values;
  };
  const repeatedKv = (flag: string): Readonly<Record<string, string>> => {
    if (state.error) return {};
    const parsed = collectRepeatedKv(state.rest, flag);
    state.rest = parsed.remaining;
    state.error = parsed.error;
    return parsed.entries;
  };
  const repeatedKvList = (flag: string): ReadonlyArray<readonly [string, string]> => {
    if (state.error) return [];
    const parsed = collectRepeatedKvList(state.rest, flag);
    state.rest = parsed.remaining;
    state.error = parsed.error;
    return parsed.entries;
  };
  const failed = (): StartArgumentsResult | undefined =>
    state.error ? { ok: false, error: state.error } : undefined;

  const providerValue = option("--provider");
  if (state.error) return failed()!;
  let explicitProvider: ProviderName | undefined;
  if (providerValue !== undefined) {
    if (!(PROVIDERS as readonly string[]).includes(providerValue)) {
      const hint = suggest(providerValue, PROVIDERS);
      return {
        ok: false,
        error:
          `--provider must be one of: ${PROVIDERS.join(", ")} (got: ${providerValue})` +
          `${hint ? `; did you mean "${hint}"?` : ""}`
      };
    }
    explicitProvider = providerValue as ProviderName;
  }

  const providerApiKeys: Partial<Record<ProviderName, string>> = {};
  for (const provider of PROVIDERS) {
    const value = option(`--${provider}-api-key`);
    if (state.error) return failed()!;
    if (value !== undefined) providerApiKeys[provider] = value;
  }

  const idempotencyKey = option("--idempotency-key");
  const webhookUrl = option("--webhook");
  const runtimeSize = option("--runtime-size");
  if (state.error) return failed()!;
  if (runtimeSize && !(RUNTIME_SIZES as readonly string[]).includes(runtimeSize)) {
    const hint = suggest(runtimeSize, RUNTIME_SIZES);
    return {
      ok: false,
      error: `--runtime-size must be one of: ${RUNTIME_SIZES.join(", ")}${hint ? `; did you mean "${hint}"?` : ""}`
    };
  }

  const runtimeKind = option("--runtime");
  if (state.error) return failed()!;
  if (runtimeKind && !(RUNTIME_KINDS as readonly string[]).includes(runtimeKind)) {
    const hint = suggest(runtimeKind, RUNTIME_KINDS);
    return {
      ok: false,
      error: `--runtime must be one of: ${RUNTIME_KINDS.join(", ")}${hint ? `; did you mean "${hint}"?` : ""}`
    };
  }

  const sessionTimeout = option("--session-timeout");
  if (state.error) return failed()!;
  if (sessionTimeout) {
    try {
      parseSessionTimeout(sessionTimeout);
    } catch (err) {
      return { ok: false, error: `--session-timeout: ${(err as Error).message}` };
    }
  }

  const followFlag = takeBooleanFlag(state.rest, "--follow");
  state.rest = followFlag.remaining;
  const timeout = option("--timeout");
  if (state.error) return failed()!;
  let followTimeoutMs: number | null = null;
  if (timeout !== undefined) {
    const parsed = parseDuration(timeout);
    if (parsed.error) return { ok: false, error: `--timeout: ${parsed.error}` };
    followTimeoutMs = parsed.ms;
  }

  const configPath = option("--config");
  const model = option("--model");
  const system = option("--system");
  const prompts = repeated("--prompt");
  const skills = repeated("--skill");
  const tools = repeated("--tool");
  const instructions = repeated("--instructions");
  const files = repeated("--file");
  const mcpEntries = repeatedKv("--mcp");
  const mcpAuthEntries = repeatedKvList("--mcp-auth");
  const metadataEntries = repeatedKv("--metadata");
  const removedEndpointValues = repeated("--proxy-endpoint");
  const proxyAuth = repeatedKv("--proxy-auth");
  if (state.error) return failed()!;
  if (removedEndpointValues.length > 0 || Object.keys(proxyAuth).length > 0) {
    return {
      ok: false,
      error: "--proxy-endpoint and --proxy-auth are no longer supported; make HTTP calls from your code and pass credentials via secrets."
    };
  }

  const positional = state.rest.filter((arg) => !arg.startsWith("--"));
  const unknownFlags = state.rest.filter((arg) => arg.startsWith("--"));
  if (unknownFlags.length > 0) return { ok: false, error: `unknown flag: ${unknownFlags[0]}` };
  if (positional.length > 0) {
    return { ok: false, error: `aex start takes no positional arguments (got: ${positional.join(" ")})` };
  }

  return {
    ok: true,
    value: {
      ...(explicitProvider ? { explicitProvider } : {}),
      providerApiKeys,
      ...(idempotencyKey !== undefined ? { idempotencyKey } : {}),
      ...(webhookUrl !== undefined ? { webhookUrl } : {}),
      ...(runtimeSize !== undefined ? { runtimeSize } : {}),
      ...(runtimeKind !== undefined ? { runtimeKind } : {}),
      ...(sessionTimeout !== undefined ? { sessionTimeout } : {}),
      follow: followFlag.present,
      followTimeoutMs,
      ...(configPath !== undefined ? { configPath } : {}),
      ...(model !== undefined ? { model } : {}),
      ...(system !== undefined ? { system } : {}),
      prompts,
      skills,
      tools,
      instructions,
      files,
      mcpEntries,
      mcpAuthEntries,
      metadataEntries
    }
  };
}
