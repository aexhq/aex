export const CONTRACT_PARSE_ERROR = "CONTRACT_PARSE_ERROR" as const;

const CONTRACT_PARSE_ERROR_BRAND = Symbol("@aexhq/contracts/ContractParseError");

export type ContractParseError = Error & {
  readonly code: typeof CONTRACT_PARSE_ERROR;
  readonly parser: string;
  readonly [CONTRACT_PARSE_ERROR_BRAND]: true;
};

export function isContractParseError(value: unknown): value is ContractParseError {
  try {
    return (
      value instanceof Error &&
      (value as Partial<ContractParseError>).code === CONTRACT_PARSE_ERROR &&
      typeof (value as Partial<ContractParseError>).parser === "string" &&
      (value as Partial<ContractParseError>)[CONTRACT_PARSE_ERROR_BRAND] === true
    );
  } catch {
    return false;
  }
}

export function markContractParseError(error: Error, parser: string): ContractParseError {
  if (isContractParseError(error)) return error;
  Object.defineProperties(error, {
    code: {
      value: CONTRACT_PARSE_ERROR,
      enumerable: false,
      writable: false,
      configurable: false
    },
    parser: {
      value: parser,
      enumerable: false,
      writable: false,
      configurable: false
    },
    [CONTRACT_PARSE_ERROR_BRAND]: {
      value: true,
      enumerable: false,
      writable: false,
      configurable: false
    }
  });
  return error as ContractParseError;
}

export function rethrowContractParseError(error: unknown, parser: string): never {
  if (error instanceof Error) throw markContractParseError(error, parser);
  throw error;
}

export function withContractParseError<T>(parser: string, parse: () => T): T {
  try {
    return parse();
  } catch (error) {
    rethrowContractParseError(error, parser);
  }
}
