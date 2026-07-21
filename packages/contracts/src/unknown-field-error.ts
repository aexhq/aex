/**
 * Structured metadata for a strict-parser unknown-field diagnostic.
 *
 * This class is exported only from the workspace-internal contracts entrypoint.
 * Its inherited `Error` name and formatted message preserve existing callers.
 */
export class UnknownFieldError extends Error {
  readonly objectPath: string;
  readonly unknownKey: string;
  readonly permittedKeys: readonly string[];

  constructor(
    objectPath: string,
    unknownKey: string,
    permittedKeys: readonly string[]
  ) {
    const copiedPermittedKeys = Object.freeze([...permittedKeys]);
    super(
      `${objectPath}.${unknownKey} is not an allowed field; permitted: ${copiedPermittedKeys.join(", ")}`
    );
    this.objectPath = objectPath;
    this.unknownKey = unknownKey;
    this.permittedKeys = copiedPermittedKeys;
  }

  withPermittedKeys(permittedKeys: readonly string[]): UnknownFieldError {
    return new UnknownFieldError(this.objectPath, this.unknownKey, permittedKeys);
  }
}
