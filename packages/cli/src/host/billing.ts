/**
 * `aex billing` — GET /api/billing. Prints the workspace prepaid balance,
 * current-month spend, and spend cap human-readably; `--json` emits the raw
 * wire body (which may carry additive server fields) for scripting.
 *
 * `aex billing ledger [--limit N]` — GET /api/billing/ledger. Prints the recent
 * credit-ledger rows (newest first) as JSON, matching the other read verbs.
 *
 * `aex billing portal` creates a hosted billing-portal session and prints the
 * URL (or JSON with `--json`).
 *
 * `aex billing upgrade` is GONE: it drove POST /api/billing/checkout, a route the
 * plan-catalog demolition removed server-side. A verb that can only 404 is not a
 * surface worth keeping.
 */
import { operations } from "@aexhq/contracts/internal";
import type { CliIO } from "../internal.js";
import {
  type CliExitCode,
  type CommonHostFlags,
  SUCCESS,
  USAGE_ERR,
  emitApiError,
  makeHttpClient,
  prepareHostCommand,
  takeOptionFlag
} from "./common.js";
import { parsePositiveLimit } from "./command-primitives.js";

function usd(value: unknown): string {
  return typeof value === "number" && Number.isFinite(value) ? `$${value.toFixed(2)}` : "-";
}
export async function executeBillingCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  const common = await prepareHostCommand(io, argv, { verb: "billing", auth: "data" });
  if (!common.ok) return common.exit;

  if (common.rest[0] === "ledger") {
    return runBillingLedger(io, common.rest.slice(1), common.flags);
  }
  if (common.rest[0] === "portal") {
    return runBillingPortal(io, common.rest.slice(1), common.flags);
  }

  // `--json` is a global flag consumed by authenticated preparation.
  const json = common.flags.json;
  if (common.rest.length > 0) {
    io.stderr(`unexpected arguments: ${common.rest.join(" ")}\n`);
    io.stderr("usage: aex billing [--json] | aex billing ledger [--limit N] | aex billing portal [common flags]\n");
    return USAGE_ERR;
  }

  const http = makeHttpClient(io, common.flags);
  try {
    const billing = await operations.getBilling(http);
    if (json) {
      io.stdout(JSON.stringify(billing) + "\n");
      return SUCCESS;
    }
    io.stdout(`Balance:      ${usd(billing.balanceUsd)}\n`);
    io.stdout(`Month spend:  ${usd(billing.monthSpendUsd)}\n`);
    io.stdout(`Spend cap:    ${usd(billing.spendCapUsd)}\n`);
    io.stdout(`Plan:         ${billing.planKey} (subscription: ${billing.subscriptionStatus})\n`);
    return SUCCESS;
  } catch (err) {
    return emitApiError(io, "billing_failed", err);
  }
}
async function runBillingPortal(
  io: CliIO,
  argv: readonly string[],
  flags: CommonHostFlags
): Promise<CliExitCode> {
  const json = flags.json;
  const returnFlag = takeOptionFlag(argv, "--return-url");
  const idempotencyFlag = takeOptionFlag(returnFlag.remaining, "--idempotency-key");
  const optionError = returnFlag.error ?? idempotencyFlag.error;
  if (optionError) { io.stderr(`${optionError}\n`); return USAGE_ERR; }
  const { value: returnUrl } = returnFlag;
  const { value: idempotencyKey, remaining } = idempotencyFlag;
  if (remaining.length > 0) {
    io.stderr(`unexpected arguments: ${remaining.join(" ")}\n`);
    io.stderr("usage: aex billing portal [--return-url URL] [--idempotency-key KEY] [--json] [common flags]\n");
    return USAGE_ERR;
  }

  const http = makeHttpClient(io, flags);
  try {
    const session = await operations.createBillingPortal(
      http,
      returnUrl !== undefined ? { returnUrl } : undefined,
      idempotencyKey !== undefined ? { idempotencyKey } : undefined
    );
    io.stdout(json ? `${JSON.stringify(session)}\n` : `${session.url}\n`);
    return SUCCESS;
  } catch (err) {
    return emitApiError(io, "billing_portal_failed", err);
  }
}

async function runBillingLedger(
  io: CliIO,
  argv: readonly string[],
  flags: CommonHostFlags
): Promise<CliExitCode> {
  const limitFlag = takeOptionFlag(argv, "--limit");
  if (limitFlag.error) { io.stderr(`${limitFlag.error}\n`); return USAGE_ERR; }
  const { value: rawLimit, remaining } = limitFlag;
  if (remaining.length > 0) {
    io.stderr(`unexpected arguments: ${remaining.join(" ")}\n`);
    io.stderr("usage: aex billing ledger [--limit N] [common flags]\n");
    return USAGE_ERR;
  }
  const parsed = parsePositiveLimit(io, rawLimit);
  if (!parsed.ok) return USAGE_ERR;

  const http = makeHttpClient(io, flags);
  try {
    const page = await operations.getBillingLedger(
      http,
      parsed.limit !== undefined ? { limit: parsed.limit } : undefined
    );
    io.stdout(JSON.stringify(page.entries) + "\n");
    return SUCCESS;
  } catch (err) {
    return emitApiError(io, "billing_ledger_failed", err);
  }
}
