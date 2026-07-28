import { SessionConfigValidationError } from "@aexhq/contracts";

const START_COMMAND = "aex start";

export type StartFlag = `--${string}`;

/** Render one CLI-owned validation message from structured command/flag provenance. */
export function startValidationMessage(flag: StartFlag | null, message: string): string {
  return `${START_COMMAND}${flag === null ? "" : ` ${flag}`}: ${message}`;
}

/** Source string supplied to shared validators at the CLI adapter boundary. */
export function startValidationSource(flag: StartFlag): string {
  return `${START_COMMAND} ${flag}`;
}

export class StartValidationError extends Error {
  readonly flag: StartFlag;

  constructor(flag: StartFlag, message: string, options?: { readonly cause?: unknown }) {
    super(startValidationMessage(flag, message), options?.cause === undefined ? undefined : { cause: options.cause });
    this.name = "StartValidationError";
    this.flag = flag;
  }
}

export function startValidationError(
  flag: StartFlag,
  caught: unknown,
  messagePrefix = ""
): StartValidationError {
  if (caught instanceof StartValidationError && messagePrefix.length === 0) return caught;
  const message = caught instanceof Error ? caught.message : String(caught);
  return new StartValidationError(flag, `${messagePrefix}${message}`, { cause: caught });
}

/** Adapt a source-aware shared validator without duplicating the source we supplied. */
export function startSourceValidationError(flag: StartFlag, caught: unknown): StartValidationError {
  if (caught instanceof StartValidationError) return caught;
  const message = caught instanceof Error ? caught.message : String(caught);
  const prefix = `${startValidationSource(flag)}: `;
  return new StartValidationError(
    flag,
    message.startsWith(prefix) ? message.slice(prefix.length) : message,
    { cause: caught }
  );
}

/**
 * Translate only stable shared validation fields. API, transport, filesystem,
 * and other unstructured errors retain their original text.
 */
export function adaptStartSubmissionError(caught: unknown): unknown {
  if (caught instanceof StartValidationError) return caught;
  if (!(caught instanceof SessionConfigValidationError)) return caught;
  const flag = startFlagForValidationField(caught.details.field);
  return flag === undefined
    ? caught
    : new StartValidationError(flag, caught.message, { cause: caught });
}

export function startCommonValidationMessage(argv: readonly string[], message: string): string {
  const removedWorkspace = argv.find((arg): arg is "--workspace" | "--workspace-id" =>
    arg === "--workspace" || arg === "--workspace-id"
  );
  if (removedWorkspace) {
    return startValidationMessage(
      removedWorkspace,
      "workspace is derived from --api-key on the server; drop this flag"
    );
  }
  if (argv.at(-1) === "--aex-url") return startValidationMessage("--aex-url", "requires a value");
  if (argv.at(-1) === "--api-key") return startValidationMessage("--api-key", "requires a value");
  return startValidationMessage("--api-key", message);
}

/** Strip only the exact flag prefix produced by the shared argv parser we invoked. */
export function startParserValidationMessage(flag: StartFlag, message: string): string {
  const colonPrefix = `${flag}: `;
  const spacePrefix = `${flag} `;
  const detail = message.startsWith(colonPrefix)
    ? message.slice(colonPrefix.length)
    : message.startsWith(spacePrefix)
      ? message.slice(spacePrefix.length)
      : message;
  return startValidationMessage(flag, detail);
}

function startFlagForValidationField(field: string): StartFlag | undefined {
  switch (field) {
    case "input":
      return "--prompt";
    default:
      return undefined;
  }
}
