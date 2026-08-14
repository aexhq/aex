import { cache } from "react";
import { headers } from "next/headers";
import { notFound, redirect } from "next/navigation";
import type { RegionCode } from "@aexhq/sdk";

import { readBootstrap, type BootstrapResult, type Workspace } from "./bootstrap";
import { RETURN_HEADER, signInPath } from "./return-to";
import { regionCodeFor } from "../ui/regions";

/**
 * One bootstrap per request, whatever asks for it.
 *
 * `cache()` deduplicates within a single render pass, so the layout, the page and
 * any nested segment all read the same payload and the authenticated first paint
 * still issues exactly one upstream request.
 */
export const currentBootstrap = cache(async (): Promise<BootstrapResult> => {
  const requestHeaders = await headers();
  return readBootstrap(requestHeaders.get("cookie"));
});

export async function signInDestination(): Promise<string> {
  const requestHeaders = await headers();
  return signInPath(requestHeaders.get(RETURN_HEADER));
}

export interface WorkspaceContext {
  readonly workspace: Workspace;
  readonly regionCode: RegionCode;
  readonly paused: boolean;
}

/**
 * Slug resolution is a lookup over the bootstrap payload, never a second call. A
 * slug that is not in the payload is a 404 by definition: the payload is the whole
 * set of workspaces this person can reach.
 */
export async function requireWorkspace(slug: string): Promise<WorkspaceContext> {
  const result = await currentBootstrap();
  if (result.kind !== "ready") redirect(await signInDestination());
  const workspace = result.bootstrap.workspace;
  if (workspace.id !== slug) notFound();
  const regionCode = regionCodeFor(workspace.region);
  if (!regionCode) notFound();
  return {
    workspace,
    regionCode,
    paused: result.bootstrap.accountState.status === "paused",
  };
}
