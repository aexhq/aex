/**
 * Response schema for `GET /whoami` — the data-plane identity canary.
 *
 * This is the route `parseWhoAmI` already hand-guards, including an explicit
 * "these three fields were removed and must not reappear" check for `caps`,
 * `tokenId` and `tokenName`. A strict object generalises that: EVERY undeclared
 * field is a reappearance, not just the three someone remembered to list.
 *
 * The CONTROL plane serves a different body at the same path — an
 * `account_token` principal, parsed by `parseAccountWhoAmI`. It is declared here
 * too, and the two are NOT unioned: unioning them would let a data-plane whoami
 * pass while carrying a control-plane body. They are kept apart by ORIGIN
 * instead — `WireResponse` carries the origin it came from, and the harness is
 * installed with the plane its bindings describe. See
 * `testing/response-bindings.ts`.
 */
import * as z from "zod/mini";
import { BILLING_ADMISSION_STATES } from "../billing-admission.js";
import { RUNTIME_KINDS } from "./runtime-kind.js";
import { RUNTIME_SIZES } from "./runtime-sizes.js";
import {
  describeResponse,
  responseObject,
  wireBoolean,
  wireEnum,
  wireLiteral,
  wireNonEmptyString,
  wireNumber,
  wireString
} from "./response-common.js";

const optional = z.optional;

/** A per-runtime-kind partial map, strict about which kinds may appear. */
function byRuntimeKind<Schema extends z.core.$ZodType>(value: Schema) {
  return responseObject({
    container: optional(value),
    spot_container: optional(value),
    lambda: optional(value)
  });
}

export const RuntimeCapabilitiesSchema = describeResponse(
  "RuntimeCapabilities",
  "Authenticated runtime availability for the workspace. `sizesByRuntimeKind` and " +
    "`unavailable` are complementary partial maps over the runtime kinds.",
  responseObject({
    schemaVersion: wireLiteral(1),
    capabilityVersion: wireNonEmptyString,
    capabilityHash: wireNonEmptyString,
    availableRuntimeKinds: z.array(wireEnum(RUNTIME_KINDS)),
    sizesByRuntimeKind: byRuntimeKind(z.array(wireEnum(RUNTIME_SIZES))),
    unavailable: byRuntimeKind(responseObject({ code: wireNonEmptyString }))
  })
);

/**
 * Effective workspace limits, from the same read models admission uses.
 *
 * Every field is REQUIRED. The plan catalog is gone, and with it the optional
 * `pastDueAt` / `graceEndsAt` pair that only existed while a subscription could
 * be in arrears — prepaid credit cannot go negative, so there is no dunning
 * state to report. What gates a submit now is the pair
 * (`balanceUsd`, `llmTokenAllowanceRemainingUsd`), and `admissionState` says
 * which of the three card-derived states produced the caps above it. A strict
 * schema still expecting the retired fields fails C4 against the current server.
 *
 * `admissionState` is a CLOSED enum here — unlike an allowance `dimension`, the
 * three admission states are public contract (`BillingAdmissionState`), not
 * hosted policy, so the declared union and the wire assertion share one list.
 */
export const WhoAmILimitsSchema = describeResponse(
  "WhoAmILimits",
  "Effective concurrency, rate, spend and prepaid-credit limits plus the card-derived admission state.",
  responseObject({
    maxConcurrentSessions: wireNumber,
    submitRatePerMinute: wireNumber,
    spendCapUsd: wireNumber,
    monthSpendUsd: wireNumber,
    balanceUsd: wireNumber,
    balanceGraceFloorUsd: wireNumber,
    llmTokenAllowanceRemainingUsd: wireNumber,
    creditGateActive: wireBoolean,
    paymentMethodStatus: wireEnum(["none", "active"]),
    admissionState: wireEnum(BILLING_ADMISSION_STATES),
    autoTopupEnabled: wireBoolean,
    accountType: wireEnum(["standard", "internal"])
  })
);

/** `GET /whoami` on the DATA plane (workspace API key). */
export const WhoAmIResponseSchema = describeResponse(
  "WhoAmIResponse",
  "Identity of a workspace API key. Strict: a removed field reappearing fails, " +
    "which is what the hand-written `caps`/`tokenId`/`tokenName` guard did for three names.",
  responseObject({
    ok: wireLiteral(true),
    principalType: wireLiteral("api_key"),
    workspaceId: wireNonEmptyString,
    scopes: z.array(wireNonEmptyString),
    limits: WhoAmILimitsSchema,
    runtimeCapabilities: optional(RuntimeCapabilitiesSchema)
  })
);

/**
 * `GET /whoami` on the CONTROL plane (account PAT / device session).
 *
 * Not bound to a data-plane route: `api-routes.ts` describes the data plane, and
 * the control plane has no route table in this package yet. Declared so the
 * shape has one statement, and so a control-plane binding table can be built
 * without re-deriving it.
 */
export const AccountWhoAmIResponseSchema = describeResponse(
  "AccountWhoAmIResponse",
  "Identity of a control-plane account token. No workspace, no data-plane limits.",
  responseObject({
    ok: wireLiteral(true),
    principalType: wireLiteral("account_token"),
    appUserId: wireNonEmptyString,
    scopes: z.array(wireString),
    orgId: optional(wireString),
    tokenId: optional(wireString),
    tokenName: optional(wireString),
    tokenKind: optional(wireString)
  })
);

export type WhoAmIResponse = z.infer<typeof WhoAmIResponseSchema>;
