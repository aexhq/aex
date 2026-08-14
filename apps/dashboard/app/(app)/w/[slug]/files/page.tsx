import { requireWorkspace } from "../../../../../src/server/context";
import { FilesPanel } from "../../../../../src/ui/panels/files";

export const dynamic = "force-dynamic";
export const metadata = { title: "Files — AEX" };

export default async function FilesPage({ params }: { params: Promise<{ slug: string }> }) {
  const { slug } = await params;
  const { regionCode } = await requireWorkspace(slug);
  return <div className="stack"><div className="page-head"><h1>Files</h1><p className="small muted">Current files available for session mounts.</p></div><FilesPanel region={regionCode} /></div>;
}
