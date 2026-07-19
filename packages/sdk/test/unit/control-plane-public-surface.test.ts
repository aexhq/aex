/**
 * Barrel-surface pin for the WS6 control-plane exports, in the spirit of
 * `slim-public-surface.test.ts`: dropping any of these from `src/index.ts` must
 * FAIL. The control-plane client classes are exported as TYPES ONLY (they are
 * reachable at runtime solely as `client.orgs` / `client.workspaces` /
 * `client.keys` instance fields), and the one-time-reveal results + wire types
 * are pure types — so the guard that fails on removal is the COMPILE-TIME
 * `import type` pin below (a dropped export stops resolving and tsc errors). The
 * runtime `it` mirrors slim's Aex-centric check: the barrel still centers on
 * `Aex`, whose instance actually surfaces the three control-plane clients.
 */
import { describe, expect, it } from "vitest";
import type {
  // Instance-field control-plane client classes (type-only on the barrel).
  OrgsClient,
  WorkspacesClient,
  KeysClient,
  // One-time-reveal results (the minted key is a redacted SecretString).
  NewWorkspaceResult,
  NewApiKeyResult,
  // New control-plane wire types.
  OrgRecord,
  CreateOrgRequest,
  WorkspaceRecord,
  CreateWorkspaceRequest,
  NewWorkspace,
  ApiKeyRecord,
  CreateApiKeyRequest,
  NewApiKey,
  OrgMemberRecord,
  CreateOrgInviteRequest,
  OrgInvite
} from "../../src/index.js";

// COMPILE-TIME pin: every control-plane export is referenced here, so removing
// any one from `src/index.ts` makes this import fail to resolve → tsc error.
type ControlPlaneBarrelSurface =
  | OrgsClient
  | WorkspacesClient
  | KeysClient
  | NewWorkspaceResult
  | NewApiKeyResult
  | OrgRecord
  | CreateOrgRequest
  | WorkspaceRecord
  | CreateWorkspaceRequest
  | NewWorkspace
  | ApiKeyRecord
  | CreateApiKeyRequest
  | NewApiKey
  | OrgMemberRecord
  | CreateOrgInviteRequest
  | OrgInvite;
void (undefined as unknown as ControlPlaneBarrelSurface);

// The one-time-reveal results must keep their key as the redacted `SecretString`
// (a required field), not a bare string — pin it structurally.
const newWorkspaceKeyIsSecret: NewWorkspaceResult["apiKey"] extends string ? false : true = true;
const newApiKeyKeyIsSecret: NewApiKeyResult["apiKey"] extends string ? false : true = true;
void [newWorkspaceKeyIsSecret, newApiKeyKeyIsSecret];

describe("SDK root barrel: control-plane surface", () => {
  it("centers the runtime surface on Aex, whose instance exposes orgs/workspaces/keys", async () => {
    const sdk = await import("../../src/index.js");
    const root = sdk as Record<string, unknown>;
    expect(typeof root["Aex"]).toBe("function");

    // The control-plane clients are type-only exports (reachable via instance
    // fields), so they are NOT runtime values on the barrel — the singular
    // `WorkspaceClient` (data-plane) stays type-only the same way.
    for (const name of ["OrgsClient", "WorkspacesClient", "KeysClient"]) {
      expect(root[name], `${name} is a type-only export, not a runtime barrel value`).toBeUndefined();
    }

    // A constructed client (zero network in the ctor) surfaces the three
    // control-plane clients with their documented methods — the observable proof
    // the pinned types correspond to a real, wired surface.
    const { Aex } = sdk;
    const client = new Aex({ apiKey: "aexu_surface_probe", baseUrl: "https://surface.invalid" });
    expect(client.orgs).toBeDefined();
    expect(client.workspaces).toBeDefined();
    expect(client.keys).toBeDefined();
    // Distinct from the singular data-plane `workspace` context.
    expect(client.workspace).not.toBe(client.workspaces);

    expect(typeof client.orgs.create).toBe("function");
    expect(typeof client.orgs.invite).toBe("function");
    expect(typeof client.workspaces.create).toBe("function");
    expect(typeof client.workspaces.delete).toBe("function");
    expect(typeof client.keys.create).toBe("function");
    expect(typeof client.keys.delete).toBe("function");
  });
});
