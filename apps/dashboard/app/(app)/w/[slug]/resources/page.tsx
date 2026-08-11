import { requireWorkspace } from "../../../../../src/server/context";
import { LimitsPanel, ProviderCredentialsPanel, RegistriesPanel } from "../../../../../src/ui/panels/resources";

export const dynamic = "force-dynamic";
export const metadata = { title: "Resources — AEX" };

export default async function ResourcesPage({ params }: { params: Promise<{ slug: string }> }) {
  const { slug } = await params;
  const { organization, regionCode } = await requireWorkspace(slug);
  const billingHref = organization ? `/org/${organization.slug}/billing` : undefined;

  return (
    <div className="stack">
      <div className="page-head">
        <h1>Resources</h1>
        <p className="small muted">What a session in this workspace is allowed to reach and to mount.</p>
      </div>
      <ProviderCredentialsPanel region={regionCode} billingHref={billingHref} />
      <RegistriesPanel region={regionCode} billingHref={billingHref} />
      <LimitsPanel region={regionCode} billingHref={billingHref} />
    </div>
  );
}
