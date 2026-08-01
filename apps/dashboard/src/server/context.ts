import { cache } from "react";
import { headers } from "next/headers";
import { notFound, redirect } from "next/navigation";
import type { RegionCode } from "@aexhq/sdk";

import { readBootstrap, type BootstrapResult, type Organization, type Workspace } from "./bootstrap";
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

export interface WorkspaceContext {
  readonly workspace: Workspace;
  readonly organization: Organization | null;
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
  if (result.kind !== "ready") redirect("/signin");
  const workspace = result.bootstrap.workspaces.find((candidate) => candidate.slug === slug);
  if (!workspace) notFound();
  const regionCode = regionCodeFor(workspace.region);
  if (!regionCode) notFound();
  return {
    workspace,
    organization: result.bootstrap.organizations.find((o) => o.id === workspace.organizationId) ?? null,
    regionCode,
    paused: result.bootstrap.account.status === "paused",
  };
}

export async function requireOrganization(slug: string): Promise<Organization> {
  const result = await currentBootstrap();
  if (result.kind !== "ready") redirect("/signin");
  const organization = result.bootstrap.organizations.find((candidate) => candidate.slug === slug);
  if (!organization) notFound();
  return organization;
}
