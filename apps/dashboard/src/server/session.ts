export const MAX_SESSION_SECONDS = 2_592_000;

export function dashboardSessionCookie(value: string, maxAgeSeconds: number): string {
  if (!/^aex_ds_[A-Za-z0-9_-]+$/.test(value)) throw new Error("invalid dashboard session credential");
  if (!Number.isSafeInteger(maxAgeSeconds) || maxAgeSeconds <= 0 || maxAgeSeconds > MAX_SESSION_SECONDS)
    throw new Error("invalid dashboard session lifetime");
  return `__Host-aex_session=${value}; Max-Age=${maxAgeSeconds}; Path=/; HttpOnly; Secure; SameSite=Lax`;
}

export function csrfCookie(value: string, maxAgeSeconds: number): string {
  if (!/^[A-Za-z0-9_-]{43}$/.test(value)) throw new Error("invalid CSRF token");
  if (!Number.isSafeInteger(maxAgeSeconds) || maxAgeSeconds <= 0 || maxAgeSeconds > MAX_SESSION_SECONDS)
    throw new Error("invalid CSRF token lifetime");
  return `__Host-aex_csrf=${value}; Max-Age=${maxAgeSeconds}; Path=/; Secure; SameSite=Strict`;
}

export function signInStateCookie(value: string, maxAgeSeconds: number): string {
  if (!/^[A-Za-z0-9_-]{1,2048}$/.test(value)) throw new Error("invalid sign-in state");
  if (!Number.isSafeInteger(maxAgeSeconds) || maxAgeSeconds <= 0 || maxAgeSeconds > 3_600)
    throw new Error("invalid sign-in state lifetime");
  return `__Host-aex_signin=${value}; Max-Age=${maxAgeSeconds}; Path=/; HttpOnly; Secure; SameSite=Lax`;
}

export const CLEARED_SESSION_COOKIE =
  "__Host-aex_session=; Max-Age=0; Path=/; HttpOnly; Secure; SameSite=Lax";
export const CLEARED_CSRF_COOKIE =
  "__Host-aex_csrf=; Max-Age=0; Path=/; Secure; SameSite=Strict";
export const CLEARED_SIGN_IN_STATE_COOKIE =
  "__Host-aex_signin=; Max-Age=0; Path=/; HttpOnly; Secure; SameSite=Lax";
