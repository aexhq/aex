export const REDACTED = "[REDACTED]";

export interface ChildSessionOutput {
  readonly exitCode: number;
  readonly stdout: string;
  readonly stderr: string;
}

export function redactKnownValues(text: string, knownValues: readonly (string | undefined)[]): string {
  let out = text;
  for (const value of knownValues) {
    if (value && value.length >= 4) {
      out = out.split(value).join(REDACTED);
    }
  }
  return out;
}

export function formatChildFailure(
  label: string,
  child: ChildSessionOutput,
  knownSecrets: readonly (string | undefined)[]
): string {
  return `${label} exited ${child.exitCode}\n--- stdout ---\n${redactKnownValues(
    child.stdout,
    knownSecrets
  )}\n--- stderr ---\n${redactKnownValues(child.stderr, knownSecrets)}`;
}

export const LIVE_REQUEST_TRACE_SOURCE = `
const __aexTraceKnownSecrets = [process.env.AEX_API_KEY, process.env.DEEPSEEK_KEY]
  .filter((value) => typeof value === "string" && value.length >= 4);

function redactTraceText(value) {
  let out = String(value);
  for (const secret of __aexTraceKnownSecrets) {
    out = out.split(secret).join("[REDACTED]");
  }
  return out;
}

function tracePath(pathOrUrl) {
  try {
    return new URL(pathOrUrl, "https://aex.invalid").pathname;
  } catch {
    const path = String(pathOrUrl).split("?")[0];
    return path.length > 0 ? path : "/";
  }
}

function writeRequestTrace(method, pathOrUrl, status, startedMs) {
  const verb = String(method || "GET").toUpperCase();
  const statusText = typeof status === "number" ? String(status) : "ERR";
  const durationMs = Math.max(0, Date.now() - startedMs);
  const line = "[aex-user-test] " + verb + " " + tracePath(pathOrUrl) + " -> " + statusText + " " + durationMs + "ms";
  process.stderr.write(redactTraceText(line) + "\\n");
}

function aexDebug(line) {
  process.stderr.write(redactTraceText(line) + "\\n");
}
`;
