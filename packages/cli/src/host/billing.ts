/**
 * `aex billing` — GET /api/billing. Prints the workspace prepaid balance,
 * current-month spend, spend cap, auto-recharge state and the free monthly
 * allowances human-readably; `--json` emits the raw wire body (which may carry
 * additive server fields) for scripting.
 *
 * `aex billing ledger [--limit N]` — GET /api/billing/ledger. Prints the recent
 * credit-ledger rows (newest first) as JSON, matching the other read verbs.
 *
 * `aex billing topup <amountUsd>` buys prepaid credit through hosted checkout
 * (the same flow captures the card on first use) and prints the URL.
 * `aex billing autotopup` sets auto-recharge. `aex billing portal` opens the
 * hosted billing portal.
 *
 * There is no `upgrade` verb: card presence is the only lever and there is no
 * plan to move between.
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
  takeBooleanFlag,
  takeOptionFlag
} from "./common.js";
import { parsePositiveLimit } from "./command-primitives.js";

const USAGE =
  "usage: aex billing [--json] | aex billing ledger [--limit N] | "
  + "aex billing topup <amountUsd> | aex billing autotopup [--enable|--disable] | "
  + "aex billing portal [common flags]\n";

function usd(value: unknown): string {
  return typeof value === "number" && Number.isFinite(value) ? `$${value.toFixed(2)}` : "-";
}

/**
 * A USD amount typed on the command line. Rejects anything non-positive or
 * non-finite BEFORE the network, so a fat-fingered `--amount` is a usage error
 * rather than a `400` round trip. The minimum is the server's to enforce — it is
 * billing policy and belongs in exactly one place.
 */
function parseUsdArg(io: CliIO, raw: string | undefined, label: string): number | null {
  const parsed = Number(raw);
  if (raw === undefined || raw.trim() === "" || !Number.isFinite(parsed) || parsed <= 0) {
    io.stderr(`${label} must be a positive amount in USD\n`);
    return null;
  }
  return parsed;
}

export async function executeBillingCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  const common = await prepareHostCommand(io, argv, { verb: "billing", auth: "data" });
  if (!common.ok) return common.exit;

  if (common.rest[0] === "ledger") {
    return runBillingLedger(io, common.rest.slice(1), common.flags);
  }
  if (common.rest[0] === "topup") {
    return runBillingTopup(io, common.rest.slice(1), common.flags);
  }
  if (common.rest[0] === "autotopup") {
    return runBillingAutoTopup(io, common.rest.slice(1), common.flags);
  }
  if (common.rest[0] === "portal") {
    return runBillingPortal(io, common.rest.slice(1), common.flags);
  }

  // `--json` is a global flag consumed by authenticated preparation.
  const json = common.flags.json;
  if (common.rest.length > 0) {
    io.stderr(`unexpected arguments: ${common.rest.join(" ")}\n`);
    io.stderr(USAGE);
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
    io.stdout(
      `Auto top-up:  ${billing.autoTopup.enabled ? "on" : "off"}`
      + ` (below ${usd(billing.autoTopup.thresholdUsd)} add ${usd(billing.autoTopup.amountUsd)})\n`
    );
    io.stdout(`Allowances (${billing.period}):\n`);
    // Quota, unit and label all come from the server. A CLI that formatted "GB"
    // beside a number it read from the wire would be a second copy of the
    // allowance table, and the wrong label on the right number is worse than none.
    for (const allowance of billing.allowances) {
      io.stdout(`  ${allowance.label.padEnd(14)} ${allowance.used} of ${allowance.quota} ${allowance.unit} used\n`);
    }
    return SUCCESS;
  } catch (err) {
    return emitApiError(io, "billing_failed", err);
  }
}

