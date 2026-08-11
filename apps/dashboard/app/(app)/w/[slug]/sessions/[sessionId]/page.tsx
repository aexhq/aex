import { requireWorkspace } from "../../../../../../src/server/context";
import {
  LiveFilesPanel,
  MessagesPanel,
  SessionEventsPanel,
  SessionHeader,
} from "../../../../../../src/ui/panels/session-detail";

export const dynamic = "force-dynamic";
export const metadata = { title: "Session — AEX" };

/**
 * Four panels, four independent requests, four independent failures.
 *
 * Nothing here waits on anything else: the events query may take the full analytics
 * deadline while the other panels remain usable. Messages are the durable
 * conversation surface; internal execution identities are deliberately absent.
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
      <MessagesPanel {...scope} />
      <SessionEventsPanel {...scope} />
      <LiveFilesPanel {...scope} />
    </div>
  );
}
