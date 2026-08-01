import { requireWorkspace } from "../../../../../../src/server/context";
import {
  ApprovalsPanel,
  PersistedFilesPanel,
  RunsPanel,
  SessionEventsPanel,
  SessionHeader,
} from "../../../../../../src/ui/panels/session-detail";

export const dynamic = "force-dynamic";
export const metadata = { title: "Session — AEX" };

/**
 * Five panels, five independent requests, five independent failures.
 *
 * Nothing here waits on anything else: the events query may take the full analytics
 * deadline and the approvals control stays usable throughout. There is deliberately
 * no message transcript — the run list and the event series already carry the
 * execution story, and a third rendering of the same turns would be a third thing
 * to keep true.
 */
export default async function SessionPage({
  params,
}: {
  params: Promise<{ slug: string; sessionId: string }>;
}) {
  const { slug, sessionId } = await params;
  const { organization, regionCode } = await requireWorkspace(slug);
  const billingHref = organization ? `/org/${organization.slug}/billing` : undefined;
  const scope = { slug, region: regionCode, sessionId, billingHref };

  return (
    <div className="stack">
      <div className="page-head">
        <h1>Session</h1>
        <p className="small muted mono">{sessionId}</p>
      </div>
      <SessionHeader {...scope} />
      <RunsPanel {...scope} />
      <ApprovalsPanel {...scope} />
      <SessionEventsPanel {...scope} />
      <PersistedFilesPanel {...scope} />
    </div>
  );
}
