/** JSON-compatible scalar values carried by public wire contracts. */
export type JsonPrimitive = string | number | boolean | null;

/** Recursively JSON-compatible values; numbers must be finite at runtime. */
export type JsonValue = JsonPrimitive | JsonValue[] | { readonly [key: string]: JsonValue };

/**
 * Broad object-record guard used at untrusted wire boundaries.
 *
 * Deliberately does not require a plain object or a particular prototype. The
 * package's existing parsers have always accepted every non-null, non-array
 * object at this first shape boundary and apply their domain checks afterwards.
 */
export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Recursively validate the package's JSON value contract. */
export function isJsonValue(value: unknown): value is JsonValue {
  if (typeof value === "number") {
    return Number.isFinite(value);
  }
  if (value === null || typeof value === "string" || typeof value === "boolean") {
    return true;
  }
  if (Array.isArray(value)) {
    return value.every(isJsonValue);
  }
  if (isRecord(value)) {
    return Object.values(value).every(isJsonValue);
  }
  return false;
}

/** Validate an object record whose enumerable values are recursively JSON-compatible. */
export function isJsonRecord(value: unknown): value is Record<string, JsonValue> {
  return isRecord(value) && Object.values(value).every(isJsonValue);
}

/** Narrow an unknown scalar to a member of an ordered readonly string tuple. */
export function isStringLiteral<const TAllowed extends readonly string[]>(
  value: unknown,
  allowed: TAllowed
): value is TAllowed[number] {
  return typeof value === "string" && allowed.includes(value);
}
