export interface PreCreateTransportObservation {
  readonly sessionId: string | null;
  readonly threw: string | null;
}

const PRE_CREATE_TRANSPORT_RE =
  /\b(ConnectionRefused|FailedToOpenSocket|ECONNABORTED|ECONNRESET|ECONNREFUSED|EAI_AGAIN|ENETDOWN|ENETRESET|ENETUNREACH|ETIMEDOUT|UND_ERR_[A-Z0-9_]+)\b|socket connection was closed unexpectedly|fetch failed|terminated|unable to connect/i;

export function isPreCreateTransportFailure(observation: PreCreateTransportObservation): boolean {
  return observation.sessionId === null && observation.threw !== null && isPreCreateTransportMessage(observation.threw);
}

export function isPreCreateTransportMessage(message: string): boolean {
  return PRE_CREATE_TRANSPORT_RE.test(message);
}

export async function withPreCreateTransportRetry<T>(
  label: string,
  fn: () => Promise<T>,
  options: { readonly maxAttempts?: number; readonly sleepMs?: (attempt: number) => number } = {}
): Promise<T> {
  const maxAttempts = Math.max(1, Math.floor(options.maxAttempts ?? 3));
  const sleepMs = options.sleepMs ?? ((attempt) => 1_500 * attempt);
  let lastError: unknown;

  for (let attempt = 1; attempt <= maxAttempts; attempt += 1) {
    try {
      return await fn();
    } catch (err) {
      lastError = err;
      if (attempt >= maxAttempts || !isPreCreateTransportMessage(errorText(err))) {
        throw err;
      }

      // Child-script live tests cannot emit a sessionId when the SDK fails before
      // POST /sessions completes, so there is no aex-ops bundle to collect. Retry
      // only the known transport shape and keep all post-create failures single-shot.
      // eslint-disable-next-line no-console
      console.warn(`[${label}] pre-create transport failure; retrying ${attempt + 1}/${maxAttempts}: ${errorText(err)}`);
      await new Promise((resolve) => setTimeout(resolve, sleepMs(attempt)));
    }
  }

  throw lastError;
}

function errorText(err: unknown): string {
  if (err instanceof Error) return err.message;
  return String(err);
}
