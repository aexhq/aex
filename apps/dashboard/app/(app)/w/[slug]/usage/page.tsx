import { requireWorkspace } from "../../../../../src/server/context";
import { UsagePanel } from "../../../../../src/ui/panels/usage";

export const dynamic = "force-dynamic";
export const metadata = { title: "Usage — AEX" };

export default async function UsagePage({ params }: { params: Promise<{ slug: string }> }) {
  const { slug } = await params;
  const { workspace, organization, regionCode } = await requireWorkspace(slug);

  return (
    <div className="stack">
      <div className="page-head">
        <h1>Usage</h1>
        <p className="small muted">
          What this workspace consumed. What it costs to keep the account funded lives in{" "}
          {organization ? <a href={`/org/${organization.slug}/billing`}>billing</a> : "billing"}.
        </p>
      </div>
      <UsagePanel
        workspaceId={workspace.id}
        region={regionCode}
        billingHref={organization ? `/org/${organization.slug}/billing` : undefined}
      />
    </div>
  );
}
