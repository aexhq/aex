"use client";

import { useState, type FormEvent } from "react";

import type { Organization } from "../server/bootstrap";
import { PanelFallback } from "./components";
import { REGIONS } from "./regions";
import { submit } from "./client";
import type { PanelState } from "./panel";

type Result = PanelState<unknown> | null;

/**
 * First sign-in creates no organization, no workspace and no key — the wire
 * contract says so. These two forms are therefore the only way out of an empty
 * account, and they are the only reason `organization_create` and
 * `workspace_create` are on the passthrough allowlist.
 */
export function CreateOrganization() {
  const [result, setResult] = useState<Result>(null);
  const [busy, setBusy] = useState(false);

  async function onSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const name = new FormData(event.currentTarget).get("name");
    if (typeof name !== "string" || name.trim().length === 0) return;
    setBusy(true);
    const state = await submit<Organization>("organization_create", { body: { name: name.trim() } });
    setBusy(false);
    setResult(state);
    if (state.kind === "ready") globalThis.location.reload();
  }

  return (
    <form className="stack" onSubmit={onSubmit}>
      <label className="field">
        <span>Organization name</span>
        <input name="name" required maxLength={120} autoComplete="organization" />
      </label>
      <p>
        <button type="submit" className="button" data-variant="primary" disabled={busy}>
          {busy ? "Creating…" : "Create organization"}
        </button>
      </p>
      {result && result.kind !== "ready" ? <PanelFallback state={result} /> : null}
    </form>
  );
}

export function CreateWorkspace({ organizations }: { organizations: readonly Organization[] }) {
  const [result, setResult] = useState<Result>(null);
  const [busy, setBusy] = useState(false);

  async function onSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const form = new FormData(event.currentTarget);
    const name = form.get("name");
    const organizationId = form.get("organizationId");
    const region = form.get("region");
    if (typeof name !== "string" || typeof organizationId !== "string" || typeof region !== "string") return;
    setBusy(true);
    const state = await submit<{ slug: string }>("workspace_create", {
      body: { name: name.trim(), organizationId, region },
    });
    setBusy(false);
    setResult(state);
    if (state.kind === "ready") globalThis.location.assign(`/w/${state.data.slug}/sessions`);
  }

  return (
    <form className="stack" onSubmit={onSubmit}>
      <label className="field">
        <span>Workspace name</span>
        <input name="name" required maxLength={120} />
      </label>
      <label className="field">
        <span>Organization</span>
        <select name="organizationId" required>
          {organizations.map((organization) => (
            <option key={organization.id} value={organization.id}>{organization.name}</option>
          ))}
        </select>
      </label>
      <label className="field">
        <span>Region — immutable once chosen</span>
        <select name="region" required defaultValue="eu-west-1">
          {REGIONS.map((row) => (
            <option key={row.code} value={row.region}>{row.region}</option>
          ))}
        </select>
      </label>
      <p>
        <button type="submit" className="button" data-variant="primary" disabled={busy}>
          {busy ? "Creating…" : "Create workspace"}
        </button>
      </p>
      {result && result.kind !== "ready" ? <PanelFallback state={result} /> : null}
    </form>
  );
}
