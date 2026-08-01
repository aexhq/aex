import { requireWorkspace } from "../../../../../src/server/context";
import { ObservabilityPanels } from "../../../../../src/ui/panels/observability";

export const dynamic = "force-dynamic";
export const metadata = { title: "Observability — AEX" };

export default async function ObservabilityPage({ params }: { params: Promise<{ slug: string }> }) {
  const { slug } = await params;
  const { organization, regionCode } = await requireWorkspace(slug);

  return (
    <div className="stack">
      <div className="page-head">
        <h1>Observability</h1>
        <p className="small muted">
          Every answer here carries the coverage it is complete over, and says so when it is not.
        </p>
      </div>
      <ObservabilityPanels
        slug={slug}
        region={regionCode}
        billingHref={organization ? `/org/${organization.slug}/billing` : undefined}
      />
    </div>
  );
}