async function runBillingTopup(
  io: CliIO,
  argv: readonly string[],
  flags: CommonHostFlags
): Promise<CliExitCode> {
  const json = flags.json;
  const successFlag = takeOptionFlag(argv, "--success-url");
  const cancelFlag = takeOptionFlag(successFlag.remaining, "--cancel-url");
  const idempotencyFlag = takeOptionFlag(cancelFlag.remaining, "--idempotency-key");
  const optionError = successFlag.error ?? cancelFlag.error ?? idempotencyFlag.error;
  if (optionError) { io.stderr(`${optionError}\n`); return USAGE_ERR; }
  const { value: successUrl } = successFlag;
  const { value: cancelUrl } = cancelFlag;
  const { value: idempotencyKey, remaining } = idempotencyFlag;
  if (remaining.length !== 1) {
    io.stderr("usage: aex billing topup <amountUsd> [--success-url URL] [--cancel-url URL] [--idempotency-key KEY] [--json] [common flags]\n");
    return USAGE_ERR;
  }
  const amountUsd = parseUsdArg(io, remaining[0], "amountUsd");
  if (amountUsd === null) return USAGE_ERR;

  const http = makeHttpClient(io, flags);
  try {
    const session = await operations.createBillingTopupCheckout(http, {
      amountUsd,
      ...(successUrl !== undefined ? { successUrl } : {}),
      ...(cancelUrl !== undefined ? { cancelUrl } : {}),
    }, idempotencyKey !== undefined ? { idempotencyKey } : undefined);
    io.stdout(json ? `${JSON.stringify(session)}\n` : `${session.url}\n`);
    return SUCCESS;
  } catch (err) {
    return emitApiError(io, "billing_topup_failed", err);
  }
}

/**
 * `aex billing autotopup [--enable|--disable] [--threshold N] [--amount N]`.
 *
 * `--enable` and `--disable` are separate flags rather than one `--enabled
 * true|false` so that omitting both means "leave it as it is" — the same
 * semantics the PATCH itself has. Passing both is a usage error, not a
 * last-one-wins guess about which the caller meant.
 */
async function runBillingAutoTopup(
  io: CliIO,
  argv: readonly string[],
  flags: CommonHostFlags
): Promise<CliExitCode> {
  const json = flags.json;
  const enable = takeBooleanFlag(argv, "--enable");
  const disable = takeBooleanFlag(enable.remaining, "--disable");
  const thresholdFlag = takeOptionFlag(disable.remaining, "--threshold");
  const amountFlag = takeOptionFlag(thresholdFlag.remaining, "--amount");
  const optionError = thresholdFlag.error ?? amountFlag.error;
  if (optionError) { io.stderr(`${optionError}\n`); return USAGE_ERR; }
  const usage =
    "usage: aex billing autotopup [--enable|--disable] [--threshold N] [--amount N] [--json] [common flags]\n";
  if (amountFlag.remaining.length > 0) {
    io.stderr(`unexpected arguments: ${amountFlag.remaining.join(" ")}\n`);
    io.stderr(usage);
    return USAGE_ERR;
  }
  if (enable.present && disable.present) {
    io.stderr("--enable and --disable are mutually exclusive\n");
    io.stderr(usage);
    return USAGE_ERR;
  }

  const thresholdUsd = thresholdFlag.value === undefined
    ? undefined
    : parseUsdArg(io, thresholdFlag.value, "--threshold");
  if (thresholdUsd === null) return USAGE_ERR;
  const amountUsd = amountFlag.value === undefined
    ? undefined
    : parseUsdArg(io, amountFlag.value, "--amount");
  if (amountUsd === null) return USAGE_ERR;

  const http = makeHttpClient(io, flags);
  try {
    const { autoTopup } = await operations.updateBillingAutoTopup(http, {
      ...(enable.present ? { enabled: true } : {}),
      ...(disable.present ? { enabled: false } : {}),
      ...(thresholdUsd !== undefined ? { thresholdUsd } : {}),
      ...(amountUsd !== undefined ? { amountUsd } : {}),
    });
    if (json) {
      io.stdout(JSON.stringify({ autoTopup }) + "\n");
      return SUCCESS;
    }
    io.stdout(
      `Auto top-up:  ${autoTopup.enabled ? "on" : "off"}`
      + ` (below ${usd(autoTopup.thresholdUsd)} add ${usd(autoTopup.amountUsd)};`
      + ` minimum ${usd(autoTopup.minimumAmountUsd)}, at most ${autoTopup.maxPerDay}/day)\n`
    );
    return SUCCESS;
  } catch (err) {
    return emitApiError(io, "billing_autotopup_failed", err);
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
