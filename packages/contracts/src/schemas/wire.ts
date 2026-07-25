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
  /** Raised for the first key the shape does not declare. */
  readonly unknownKey: (path: string, key: string, permitted: readonly string[]) => string;
}

const defaultText: WireObjectText = {
  notObject: (path) => `${path} must be an object`,
  unknownKey: (path, key, permitted) =>
    `${path}.${key} is not an allowed field; permitted: ${permitted.join(", ")}`
};

/**
 * A strict object schema that reports unknown keys and non-object input in the
 * caller's own words.
 *
 * `path` is the wire path the surrounding parser reports errors under
 * (`"webhook"`, `"submission.environment"`), not a schema name.
 */
export function wireObject<Shape extends z.core.$ZodLooseShape>(
  path: string,
  shape: Shape,
  text: Partial<WireObjectText> = {}
): z.ZodMiniObject<Shape, z.core.$strict> {
  const permitted = Object.freeze(Object.keys(shape));
  const notObject = text.notObject ?? defaultText.notObject;
  const unknownKey = text.unknownKey ?? defaultText.unknownKey;
  return z.strictObject(shape, {
    error: (issue) =>
      issue.code === "unrecognized_keys"
        ? unknownKey(path, issue.keys[0] ?? "", permitted)
        : notObject(path)
  });
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

/** Parse `input` against `schema`, throwing the family's own error text on failure. */
export function parseWire<Schema extends z.core.$ZodType>(
  schema: Schema,
  input: unknown
): z.infer<Schema> {
  const result = z.safeParse(schema, input);
  if (!result.success) {
    throw errorFromZod(result.error);
  }
  return result.data;
}
