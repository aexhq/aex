type StringKeyOf<T> = Extract<keyof T, string>;

type ExactKeyTuple<T, Keys extends readonly string[]> =
  Exclude<StringKeyOf<T>, Keys[number]> extends never
    ? Exclude<Keys[number], StringKeyOf<T>> extends never
      ? unknown
      : never
    : never;

/**
 * Preserve an ordered key tuple while requiring it to name every string key of
 * `T` and no others. This is intentionally private parser infrastructure.
 */
export function defineAllowedKeys<T>() {
  return <const Keys extends readonly string[]>(
    ...keys: Keys & ExactKeyTuple<T, Keys>
  ): Keys => keys;
}

/**
 * Reject the first unsupported enumerable own string key, using native
 * `Object.keys` ordering and domain-owned error text.
 */
export function assertAllowedKeys(
  record: object,
  allowedKeys: readonly string[],
  errorForKey: (key: string, orderedKeys: readonly string[]) => Error
): void {
  for (const key of Object.keys(record)) {
    if (!allowedKeys.includes(key)) {
      throw errorForKey(key, allowedKeys);
    }
  }
}
