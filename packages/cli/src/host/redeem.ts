/**
 * `aex redeem <code>` — POST /billing/redeem.
 *
 * Redeems a single-use coupon code to fund the workspace's prepaid USD
 * balance. The workspace is derived server-side from the API token.
 *
 * Intentionally implemented as a DIRECT fetch rather than through a shared
 * `operations.*` / SDK helper: redemption is a CLI-only affordance and must
 * stay off the public `@aexhq/sdk` surface. Keeping the call here — reading the
 * status/error code straight off the `Response` — is what holds that boundary.
 */
import type { CliIO } from "../internal.js";
import {
  type CliExitCode,
  SUCCESS,
  USAGE_ERR,
  RUNTIME_ERR,
  resolveCommonHostFlags,
  refuseInsideManagedRun
} from "./common.js";

interface RedeemSuccessBody {
  readonly redeemed: true;
  readonly amountUsd: number;
  readonly newBalanceUsd: number;
}

/** Human-readable message for a non-2xx redeem response, keyed on HTTP status. */
function messageForStatus(status: number, serverMessage: string | undefined): string {
  switch (status) {
    case 404:
      return "coupon code not found";
    case 403:
      return "this coupon can't be redeemed by this workspace";
    case 409:
      return "coupon already redeemed";
    case 400:
      return serverMessage ? `invalid input: ${serverMessage}` : "invalid input";
    case 401:
      return "not authorized — check --api-token, or run `aex login`";
    default:
      return serverMessage ? `redeem failed: ${serverMessage}` : `redeem failed (HTTP ${status})`;
  }
}

export async function runRedeemCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  if (await refuseInsideManagedRun(io, "redeem")) return USAGE_ERR;

  const common = await resolveCommonHostFlags(io, argv);
  if (!common.ok) {
    io.stderr(`${common.reason}\n`);
    return USAGE_ERR;
  }
  const positional = common.rest.filter((arg) => !arg.startsWith("--"));
  if (positional.length !== 1) {
    io.stderr("usage: aex redeem <code> [common flags]\n");
    return USAGE_ERR;
  }
  const code = positional[0]!;

  // DIRECT fetch — deliberately not routed through `@aexhq/sdk`. Redemption is
  // CLI-only, so it must not gain a public SDK method. The workspace bearer the
  // CLI already resolves (flag or stored login) authenticates the call.
  const base = common.flags.aexUrl.replace(/\/+$/, "");
  const url = `${base}/billing/redeem`;

  let response: Response;
  try {
    response = await io.fetchImpl(url, {
      method: "POST",
      headers: {
        accept: "application/json",
        "content-type": "application/json",
        authorization: `Bearer ${common.flags.apiToken}`
      },
      body: JSON.stringify({ code })
    });
  } catch (err) {
    io.stderr(`redeem failed: ${err instanceof Error ? err.message : String(err)}\n`);
    return RUNTIME_ERR;
  }

  if (common.flags.debug) {
    io.stderr(`[aex] POST /billing/redeem -> ${response.status}\n`);
  }

  const text = await response.text();
  let body: unknown = {};
  try {
    if (text.length > 0) body = JSON.parse(text);
  } catch {
    body = {};
  }

  if (!response.ok) {
    const serverMessage =
      body && typeof body === "object" && typeof (body as { message?: unknown }).message === "string"
        ? (body as { message: string }).message
        : undefined;
    io.stderr(`${messageForStatus(response.status, serverMessage)}\n`);
    return RUNTIME_ERR;
  }

  const ok = body as Partial<RedeemSuccessBody>;
  const amountUsd = typeof ok.amountUsd === "number" ? ok.amountUsd : 0;
  const newBalanceUsd = typeof ok.newBalanceUsd === "number" ? ok.newBalanceUsd : 0;
  io.stdout(
    `Redeemed $${amountUsd.toFixed(2)}. New balance: $${newBalanceUsd.toFixed(2)}.\n`
  );
  return SUCCESS;
}
