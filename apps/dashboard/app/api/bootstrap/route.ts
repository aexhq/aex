export function GET(): Response {
  return Response.json(
    { error: { code: "unauthenticated", message: "browser session required", retryable: false } },
    { status: 401, headers: { "Cache-Control": "private, no-store" } },
  );
}
