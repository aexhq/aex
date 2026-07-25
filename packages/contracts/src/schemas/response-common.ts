/**
 * Shared machinery for the RESPONSE half of the wire contract (C4 / P3).
 *
 * The request schemas answer "what may a caller send". These answer "what did
 * the server actually send", and they are checked against real bytes by
 * {@link import("../testing/wire-conformance.js").installWireConformance} rather
 * than by anything on the request path. That is the whole value: it is the one
 * gate in the contract pipeline that observes reality instead of comparing two
 * of our own artefacts to each other.
 *
 * Three rules the modules in this family follow:
 *
 * 1. **Strict.** Every response object is a {@link responseObject}, i.e. a
 *    `z.strictObject`. A field the server added and we never declared FAILS the
 *    suite. That generalises what `parseWhoAmI` did by hand for three removed
 *    fields (`caps`, `tokenId`, `tokenName`) to the whole surface. Several of
 *    our own declared types used to carry `[key: string]: unknown` — an explicit
 *    "additive server fields pass through" promise — and these schemas
 *    deliberately did not honour it. They no longer have to: an index signature
 *    makes EVERY undeclared field structurally legal, which is how
 *    `BillingSummary` came to omit two fields the server always sends without
 *    anything being able to notice. The signatures are gone from the types too.
 * 2. **No `.transform()`.** `z.toJSONSchema(s, { io: "output" })` throws on any
 *    transform, which would make the response half of the generated spec
 *    ungenerable. Schemas validate; `normalize*()` functions transform.
 * 3. **`zod/mini` only**, like every other module in this directory.
 */
import * as z from "zod/mini";
import { wireObject, wirePath } from "./wire.js";

/**
 * Metadata registry for response schemas.
 *
 * Deliberately NOT `z.globalRegistry`. The OpenAPI generator converts
 * `OPENAPI_SCHEMA_REGISTRY`, and that constant *is* `z.globalRegistry` — so
 * registering a response schema there would silently add a component to
 * `openapi/data-plane.json`, break the committed-spec freshness gate (C3), and
 * do it as a side effect of merely importing this file. Response schemas are not
 * referenced by any generated operation yet (`scripts/openapi/generate.ts` still
 * emits `"2XX": { description: "Success." }`), so an entry there would be an
 * unreferenced component describing nothing.
 *
 * When the generator learns to declare responses, this registry is what it
 * converts — the ids below are already the component names.
 */
export const responseSchemaRegistry = z.registry<{
  readonly id: string;
  readonly description: string;
}>();

/** Render the position of an issue as a wire path rooted at the response body. */
function responsePath(issuePath: readonly PropertyKey[]): string {
  return wirePath("response", issuePath);
}

/** An issue as Zod hands it to an error map — only its position matters here. */
interface PositionedIssue {
  readonly path?: readonly PropertyKey[] | undefined;
}

function expected(description: string): (issue: PositionedIssue) => string {
  return (issue) => `${responsePath(issue.path ?? [])} must be ${description}`;
}

/**
 * A strict response object.
 *
 * Paths are resolved against the ROOT of the parse rather than anchored on a
 * fixed name, so a nested object reports its real position
 * (`response.session.currentRun.phase`) instead of doubling a segment — the trap
 * the port measured and `08-refined-plan.md` records.
 */
export function responseObject<Shape extends z.core.$ZodLooseShape>(
  shape: Shape
): z.ZodMiniObject<Shape, z.core.$strict> {
  return wireObject(responsePath, shape, {
    notObject: (path) => `${path} must be an object`,
    unknownKey: (path, key, permitted) =>
      `${path}.${key} is not a declared response field; declared: ${permitted.join(", ")}`
  });
}

/**
 * Attach the component id and prose a spec consumer needs.
 *
 * `.meta()` does not exist on `zod/mini` schemas; `.register()` is the mini
 * equivalent and is what every metadata instruction in the plan documents means.
 */
export function describeResponse<Schema extends z.core.$ZodType>(
  id: string,
  description: string,
  schema: Schema
): Schema {
  responseSchemaRegistry.add(schema, { id, description });
  return schema;
}

// ===========================================================================
// Field primitives
//
// One instance each, shared across every response shape. Zod schemas are
// immutable, and the message is derived from the issue's own path, so a single
// `wireString` reports `response.session.id must be a string` in one object and
// `response.entries[3].currency must be a string` in another.
// ===========================================================================

export const wireString = z.string({ error: expected("a string") });

/**
 * A string the hand-written parsers already required to be non-empty (ids,
 * statuses, timestamps). Asserting less than the client already asserts would
 * make C4 weaker than the code it is meant to backstop.
 */
