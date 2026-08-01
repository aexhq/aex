import { ROUTES, type RouteDescriptor, type RouteId } from "@aexhq/sdk";

export const DASHBOARD_ROUTES = ["session_get", "workspace_get"] as const satisfies readonly RouteId[];

export function authorizeDashboardRoute(routeId: string): RouteDescriptor | null {
  if (!(DASHBOARD_ROUTES as readonly string[]).includes(routeId)) return null;
  return ROUTES[routeId as RouteId] ?? null;
}
