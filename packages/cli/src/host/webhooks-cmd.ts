/**
 * `aex webhooks secret` — POST /api/webhook/signing-secret.
 *
 * Reveals the workspace webhook signing secret (creating one on first use) and
 * prints the bare `whsec_<base64>` string to stdout — the value
 * `verifyAexWebhook` takes as `secret`. Running this command IS the explicit
 * reveal request; the secret is never echoed anywhere else (stderr, debug
 * traces, and error envelopes stay secret-free — the transport's `--debug`
 * trace is already redacted to method/path/status).
 *
 * There is no `--rotate`: the hosted API's signing-secret endpoint is
 * reveal-or-create only (a repeat call returns the same value). The flag is
 * recognized and rejected with an actionable message rather than silently
 * revealing, so a caller expecting rotation is never misled.
 */
import { operations } from "@aexhq/contracts/internal";
import type { CliIO } from "../internal.js";
import {
  type CliExitCode,
  SUCCESS,
  USAGE_ERR,
  emitApiError,
  makeHttpClient,
  resolveCommonHostFlags,
  refuseInsideManagedSession,
  takeBooleanFlag
} from "./common.js";

export async function sessionWebhooksCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  if (await refuseInsideManagedSession(io, "webhooks")) return USAGE_ERR;

  const common = await resolveCommonHostFlags(io, argv);
  if (!common.ok) {
    io.stderr(`${common.reason}\n`);
    return USAGE_ERR;
  }

  const [sub, ...rest] = common.rest;
  if (sub !== "secret") {
    io.stderr("usage: aex webhooks secret [common flags]\n");
    return USAGE_ERR;
  }

  const { present: rotate, remaining } = takeBooleanFlag(rest, "--rotate");
  if (rotate) {
    io.stderr(
      "--rotate is not supported: the hosted API reveals (or creates on first use) the workspace " +
        "webhook signing secret but does not rotate it. Run `aex webhooks secret` to reveal the current value.\n"
    );
    return USAGE_ERR;
  }
  if (remaining.length > 0) {
    io.stderr(`unexpected arguments: ${remaining.join(" ")}\n`);
    io.stderr("usage: aex webhooks secret [common flags]\n");
    return USAGE_ERR;
  }

  const http = makeHttpClient(io, common.flags);
  try {
    const { whsec } = await operations.getWebhookSigningSecret(http);
    io.stdout(`${whsec}\n`);
    return SUCCESS;
  } catch (err) {
    return emitApiError(io, "webhooks_secret_failed", err);
  }
}
