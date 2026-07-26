/**
 * Shared machinery for schema-backed wire parsing.
 *
 * Every module in this directory imports **only `zod/mini`**. The published SDK
 * inlines this package, and full Zod's method-chained API defeats tree-shaking
 * (measured: 63.7 KB gzip for a trivial schema, against 4.3 KB for the same
 * schema in mini). Build-time tooling that needs the classic surface — the
 * OpenAPI generator — imports full `zod` off these same schema objects, which is
 * sound because both entrypoints construct the same core classes.
 *
 * The house parser idiom this replaces paired a hand-written allow-list with
 * `defineAllowedKeys<T>()`, a compile-time proof that the runtime list named
 * every key of the TypeScript interface and no others. That proof is not
 * preserved here so much as made unnecessary: the schema *is* the type, so
 * {@link wireObject} reads the ordered permitted list off the shape it was
 * built from and there is no second list to disagree with the first.
 */
import * as z from "zod/mini";

/**
 * Wording for the two diagnostics a strict wire object raises on its own
 * behalf. Families differ ("is not an allowed field; permitted: …" against "is
 * not allowed; permitted: …" against "contains unexpected field: …"), so each
 * supplies the text it already emits. The ordered key list handed to
 * {@link WireObjectText.unknownKey} always comes from the schema shape.
 */
export interface WireObjectText {
  /** Raised when the input is not an object at all. */
  readonly notObject: (path: string) => string;
  /**
   * Raised for the first key the shape does not declare.
   *
   * Return an `Error` instance rather than a string to keep a structured
   * diagnostic — `UnknownFieldError` carries `objectPath`, `unknownKey` and the
   * ordered `permittedKeys` as fields. Zod's message channel is strings only, so
   * {@link parseWire} re-invokes this to recover the instance at the throw site;
   * implementations must therefore be pure. Only applies at the root of the
   * parsed schema; a nested unknown key surfaces as its message alone.
   */
  readonly unknownKey: (
    path: string,
    key: string,
    permitted: readonly string[]
  ) => string | Error;
}

const defaultText: WireObjectText = {
  notObject: (path) => `${path} must be an object`,
  unknownKey: (path, key, permitted) =>
    `${path}.${key} is not an allowed field; permitted: ${permitted.join(", ")}`
};

/**
 * What each `wireObject` schema needs in order to rebuild a structured error.
 * Keyed by the schema object so {@link parseWire} — which is handed exactly that
 * object — can look it up without the family passing anything twice.
 */
interface WireObjectMeta {
  readonly resolvePath: (issuePath: readonly PropertyKey[]) => string;
  readonly permitted: readonly string[];
  readonly unknownKey: WireObjectText["unknownKey"];
}

const wireObjectMeta = new WeakMap<object, WireObjectMeta>();

/**
 * Render a wire path from a base and the position Zod reports an issue at:
 * `submission.environment.packages` + `[0]` -> `submission.environment.packages[0]`.
 *
 * Array indices render as `[0]` and object keys as `.name`, matching the paths
 * the hand-written parsers interpolate into their messages.
 */
export function wirePath(base: string, issuePath: readonly PropertyKey[]): string {
  return issuePath.reduce<string>(
    (rendered, segment) =>
      typeof segment === "number" ? `${rendered}[${segment}]` : `${rendered}.${String(segment)}`,
    base
  );
}

/**
 * The wire path of an object that sits at an INDEXED position in a list —
 * `secrets.mcpServers[2]`, `submission.assets.files[0]` — derived from where
 * Zod reports the issue rather than from the whole reported position.
 *
 * Prefer this over {@link wirePath} for anything mounted inside an array.
 * Zod reports a position relative to the schema the parse STARTED at, so a
 * schema that renders its own path by folding the whole issue path is only
 * correct at one mount depth: parse the same object one level lower and the
 * segment doubles (`submission.environment.environment.packages[0]`). The
 * element's own diagnostics — not-an-object and rejected-key — always report at
 * the element's position, so the element INDEX is the last segment whatever
 * sits above it, and reading just that segment makes the message depth-proof.
 */
export function indexedPath(base: string): (issuePath: readonly PropertyKey[]) => string {
  return (issuePath) => `${base}[${String(issuePath[issuePath.length - 1] ?? 0)}]`;
}

/**
 * The path of a FIELD on an element at an indexed position:
 * `submission.assets.files[0].mountPath`.
 *
 * Depth-proof for the same reason as {@link indexedPath}, reading the last two
 * segments — the element index and the field name — off the reported position.
 */
export function indexedFieldPath(
  base: string,
  issuePath: readonly PropertyKey[] | undefined
): string {
  const path = issuePath ?? [];
  return `${base}[${String(path[path.length - 2] ?? 0)}].${String(path[path.length - 1] ?? "")}`;
}

/** The last segment of a reported position — a record key, or an array index. */
export function lastSegment(issuePath: readonly PropertyKey[] | undefined): string {
  const segment = issuePath?.[issuePath.length - 1];
  return segment === undefined ? "" : String(segment);
}

/**
 * A required, non-empty string reported under `path`.
 *
 * The type failure and the emptiness failure share one message because the
 * `requireString(value, path)` ladder this replaces collapsed them, and `abort`
 * keeps the second from firing on a value that never cleared the first.
 */
export function nonEmptyString(path: string): z.ZodMiniString<string> {
  const message = `${path} must be a non-empty string`;
  return z.string({ error: message }).check(z.minLength(1, { error: message, abort: true }));
}

