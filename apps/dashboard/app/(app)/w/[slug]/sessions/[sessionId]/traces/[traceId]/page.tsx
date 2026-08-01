import { requireWorkspace } from "../../../../../../../../src/server/context";
import { TracePanel } from "../../../../../../../../src/ui/panels/trace";

export const dynamic = "force-dynamic";
export const metadata = { title: "Trace — AEX" };

export default async function TracePage({
  params,
}: {
  params: Promise<{ slug: string; sessionId: string; traceId: string }>;
}) {
  const { slug, sessionId, traceId } = await params;
  const { organization, regionCode } = await requireWorkspace(slug);

  return (
    <div className="stack">
      <div className="page-head">
        <h1>Trace</h1>
        <p className="small muted">
          <a href={`/w/${slug}/sessions/${sessionId}`}>Back to the session</a>
        </p>
      </div>
      <TracePanel
        region={regionCode}
        sessionId={sessionId}
        traceId={traceId}
        billingHref={organization ? `/org/${organization.slug}/billing` : undefined}
      />
    </div>
  );
}
