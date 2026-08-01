import { AexApiError } from "./errors.js";

export const RETRY_POLICY = Object.freeze({
  maxAttempts: 4,
  initialDelayMs: 500,
  maxDelayMs: 20_000,
  maxElapsedMs: 120_000,
});

export interface RetryExecution<T> {
  readonly route: { readonly safeRetry: boolean };
  readonly identity: string;
  readonly streamedBytes?: number;
  readonly random?: () => number;
  readonly sleep?: (delayMs: number) => Promise<void>;
  readonly now?: () => number;
  readonly attempt: (context: { readonly attempt: number; readonly identity: string }) => Promise<T>;
}

export async function executeWithRetry<T>(input: RetryExecution<T>): Promise<T> {
  const now = input.now ?? Date.now;
  const sleep = input.sleep ?? ((delay: number) => new Promise<void>((resolve) => setTimeout(resolve, delay)));
  const random = input.random ?? Math.random;
  const started = now();
  let attempt = 1;
  for (;;) {
    try {
      return await input.attempt({ attempt, identity: input.identity });
    } catch (error) {
      const retryableError = !(error instanceof AexApiError) || error.retryable;
      const mayRetry = input.route.safeRetry
        && (input.streamedBytes ?? 0) === 0
        && retryableError
        && attempt < RETRY_POLICY.maxAttempts;
      if (!mayRetry) throw error;

      const exponential = Math.min(
        RETRY_POLICY.maxDelayMs,
        RETRY_POLICY.initialDelayMs * (2 ** (attempt - 1)),
      );
      const jitter = Math.floor(exponential * Math.max(0, Math.min(1, random())));
      const retryAfter = error instanceof AexApiError ? (error.retryAfterMs ?? 0) : 0;
      const delay = Math.max(jitter, retryAfter);
      if (now() - started + delay > RETRY_POLICY.maxElapsedMs) throw error;
      await sleep(delay);
      attempt += 1;
    }
  }
}
