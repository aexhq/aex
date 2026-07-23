import type { CliIO } from "../internal.js";

/** Cross-command mechanics whose behavior must stay identical across host verbs. */
export function portableBasename(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  const separator = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return separator >= 0 ? trimmed.slice(separator + 1) : trimmed;
}

export function parsePositiveLimit(
  io: CliIO,
  raw: string | undefined
): { ok: true; limit: number | undefined } | { ok: false } {
  if (raw === undefined) return { ok: true, limit: undefined };
  const limit = Number(raw);
  if (!Number.isInteger(limit) || limit < 1) {
    io.stderr(`--limit must be a positive integer (got: ${raw})\n`);
    return { ok: false };
  }
  return { ok: true, limit };
}

/** The single host-timer call {@link pollingDelay} schedules on; injectable for deterministic tests. */
export interface PollingDelayTimer {
  setTimeout(callback: () => void, delayMs: number): unknown;
}

export function pollingDelay(ms: number, timers: PollingDelayTimer = globalThis): Promise<void> {
  return new Promise((resolve) => {
    timers.setTimeout(resolve, ms);
  });
}
