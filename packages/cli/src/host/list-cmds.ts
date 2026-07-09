/**
 * Workspace session list:
 *   - `aex sessions [--limit N] [--since ISO]` — GET /api/sessions (newest first).
 *
 * Prints the page as JSON (`{ sessions, nextCursor? }`), matching the per-session
 * read verbs. `--since` is sent to the server AND enforced client-side
 * on `createdAt` — the currently deployed API accepts but ignores the `since`
 * query on GET /api/sessions, and a silently no-op flag would mislead scripts.
 */
import { operations } from "@aexhq/contracts";
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
  const { value: rawLimit, remaining: afterLimit } = takeOptionFlag(common.rest, "--limit");
  const { value: since, remaining } = takeOptionFlag(afterLimit, "--since");
  if (remaining.length > 0) {
    io.stderr(`unexpected arguments: ${remaining.join(" ")}\n`);
    io.stderr("usage: aex sessions [--limit N] [--since ISO-8601] [common flags]\n");
    return USAGE_ERR;
  }
  const parsed = parseLimit(io, rawLimit);
  if (!parsed.ok) return USAGE_ERR;
  const sinceMs = since !== undefined ? Date.parse(since) : undefined;
  if (sinceMs !== undefined && Number.isNaN(sinceMs)) {
    io.stderr(`--since must be an ISO-8601 timestamp (got: ${since})\n`);
    return USAGE_ERR;
  }

  const http = makeHttpClient(io, common.flags);
  try {
    const page = await operations.listSessionRecords(http, {
      ...(parsed.limit !== undefined ? { limit: parsed.limit } : {}),
      ...(since !== undefined ? { since } : {})
    });
    // Client-side `since` enforcement (see module header): keep only rows whose
    // createdAt parses AND is >= the bound, so the flag filters even against a
    // server that ignores the query param.
    const sessions =
      sinceMs === undefined
        ? page.sessions
        : page.sessions.filter((session) => {
            const created = Date.parse(session.createdAt);
            return !Number.isNaN(created) && created >= sinceMs;
          });
    io.stdout(JSON.stringify({ ...page, sessions }) + "\n");
    return SUCCESS;
  } catch (err) {
    const d = describeApiError(err);
    return emitJsonError(io, "sessions_failed", d.message, {
      ...(d.status !== undefined ? { status: d.status } : {}),
      ...(d.remedy ? { remedy: d.remedy } : {})
    });
  }
}
