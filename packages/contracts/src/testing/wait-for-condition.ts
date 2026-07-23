/**
 * Deadline-bounded condition polling for live/integration suites — the
 * replacement for sleep-retry loops (promoted from the bash-bg
 * `readUntilTicks` shape: predicate loop + deadline + rich diagnostic on
 * timeout).
 *
 * Uses REAL timers: do not call under `withFakeClock`.
 */

export interface WaitForConditionOptions {
  /** Total time budget in ms (required — no unbounded waits). */
  readonly deadline: number;
  /** Poll gap in ms between predicate attempts. Default 50. */
  readonly interval?: number;
  /** Names the awaited condition in the timeout error. */
  readonly label?: string;
  /** Extra failure context appended to the timeout error (may be async). */
  readonly diagnostic?: () => string | Promise<string>;
}

/**
 * Poll `predicate` until it returns a truthy value (resolved and returned) or
 * the deadline elapses (rejects with label + attempt count + diagnostic).
 * Predicate exceptions propagate immediately — a broken probe is a failure,
 * not a retry.
 */
export async function waitForCondition<T>(
  predicate: () => T | false | null | undefined | Promise<T | false | null | undefined>,
  options: WaitForConditionOptions
): Promise<T> {
  const { deadline, interval = 50, label, diagnostic } = options;
  if (!Number.isFinite(deadline) || deadline <= 0) {
    throw new TypeError("waitForCondition requires a positive finite deadline (ms budget)");
  }
  if (!Number.isFinite(interval) || interval <= 0) {
    throw new TypeError("waitForCondition requires a positive finite interval");
  }

  const startedAt = Date.now();
  const deadlineAt = startedAt + deadline;
  let attempts = 0;
  for (;;) {
    attempts += 1;
    const value = await predicate();
    if (value) return value;
    const remaining = deadlineAt - Date.now();
    if (remaining <= 0) break;
    await sleep(Math.min(interval, remaining));
  }

  const detail = diagnostic === undefined ? "" : `\n${await diagnostic()}`;
  const condition = label === undefined ? "condition" : label;
  throw new Error(
    `waitForCondition timed out: ${condition} not met within ${deadline}ms (${attempts} attempts)${detail}`
  );
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolvePromise) => setTimeout(resolvePromise, ms));
}
