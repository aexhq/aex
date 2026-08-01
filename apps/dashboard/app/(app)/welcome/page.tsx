import { redirect } from "next/navigation";

import { currentBootstrap } from "../../../src/server/context";
import { Card } from "../../../src/ui/components";
import { CreateOrganization, CreateWorkspace } from "../../../src/ui/create";

export const dynamic = "force-dynamic";
export const metadata = { title: "Get started — AEX" };

export default async function Welcome() {
  const result = await currentBootstrap();
  if (result.kind !== "ready") redirect("/signin");
  const { organizations } = result.bootstrap;

  return (
    <main id="main" className="frame">
      <div className="stack">
        <div className="page-head">
          <h1>Get started</h1>
          <p className="muted small">
            An organization owns billing. A workspace owns execution and content, and its region is
            fixed at creation.
          </p>
        </div>

        <div className="grid">
          <Card
            title="1. Organization"
            description={organizations.length === 0 ? "You do not belong to one yet." : "You already have one."}
          >
            {organizations.length === 0 ? (
              <CreateOrganization />
            ) : (
              <ul className="stack-tight small">
                {organizations.map((organization) => (
                  <li key={organization.id}>
                    {organization.name} <span className="muted">· {organization.callerRole}</span>
                  </li>
                ))}
              </ul>
            )}
          </Card>

          <Card
            title="2. Workspace"
            description={organizations.length === 0 ? "Available once an organization exists." : undefined}
          >
            {organizations.length === 0 ? (
              <p className="small muted">Create an organization first.</p>
            ) : (
              <CreateWorkspace organizations={organizations} />
            )}
          </Card>
        </div>
      </div>
    </main>
  );
}
