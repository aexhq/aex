import {
  ROUTES,
  apiErrorFromResponse,
  type AccountOperationalState,
  type DashboardBootstrap,
  type Workspace,
} from "@aexhq/sdk";

import { readCookie } from "./passthrough";
import { CLIENT_HEADER, transportFor } from "./upstream";

/**
 * The shell's one read.
 *
 * `dashboard_bootstrap_get` is a purpose-built route, not a passthrough member: the
 * authenticated first paint issues exactly one upstream request and every other
 * panel hangs off the payload it returns. It is deliberately absent from
 * `DASHBOARD_ROUTES` so the generic passthrough cannot serve it a second way.
 */

export type { AccountOperationalState, DashboardBootstrap, Workspace };

export type BootstrapResult =
  | { readonly kind: "ready"; readonly bootstrap: DashboardBootstrap }
  | { readonly kind: "unauthenticated" }
  /** `account_state_unavailable` is a real 503: the shell cannot honestly render. */
  | { readonly kind: "unavailable"; readonly code: string; readonly message: string };

export const BOOTSTRAP_TIMEOUT_MS = 5_000;

export async function readBootstrap(cookieHeader: string | null): Promise<BootstrapResult> {
  const credential = readCookie(cookieHeader, "__Host-aex_session");
  if (!credential) return { kind: "unauthenticated" };

  const descriptor = ROUTES.dashboard_bootstrap_get;
  const transport = transportFor(descriptor.plane, null);
  let response;
  try {
    response = await transport.execute<unknown>({
      routeId: descriptor.id,
      method: descriptor.method,
      path: descriptor.path,
      headers: new Headers({
        authorization: `Bearer ${credential}`,
        accept: "application/json",
        "Aex-Client": CLIENT_HEADER,
      }),
      signal: AbortSignal.timeout(BOOTSTRAP_TIMEOUT_MS),
    });
  } catch {
    return { kind: "unavailable", code: "upstream_error", message: "the account service did not answer" };
  }
  if (response.status === 401) return { kind: "unauthenticated" };
  if (response.status >= 400) {
    const error = apiErrorFromResponse(descriptor.id, response.status, response.body, response.headers);
    return { kind: "unavailable", code: error.code, message: error.message };
  }
  return { kind: "ready", bootstrap: response.body as DashboardBootstrap };
}
