export function GET(): Response {
  return Response.json({ authenticated: false }, { headers: { "Cache-Control": "private, no-store" } });
}

export function DELETE(): Response {
  return new Response(null, {
    status: 204,
    headers: { "Set-Cookie": "__Host-aex_session=; Max-Age=0; Path=/; HttpOnly; Secure; SameSite=Lax" },
  });
}
