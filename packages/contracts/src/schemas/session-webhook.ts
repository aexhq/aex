/**
 * Schema for the per-session run-callback registration.
 *
 * Submit-time shape gate only. Delivery-time re-resolution and IP-deny checks
 * live server-side; this rejects the URLs that must never reach that stage.
 */
import * as z from "zod/mini";
import { wireObject } from "./wire.js";

function toUrl(value: string): URL | undefined {
  try {
    return new URL(value);
  } catch {
    return undefined;
  }
}

/**
 * An https callback URL with no userinfo — credentials must not ride in a URL
 * the platform stores and later replays.
 *
 * Each check aborts so the reported failure is the first one a reader would
 * hit, matching the sequential `new URL()` / protocol / userinfo ladder this
 * replaces.
 */
const callbackUrl = z
  .string({ error: "webhook.url must be a non-empty string" })
  .check(
    z.minLength(1, { error: "webhook.url must be a non-empty string", abort: true }),
    z.refine((value: string) => toUrl(value) !== undefined, {
      error: (issue) =>
        `webhook.url must be a valid absolute URL (got ${JSON.stringify(issue.input)})`,
      abort: true
    }),
    z.refine((value: string) => toUrl(value)?.protocol === "https:", {
      error: (issue) =>
        `webhook.url must use https (got ${
          toUrl(issue.input as string)?.protocol.replace(/:$/, "") ?? ""
        })`,
      abort: true
    }),
    z.refine(
      (value: string) => {
        const url = toUrl(value);
        return url?.username === "" && url.password === "";
      },
      { error: "webhook.url must not contain userinfo (user:pass@host)", abort: true }
    )
  );

/** Wire shape of `webhook` on a session submission request. */
export const SessionWebhookSchema = wireObject("webhook", { url: callbackUrl });
