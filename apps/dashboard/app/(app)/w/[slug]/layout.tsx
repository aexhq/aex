import type { ReactNode } from "react";

import { requireWorkspace } from "../../../../src/server/context";
import { WorkspaceNav } from "../../../../src/ui/nav";

export const dynamic = "force-dynamic";

export default async function WorkspaceLayout({
  children,
  params,
}: Readonly<{ children: ReactNode; params: Promise<{ slug: string }> }>) {
  const { slug } = await params;
  const { organization } = await requireWorkspace(slug);
  return (
    <>
      <WorkspaceNav slug={slug} organizationSlug={organization?.slug ?? null} />
      <main id="main" className="frame">{children}</main>
    </>
  );
}
