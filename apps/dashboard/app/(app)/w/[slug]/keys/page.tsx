import { requireWorkspace } from "../../../../../src/server/context";
import { ApiKeysPanel } from "../../../../../src/ui/panels/keys";

export const dynamic = "force-dynamic";
export const metadata = { title: "API keys — AEX" };

export default async function KeysPage({ params }: { params: Promise<{ slug: string }> }) {
  const { slug } = await params;
  const { workspace, organization } = await requireWorkspace(slug);

  return (
    <div className="stack">
      <div className="page-head">
        <h1>API keys</h1>
        <p className="small muted">
          A key authorizes one workspace, in <code className="mono">{workspace.region}</code>, with the
          scopes it was minted with.
        </p>
      </div>
      <ApiKeysPanel
        workspaceId={workspace.id}
        billingHref={organization ? `/org/${organization.slug}/billing` : undefined}
      />
    </div>
  );
}