/**
 * A strict object schema that reports unknown keys and non-object input in the
 * caller's own words.
 *
 * `path` is the wire path the surrounding parser reports errors under
 * (`"webhook"`, `"submission.environment"`), not a schema name. Pass a function
 * when the object sits inside an array and the path carries an index — it
 * receives the issue's position, which {@link wirePath} renders.
 */
export function wireObject<Shape extends z.core.$ZodLooseShape>(
  path: string | ((issuePath: readonly PropertyKey[]) => string),
  shape: Shape,
  text: Partial<WireObjectText> = {}
): z.ZodMiniObject<Shape, z.core.$strict> {
  const permitted = Object.freeze(Object.keys(shape));
  const notObject = text.notObject ?? defaultText.notObject;
  const unknownKey = text.unknownKey ?? defaultText.unknownKey;
  const resolvePath =
    typeof path === "string" ? () => path : (issuePath: readonly PropertyKey[]) => path(issuePath);
  const schema = z.strictObject(shape, {
    error: (issue) =>
      issue.code === "unrecognized_keys"
        ? messageOf(unknownKey(resolvePath(issue.path ?? []), issue.keys[0] ?? "", permitted))
        : notObject(resolvePath(issue.path ?? []))
  });
  wireObjectMeta.set(schema, { resolvePath, permitted, unknownKey });
  return schema;
}

function messageOf(text: string | Error): string {
  return typeof text === "string" ? text : text.message;
}

/**
 * The issue a hand-written parser would have raised first.
 *
 * The parsers these schemas replace validate an object top-down — shape, then
 * the allow-list, then each field in turn, recursing as they go — so the
 * shallowest complaint surfaces and deeper ones never run. Zod collects every
 * issue instead. Ordering by path depth, with a rejected key ahead of a field
 * error at the same depth, reproduces the original traversal, which the
 * error-precedence tests pin.
 */
function primaryIssue(issues: readonly z.core.$ZodIssue[]): z.core.$ZodIssue | undefined {
  return [...issues].sort(
    (left, right) =>
      left.path.length - right.path.length || issueRank(left) - issueRank(right)
  )[0];
}

function issueRank(issue: z.core.$ZodIssue): number {
  return issue.code === "unrecognized_keys" ? 0 : 1;
}

/**
 * Map a failed parse onto the package's error surface: a plain `Error` carrying
 * the message the schema declared.
 *
 * Deliberately not a `ZodError`. Callers wrap parsers in `withContractParseError`,
 * which brands the thrown error with non-enumerable metadata and asserts the
 * error has no enumerable own keys of its own; a `ZodError` carries `issues`
 * and would break that contract as well as leak the schema library into a
 * public error shape.
 */
export function errorFromZod(error: z.core.$ZodError): Error {
  return new Error(primaryIssue(error.issues)?.message ?? "invalid input");
}

/**
 * Drop explicit `undefined` from optional properties.
 *
 * `z.infer` of an optional field yields `field?: T | undefined`, which under
 * `exactOptionalPropertyTypes` is NOT assignable to the package's `field?: T`
 * interfaces — an inferred value may carry the key with an `undefined` value,
 * and those interfaces promise it is either absent or a `T`.
 *
 * A `normalize*()` function is where that promise is actually kept, since it is
 * the step that drops absent fields, so it is where this type belongs.
 */
export type Present<T> = { [K in keyof T]?: Exclude<T[K], undefined> };

/** Parse `input` against `schema`, throwing the family's own error text on failure. */
export function parseWire<Schema extends z.core.$ZodType>(
  schema: Schema,
  input: unknown
): z.infer<Schema> {
  const meta = wireObjectMeta.get(schema as object);
  if (meta && input !== null && typeof input === "object" && !Array.isArray(input)) {
    // Zod deliberately skips `__proto__` while collecting unknown keys so a
    // plain-object output cannot have its prototype replaced. That safety
    // rule must not turn an attacker-controlled field into an accepted field;
    // recover the strict wire-object diagnostic before Zod sees the input.
    const unknownKey = Object.keys(input).find((key) => !meta.permitted.includes(key));
    if (unknownKey !== undefined) {
      const diagnostic = meta.unknownKey(meta.resolvePath([]), unknownKey, meta.permitted);
      throw typeof diagnostic === "string" ? new Error(diagnostic) : diagnostic;
    }
  }
  const result = z.safeParse(schema, input);
  if (!result.success) {
    throw structuredError(schema, result.error) ?? errorFromZod(result.error);
  }
  return result.data;
}

/**
 * Recover the family's own `Error` subclass for an unknown key at the root of
 * this schema, when it declared one.
 *
 * Zod carries messages, not error instances, so the structured diagnostic has to
 * be rebuilt here — this is the one place that holds both the schema and the
 * failure.
 */
function structuredError(schema: object, error: z.core.$ZodError): Error | undefined {
  const issue = primaryIssue(error.issues);
  if (issue?.code !== "unrecognized_keys" || issue.path.length > 0) {
    return undefined;
  }
  const meta = wireObjectMeta.get(schema);
  if (!meta) {
    return undefined;
  }
  const rebuilt = meta.unknownKey(meta.resolvePath(issue.path), issue.keys[0] ?? "", meta.permitted);
  return typeof rebuilt === "string" ? undefined : rebuilt;
}
