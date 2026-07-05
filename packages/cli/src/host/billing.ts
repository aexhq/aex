/**
 * `aex billing` — GET /api/billing. Prints the workspace prepaid balance,
 * current-month spend, and spend cap human-readably; `--json` emits the raw
 * wire body (which may carry additive server fields) for scripting.
 *
 * `aex billing ledger [--limit N]` — GET /api/billing/ledger. Prints the recent
 * credit-ledger rows (newest first) as JSON, matching the other read verbs.
 *
 * `aex billing upgrade pro|team` / `aex billing portal` create hosted billing
 * sessions and print the URL (or JSON with `--json`).
 */
import { operations } from "@aexhq/contracts";
import type { CliIO } from "../internal.js";
import {
  type CliExitCode,
  type CommonHostFlags,
  SUCCESS,
  USAGE_ERR,
  describeApiError,
  emitJsonError,
  makeHttpClient,
  resolveCommonHostFlags,
  refuseInsideManagedRun,
  takeOptionFlag
} from "./common.js";

function usd(value: unknown): string {
  return typeof value === "number" && Number.isFinite(value) ? `$${value.toFixed(2)}` : "-";
}

function isPaidPlanKey(value: unknown): value is "pro" | "team" {
  return value === "pro" || value === "team";
}

/** Parse a positive-integer `--limit` value; returns null (after printing) when invalid. */
function parseLimit(io: CliIO, raw: string | undefined): { ok: true; limit: number | undefined } | { ok: false } {
  if (raw === undefined) return { ok: true, limit: undefined };
  const limit = Number(raw);
  if (!Number.isInteger(limit) || limit < 1) {
    io.stderr(`--limit must be a positive integer (got: ${raw})\n`);
    return { ok: false };
  }
  return { ok: true, limit };
}

export async function runBillingCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  if (await refuseInsideManagedRun(io, "billing")) return USAGE_ERR;

  const common = await resolveCommonHostFlags(io, argv);
  if (!common.ok) {
    io.stderr(`${common.reason}\n`);
    return USAGE_ERR;
  }

  if (common.rest[0] === "ledger") {
    return runBillingLedger(io, common.rest.slice(1), common.flags);
  }
  if (common.rest[0] === "upgrade") {
    return runBillingUpgrade(io, common.rest.slice(1), common.flags);
  }
  if (common.rest[0] === "portal") {
    return runBillingPortal(io, common.rest.slice(1), common.flags);
  }

  // `--json` is a global flag consumed by resolveCommonHostFlags.
  const json = common.flags.json;
  if (common.rest.length > 0) {
    io.stderr(`unexpected arguments: ${common.rest.join(" ")}\n`);
    io.stderr("usage: aex billing [--json] | aex billing ledger [--limit N] | aex billing upgrade pro|team | aex billing portal [common flags]\n");
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
    const d = describeApiError(err);
    return emitJsonError(io, "billing_failed", d.message, {
      ...(d.status !== undefined ? { status: d.status } : {}),
      ...(d.remedy ? { remedy: d.remedy } : {})
    });
  }
}

async function runBillingUpgrade(
  io: CliIO,
  argv: readonly string[],
  flags: CommonHostFlags
): Promise<CliExitCode> {
  const json = flags.json;
  const { value: successUrl, remaining: rest2 } = takeOptionFlag(argv, "--success-url");
  const { value: cancelUrl, remaining: rest3 } = takeOptionFlag(rest2, "--cancel-url");
  const { value: idempotencyKey, remaining } = takeOptionFlag(rest3, "--idempotency-key");
  const planKey = remaining[0];
  if (!isPaidPlanKey(planKey) || remaining.length !== 1) {
    io.stderr("usage: aex billing upgrade pro|team [--success-url URL] [--cancel-url URL] [--idempotency-key KEY] [--json] [common flags]\n");
    return USAGE_ERR;
  }

  const http = makeHttpClient(io, flags);
  try {
    const session = await operations.createBillingCheckout(http, {
      planKey,
      ...(successUrl !== undefined ? { successUrl } : {}),
      ...(cancelUrl !== undefined ? { cancelUrl } : {}),
      ...(idempotencyKey !== undefined ? { idempotencyKey } : {}),
    });
    io.stdout(json ? `${JSON.stringify(session)}\n` : `${session.url}\n`);
    return SUCCESS;
  } catch (err) {
    const d = describeApiError(err);
    return emitJsonError(io, "billing_checkout_failed", d.message, {
      ...(d.status !== undefined ? { status: d.status } : {}),
      ...(d.remedy ? { remedy: d.remedy } : {})
    });
  }
}

async function runBillingPortal(
  io: CliIO,
  argv: readonly string[],
  flags: CommonHostFlags
): Promise<CliExitCode> {
  const json = flags.json;
  const { value: returnUrl, remaining } = takeOptionFlag(argv, "--return-url");
  if (remaining.length > 0) {
    io.stderr(`unexpected arguments: ${remaining.join(" ")}\n`);
    io.stderr("usage: aex billing portal [--return-url URL] [--json] [common flags]\n");
    return USAGE_ERR;
  }

  const http = makeHttpClient(io, flags);
  try {
    const session = await operations.createBillingPortal(
      http,
      returnUrl !== undefined ? { returnUrl } : undefined
    );
    io.stdout(json ? `${JSON.stringify(session)}\n` : `${session.url}\n`);
    return SUCCESS;
  } catch (err) {
    const d = describeApiError(err);
    return emitJsonError(io, "billing_portal_failed", d.message, {
      ...(d.status !== undefined ? { status: d.status } : {}),
      ...(d.remedy ? { remedy: d.remedy } : {})
    });
  }
}

async function runBillingLedger(
  io: CliIO,
  argv: readonly string[],
  flags: CommonHostFlags
): Promise<CliExitCode> {
  const { value: rawLimit, remaining } = takeOptionFlag(argv, "--limit");
  if (remaining.length > 0) {
    io.stderr(`unexpected arguments: ${remaining.join(" ")}\n`);
    io.stderr("usage: aex billing ledger [--limit N] [common flags]\n");
    return USAGE_ERR;
  }
  const parsed = parseLimit(io, rawLimit);
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
    const d = describeApiError(err);
    return emitJsonError(io, "billing_ledger_failed", d.message, {
      ...(d.status !== undefined ? { status: d.status } : {}),
      ...(d.remedy ? { remedy: d.remedy } : {})
    });
  }
}
