import { requireOrganization } from "../../../../../src/server/context";
import { AutoTopupPanel, BalancePanel, StatementsPanel } from "../../../../../src/ui/panels/billing";

export const dynamic = "force-dynamic";
export const metadata = { title: "Billing — AEX" };

/**
 * Money lives with the organization, consumption lives with the workspace.
 *
 * The balance appears here and nowhere else; usage appears on the workspace usage
 * page and nowhere else. They answer different questions and share no figure.
 */
export default async function BillingPage({ params }: { params: Promise<{ slug: string }> }) {
  const { slug } = await params;
  const organization = await requireOrganization(slug);

  return (
    <>
      <main id="main" className="frame">
        <div className="stack">
          <div className="page-head">
            <h1>Billing</h1>
            <p className="small muted">
              {organization.name} · you are {organization.callerRole} here
            </p>
          </div>
          <BalancePanel organizationId={organization.id} />
          <AutoTopupPanel organizationId={organization.id} />
          <StatementsPanel organizationId={organization.id} />
        </div>
      </main>
    </>
  );
}
