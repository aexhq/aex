import { redirect } from "next/navigation";
import type { ReactNode } from "react";

import { currentBootstrap, signInDestination } from "../../src/server/context";
import { AccountBanner } from "../../src/ui/account";
import { AccountMenu } from "../../src/ui/account-menu";
import { Notice } from "../../src/ui/components";

export const dynamic = "force-dynamic";

/**
 * The only gate and the only bootstrap consumer.
 *
 * The authenticated first paint issues exactly one upstream request. Everything
 * below this layout is a panel with its own request, its own deadline and its own
 * abort, so a slow analytics query can never hold up the control surfaces.
 *
 * A failed shell is a failed page: there is no honest way to render the fixed
 * personal workspace without its bootstrap authority.
 */
export default async function AppLayout({ children }: Readonly<{ children: ReactNode }>) {
  const result = await currentBootstrap();
  if (result.kind === "unauthenticated") redirect(await signInDestination());

  if (result.kind === "unavailable") {
    return (
      <>
        <Chrome />
        <main id="main" className="frame">
          <Notice status="serious" title="The account service is not answering" live>
            <p className="small">
              {result.message} Your data is unaffected; the dashboard cannot read your personal
              workspace right now and will not guess at it.
            </p>
            <p className="small muted">
              <code className="mono">{result.code}</code>
            </p>
          </Notice>
        </main>
      </>
    );
  }

  const { bootstrap } = result;
  return (
    <>
      <a className="skip" href="#main">Skip to content</a>
      <header className="banner">
        <div className="frame banner-inner">
          <a className="brand" href="/">AEX</a>
          <span className="crumb">/</span>
          <a href={`/w/${bootstrap.workspace.id}/sessions`}>{bootstrap.workspace.name}</a>
          <span className="spacer" />
          <AccountMenu email={bootstrap.email} userId={bootstrap.userId} />
        </div>
      </header>
      <AccountBanner account={bootstrap.accountState} />
      {children}
      <footer className="frame small muted" style={{ paddingBlock: "var(--aex-space-6)" }}>
        <a href="https://aex.dev/docs">Documentation</a>
        <span aria-hidden="true"> · </span>
        <span>Shell read at {bootstrap.generatedAt}</span>
      </footer>
    </>
  );
}

function Chrome() {
  return (
    <header className="banner">
      <div className="frame banner-inner">
        <a className="brand" href="/">AEX</a>
      </div>
    </header>
  );
}
