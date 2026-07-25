/**
 * Numeric wire primitives shared by the request schemas.
 *
 * Each takes the wire path it is mounted at so the message names the field the
 * caller actually sent, matching the `optionalPositiveInt(value, "limits.maxTurns")`
 * style these replace.
 */
import * as z from "zod/mini";

/**
 * A positive safe integer — a count. Rejects non-numbers, non-integers,
 * unsafe integers, and `<= 0`.
 */
export function positiveInt(path: string) {
  const message = `${path} must be a positive safe integer`;
  return z
    .number({ error: message })
    .check(
      z.refine(
        (value: number) => Number.isSafeInteger(value) && value > 0,
        { error: message, abort: true }
      )
    );
}

/**
 * A positive finite number — an amount, so fractional values are allowed
 * (`maxSpendUsd: 2.5`). Rejects non-numbers, NaN/Infinity, and `<= 0`.
 */
export function positiveNumber(path: string) {
  const message = `${path} must be a positive finite number`;
  return z
    .number({ error: message })
    .check(
      z.refine(
        (value: number) => Number.isFinite(value) && value > 0,
        { error: message, abort: true }
      )
    );
}
