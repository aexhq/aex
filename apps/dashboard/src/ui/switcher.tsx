"use client";

import { usePathname } from "next/navigation";
import { useState } from "react";

import type { DashboardBootstrap } from "../server/bootstrap";

function SignOut() {
  const [failed, setFailed] = useState(false);

  async function submit(): Promise<void> {
    const match = /(?:^|;\s*)__Host-aex_csrf=([^;]*)/.exec(document.cookie);
    const response = await fetch("/api/session", {
      method: "DELETE",
      headers: { "x-aex-csrf": match?.[1] ? decodeURIComponent(match[1]) : "" },
      credentials: "same-origin",
    }).catch(() => null);
    if (!response?.ok) {
      setFailed(true);
      return;
    }
    globalThis.location.assign("/signin");
  }

  return (
    <>
      <button type="button" onClick={() => void submit()}>Sign out</button>
      {failed ? <p className="menu-group small" role="alert">You are still signed in — try again.</p> : null}
    </>
  );
}

/**
 * Organization and workspace switching, and the account menu.
 *
 * The only hydrated tree above the fold. It exists as a client component for one
 * reason: the current context is in the URL and a server layout above the route
 * segment cannot read it. The menus themselves are `<details>` — no state, no
 * portal, keyboard-operable by the element's own semantics.
 */
export function ContextSwitcher({ bootstrap }: { bootstrap: DashboardBootstrap }) {
  const pathname = usePathname();
  const workspaceSlug = /^\/w\/([^/]+)/.exec(pathname)?.[1] ?? null;
  const organizationSlug = /^\/org\/([^/]+)/.exec(pathname)?.[1] ?? null;

  const workspace = bootstrap.workspaces.find((candidate) => candidate.slug === workspaceSlug) ?? null;
  const organization = workspace
    ? bootstrap.organizations.find((candidate) => candidate.id === workspace.organizationId) ?? null
    : bootstrap.organizations.find((candidate) => candidate.slug === organizationSlug) ?? null;

  const siblings = organization
    ? bootstrap.workspaces.filter((candidate) => candidate.organizationId === organization.id)
    : bootstrap.workspaces;

  return (
    <>
      <details className="menu">
        <summary aria-label="Switch organization">{organization?.name ?? "Organizations"}</summary>
        <div className="menu-panel">
          <p className="menu-group">Organizations</p>
          {bootstrap.organizations.map((candidate) => (
            <a key={candidate.id} href={`/org/${candidate.slug}/billing`}>
              {candidate.name}
              <span className="small muted"> · {candidate.callerRole}</span>
            </a>
          ))}
          <a href="/welcome">Create an organization</a>
        </div>
      </details>

      <span className="crumb" aria-hidden="true">/</span>

      <details className="menu">
        <summary aria-label="Switch workspace">{workspace?.name ?? "Workspaces"}</summary>
        <div className="menu-panel">
          <p className="menu-group">Workspaces</p>
          {siblings.length === 0 ? <p className="menu-group">None yet</p> : null}
          {siblings.map((candidate) => (
            <a key={candidate.id} href={`/w/${candidate.slug}/sessions`}>
              {candidate.name}
              <span className="small muted"> · {candidate.region}</span>
            </a>
          ))}
          <a href="/welcome">Create a workspace</a>
        </div>
      </details>

      <span className="spacer" />

      <details className="menu">
        <summary aria-label="Account">{bootstrap.email}</summary>
        <div className="menu-panel end">
          <p className="menu-group">Signed in as</p>
          <p className="menu-group mono">{bootstrap.userId}</p>
          <SignOut />
        </div>
      </details>
    </>
  );
}
