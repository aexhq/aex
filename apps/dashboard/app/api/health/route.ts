export function GET(): Response {
  return Response.json(
    { status: "ok", build: process.env.VERCEL_GIT_COMMIT_SHA ?? "development" },
    { headers: { "Cache-Control": "no-store" } },
  );
}
