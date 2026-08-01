import { requireWorkspace } from "../../../../../src/server/context";
import { SessionsPanel } from "../../../../../src/ui/panels/sessions";

export const dynamic = "force-dynamic";
export const metadata = { title: "Sessions — AEX" };

export default async function SessionsPage({ params }: { params: Promise<{ slug: string }> }) {
  const { slug } = await params;
  const { workspace, organization, regionCode } = await requireWorkspace(slug);

  return (
    <div className="stack">
      <div className="page-head">
        <h1>Sessions</h1>
        <p className="small muted">
          {workspace.name} · <code className="mono">{workspace.region}</code>
        </p>
      </div>
      <SessionsPanel
        slug={slug}
        region={regionCode}
        {...(organization ? { billingHref: `/org/${organization.slug}/billing` } : {})}
      />
    </div>
  );
}
