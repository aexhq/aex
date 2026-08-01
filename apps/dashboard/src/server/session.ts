export function dashboardSessionCookie(value: string, maxAgeSeconds: number): string {
  if (!/^aex_ds_[A-Za-z0-9_-]+$/.test(value)) throw new Error("invalid dashboard session credential");
  if (!Number.isSafeInteger(maxAgeSeconds) || maxAgeSeconds <= 0 || maxAgeSeconds > 2_592_000)
    throw new Error("invalid dashboard session lifetime");
  return `__Host-aex_session=${value}; Max-Age=${maxAgeSeconds}; Path=/; HttpOnly; Secure; SameSite=Lax`;
}

export function csrfCookie(value: string, maxAgeSeconds: number): string {
  if (!/^[A-Za-z0-9_-]{43}$/.test(value)) throw new Error("invalid CSRF token");
  return `__Host-aex_csrf=${value}; Max-Age=${maxAgeSeconds}; Path=/; Secure; SameSite=Strict`;
}
