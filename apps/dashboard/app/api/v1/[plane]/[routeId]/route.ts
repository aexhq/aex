import { executePassthrough, resolvePassthrough } from "../../../../../src/server/passthrough";

export const dynamic = "force-dynamic";

interface Context {
  readonly params: Promise<{ readonly plane: string; readonly routeId: string }>;
}

async function handle(request: Request, context: Context): Promise<Response> {
  const { plane, routeId } = await context.params;
  const body = request.method === "GET" || request.method === "DELETE"
    ? null
    : new Uint8Array(await request.arrayBuffer());
  return executePassthrough(resolvePassthrough(plane, routeId, request, body));
}

export const GET = handle;
export const POST = handle;
export const PUT = handle;
export const DELETE = handle;
