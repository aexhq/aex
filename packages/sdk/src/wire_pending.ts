/**
 * TODO(cross-stream): `packages/sdk/src/generated/` does not exist. The contracts
 * stream owes the generated TypeScript route and error vocabulary; until it lands this
 * file is the only definition.
 */

export type RouteId =
  | "session_create"
  | "session_get"
  | "workspace_get";

export interface RouteDescriptor {
  readonly id: RouteId;
  readonly method: "GET" | "POST";
  readonly path: string;
  readonly plane: "central" | "regional";
  readonly safeRetry: boolean;
  readonly idempotency: "none" | "idempotency_key";
}

export const ROUTES: Readonly<Record<RouteId, RouteDescriptor>> = Object.freeze({
  session_create: {
    id: "session_create",
    method: "POST",
    path: "/api/sessions",
    plane: "regional",
    safeRetry: false,
    idempotency: "idempotency_key",
  },
  session_get: {
    id: "session_get",
    method: "GET",
    path: "/api/sessions/{sessionId}",
    plane: "regional",
    safeRetry: true,
    idempotency: "none",
  },
  workspace_get: {
    id: "workspace_get",
    method: "GET",
    path: "/api/workspaces/{workspaceId}",
    plane: "central",
    safeRetry: true,
    idempotency: "none",
  },
});

export type ErrorClass =
  | "auth"
  | "not_found"
  | "conflict"
  | "precondition"
  | "validation"
  | "quota"
  | "state"
  | "unavailable"
  | "internal";

export type AexErrorCode = string;
export const ERROR_METADATA: Readonly<Record<string, { readonly class: ErrorClass }>> =
  Object.freeze({
    unauthenticated: { class: "auth" },
    forbidden: { class: "auth" },
    insufficient_scope: { class: "auth" },
    not_found: { class: "not_found" },
    conflict: { class: "conflict" },
    precondition_failed: { class: "precondition" },
    invalid_request: { class: "validation" },
    rate_limited: { class: "quota" },
    invalid_state: { class: "state" },
    upstream_unavailable: { class: "unavailable" },
    internal_error: { class: "internal" },
  });
