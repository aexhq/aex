/**
 * Workspace session list:
 *   - `aex sessions [--limit N] [--since ISO]` — GET /api/sessions (newest first).
 *
 * Prints the canonical page as JSON (`{ sessions, nextCursor? }`), matching the
 * SDK's `aex.sessions.list(...)` response.
 */
import { operations } from "@aexhq/contracts/internal";
import type { CliIO } from "../internal.js";
import {
  type CliExitCode,
  SUCCESS,
  USAGE_ERR,
  describeApiError,
  emitJsonError,
  makeHttpClient,
  resolveCommonHostFlags,
  refuseInsideManagedSession,
  takeOptionFlag
} from "./common.js";

function parseLimit(io: CliIO, raw: string | undefined): { ok: true; limit: number | undefined } | { ok: false } {
  if (raw === undefined) return { ok: true, limit: undefined };
  const limit = Number(raw);
  if (!Number.isInteger(limit) || limit < 1) {
    io.stderr(`--limit must be a positive integer (got: ${raw})\n`);
    return { ok: false };
  }
  return { ok: true, limit };
}

export async function executeSessionsCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  if (await refuseInsideManagedSession(io, "sessions")) return USAGE_ERR;

  const common = await resolveCommonHostFlags(io, argv);
  if (!common.ok) {
    io.stderr(`${common.reason}\n`);
    return USAGE_ERR;
  }
  const limitFlag = takeOptionFlag(common.rest, "--limit");
  const sinceFlag = takeOptionFlag(limitFlag.remaining, "--since");
  const optionError = limitFlag.error ?? sinceFlag.error;
  if (optionError) { io.stderr(`${optionError}\n`); return USAGE_ERR; }
  const { value: rawLimit } = limitFlag;
  const { value: since, remaining } = sinceFlag;
  if (remaining.length > 0) {
    io.stderr(`unexpected arguments: ${remaining.join(" ")}\n`);
    io.stderr("usage: aex sessions [--limit N] [--since ISO-8601] [common flags]\n");
    return USAGE_ERR;
  }
  const parsed = parseLimit(io, rawLimit);
  if (!parsed.ok) return USAGE_ERR;
  if (since !== undefined && Number.isNaN(Date.parse(since))) {
    io.stderr(`--since must be an ISO-8601 timestamp (got: ${since})\n`);
    return USAGE_ERR;
  }

  const http = makeHttpClient(io, common.flags);
  try {
    const page = await operations.listSessions(http, {
      ...(parsed.limit !== undefined ? { limit: parsed.limit } : {}),
      ...(since !== undefined ? { since } : {})
    });
    io.stdout(JSON.stringify(page) + "\n");
    return SUCCESS;
  } catch (err) {
    const d = describeApiError(err);
    return emitJsonError(io, "sessions_failed", d.message, {
      ...(d.status !== undefined ? { status: d.status } : {}),
      ...(d.remedy ? { remedy: d.remedy } : {})
    });
  }
}
