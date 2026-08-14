import { requireWorkspace } from "../../../../../../src/server/context";
import {
  MessagesPanel,
  SessionHeader,
  TelemetryPanel,
} from "../../../../../../src/ui/panels/session-detail";

export const dynamic = "force-dynamic";
export const metadata = { title: "Session — AEX" };

/**
 * Three panels, three independent requests, three independent failures.
 *
 * Nothing here waits on anything else. Messages are the durable conversation
 * surface; internal execution identities are deliberately absent.
 */
export default async function SessionPage({
  params,
}: {
  params: Promise<{ slug: string; sessionId: string }>;
}) {
  const { slug, sessionId } = await params;
  const { regionCode } = await requireWorkspace(slug);
  const billingHref = "/billing";
  const scope = { slug, region: regionCode, sessionId, billingHref };

  return (
    <div className="stack">
      <div className="page-head">
        <h1>Session</h1>
        <p className="small muted mono">{sessionId}</p>
      </div>
      <SessionHeader {...scope} />
      <MessagesPanel {...scope} />
      <TelemetryPanel {...scope} />
    </div>
  );
}