export const wireNonEmptyString = z.string({ error: expected("a non-empty string") }).check(
  z.refine((value: string) => value.length > 0, { error: expected("a non-empty string") })
);

/** A finite number. `z.number()` already rejects `NaN` and `±Infinity` (measured). */
export const wireNumber = z.number({ error: expected("a finite number") });

export const wireNonNegativeNumber = z.number({ error: expected("a non-negative finite number") }).check(
  z.refine((value: number) => value >= 0, { error: expected("a non-negative finite number") })
);

export const wireInteger = z.int({ error: expected("a safe integer") });

export const wireNonNegativeInteger = z.int({ error: expected("a non-negative safe integer") }).check(
  z.refine((value: number) => value >= 0, { error: expected("a non-negative safe integer") })
);

export const wirePositiveInteger = z.int({ error: expected("a positive safe integer") }).check(
  z.refine((value: number) => value >= 1, { error: expected("a positive safe integer") })
);

export const wireBoolean = z.boolean({ error: expected("a boolean") });

/** An ISO-8601 timestamp, judged the way the parsers judge one: `Date.parse` succeeds. */
export const wireTimestamp = z.string({ error: expected("an ISO-8601 timestamp") }).check(
  z.refine((value: string) => Number.isFinite(Date.parse(value)), {
    error: expected("an ISO-8601 timestamp")
  })
);

/** A closed vocabulary. The message names the accepted values, as the parsers do. */
export function wireEnum<const Values extends readonly [string, ...string[]]>(values: Values) {
  return z.enum(values, { error: expected(`one of: ${values.join(", ")}`) });
}

/** A literal the server always sends verbatim (`ok: true`, `kind: "file"`). */
export function wireLiteral<const Value extends string | number | boolean>(value: Value) {
  return z.literal(value, { error: expected(JSON.stringify(value)) });
}

/**
 * The empty JSON body an HTTP 204 becomes by the time the harness sees it.
 *
 * `HttpClient.request` reads the body as text and returns `{}` for a zero-length
 * one, so a 204 arrives at the observer as an empty object. Declaring it as a
 * strict empty object is a real assertion — a route that starts returning
 * content fails.
 */
export const NoContentResponseSchema = describeResponse(
  "NoContentResponse",
  "An HTTP 204 with no body. `HttpClient` renders a zero-length body as `{}`, " +
    "so the assertion is that the route sends nothing at all.",
  responseObject({})
);

/**
 * The error envelope EVERY operation declares — and, until now, the only
 * declared response nothing ever checked.
 *
 * `scripts/openapi/generate.ts` gives all 68 operations a `default` response of
 * `#/components/schemas/ApiErrorEnvelope`. C4 could not see it, because
 * `HttpClient.request` threw on a non-2xx *before* reporting to the wire
 * observer, so the harness only ever met 2xx bodies. Both halves move together:
 * the report now happens ahead of the throw, and this is what the reported body
 * is checked against.
 *
 * ## Why this one is NOT a `responseObject`
 *
 * Every other schema in this family is strict, deliberately — an undeclared
 * field should fail the suite. The generated component declares
 * `additionalProperties: true`, and it is right to: the envelope is a BASE that
 * individual codes extend. `session_busy` adds the session's current `status`,
 * `rate_limited` adds `retryAfterMs`, `content_deleted` adds `sessionId` /
 * `purgedAt` / `deletedBy`, an auth failure adds `requiredScope` —
 * `error-factory.ts` reads every one of them. Declaring this strict would fail
 * the suite on error bodies our own client is built to consume, which is
 * inventing a contract rather than checking one.
 *
 * ## What it therefore does assert
 *
 * That a non-2xx JSON body carries the two fields the spec marks required — a
 * stable machine-readable `error` code and a human `message` — and that
 * `requestId`, when present, is a string. That is a real assertion about the
 * wire, not a tautology: the lambda's `finalizeApiResponse` only defaults a
 * `message` for codes present in its `API_ERROR_MESSAGES` table, and only
 * rewrites a body that already carries an `error` or a `code`. A rejection
 * produced anywhere other than a route handler — an API-Gateway-native 403, say
 * — carries neither and surfaces here as a violation. That is a FINDING about
 * the declared contract; do not loosen this schema to make such a body pass.
 */
export const ApiErrorEnvelopeSchema = describeResponse(
  "ApiErrorEnvelope",
  "The body every non-2xx response carries: a stable `error` code, a human " +
    "`message`, and the `requestId` to quote in a support request. Open by " +
    "design — individual codes extend it with their own fields.",
  z.looseObject(
    {
      error: wireNonEmptyString,
      message: wireString,
      requestId: z.optional(wireString)
    },
    { error: (issue) => `${responsePath(issue.path ?? [])} must be an object` }
  )
);
