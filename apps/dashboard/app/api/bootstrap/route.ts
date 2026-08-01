import { readBootstrap } from "../../../src/server/bootstrap";

export const dynamic = "force-dynamic";

const PRIVATE = { "Cache-Control": "private, no-store" } as const;

export async function GET(request: Request): Promise<Response> {
  const result = await readBootstrap(request.headers.get("cookie"));
  if (result.kind === "unauthenticated") {
    return Response.json(
      { error: { code: "unauthenticated", message: "browser session required", retryable: false } },
      { status: 401, headers: PRIVATE },
    );
  }
  if (result.kind === "unavailable") {
    return Response.json(
      { error: { code: result.code, message: result.message, retryable: true } },
      { status: 503, headers: PRIVATE },
    );
  }
  return Response.json(result.bootstrap, { headers: PRIVATE });
}
