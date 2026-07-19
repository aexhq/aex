/**
 * `aex workspaces` — control-plane management of workspaces across your orgs
 * (the PLURAL collection; distinct from the SINGULAR data-plane `workspace`
 * bound to a key). Reached with an ACCOUNT credential.
 *
 *   aex workspaces [list]                          List manageable workspaces (JSON)
 *   aex workspaces create --org <orgId> --name N   Create + reveal its first key ONCE
 *   aex workspaces delete <workspaceId>            Delete a workspace
 *
 * `create` prints the one-time workspace-scoped API key — store it now; the
 * account principal has no other data-plane access to it. If `--org` is omitted
 * the stored `defaultOrgId` (from `aex login`) is used.
 */
import { operations } from "@aexhq/contracts/internal";
import type { CliIO } from "../internal.js";
import {
  type CliExitCode,
  type CommonHostFlags,
  SUCCESS,
  USAGE_ERR,
  describeApiError,
  emitJsonError,
  makeHttpClient,
  resolveControlPlaneHostFlags,
  refuseInsideManagedSession,
  takeOptionFlag
} from "./common.js";

const USAGE =
  "usage: aex workspaces [list] | aex workspaces create --org <orgId> --name <name> | " +
  "aex workspaces delete <workspaceId> [common flags]";

export async function executeWorkspacesCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  if (await refuseInsideManagedSession(io, "workspaces")) return USAGE_ERR;

  const common = await resolveControlPlaneHostFlags(io, argv);
  if (!common.ok) {
    io.stderr(`${common.reason}\n`);
    return USAGE_ERR;
  }
  const sub = common.rest[0];
  if (sub === "create") {
    return runWorkspacesCreate(io, common.rest.slice(1), common.flags, common.defaultOrgId);
  }
  if (sub === "delete") return runWorkspacesDelete(io, common.rest.slice(1), common.flags);
  if (sub === undefined || sub === "list") {
    return runWorkspacesList(io, common.rest.slice(sub === "list" ? 1 : 0), common.flags);
  }

  io.stderr(`unknown workspaces subcommand: ${sub}\n${USAGE}\n`);
  return USAGE_ERR;
}

async function runWorkspacesList(io: CliIO, rest: readonly string[], flags: CommonHostFlags): Promise<CliExitCode> {
  if (rest.length > 0) {
    io.stderr(`unexpected arguments: ${rest.join(" ")}\n${USAGE}\n`);
    return USAGE_ERR;
  }
  const http = makeHttpClient(io, flags);
  try {
    io.stdout(JSON.stringify(await operations.listWorkspaces(http)) + "\n");
    return SUCCESS;
  } catch (err) {
    return emitControlError(io, "workspaces_list_failed", err);
  }
}

async function runWorkspacesCreate(
  io: CliIO,
  argv: readonly string[],
  flags: CommonHostFlags,
  defaultOrgId: string | undefined
): Promise<CliExitCode> {
  const orgFlag = takeOptionFlag(argv, "--org");
  const nameFlag = takeOptionFlag(orgFlag.remaining, "--name");
  const optionError = orgFlag.error ?? nameFlag.error;
  if (optionError) { io.stderr(`${optionError}\n`); return USAGE_ERR; }
  const orgId = orgFlag.value ?? defaultOrgId;
  const name = nameFlag.value;
  if (!orgId || !name || nameFlag.remaining.length > 0) {
    io.stderr("usage: aex workspaces create --org <orgId> --name <name> [common flags]\n");
    if (!orgId) io.stderr("no org — pass --org <orgId> or set a default via `aex login`\n");
    return USAGE_ERR;
  }
  const http = makeHttpClient(io, flags);
  try {
    // The response carries the ONE-TIME workspace key; print it verbatim so the
    // operator can capture it (there is no second reveal).
    io.stdout(JSON.stringify(await operations.createWorkspace(http, { orgId, name })) + "\n");
    return SUCCESS;
  } catch (err) {
    return emitControlError(io, "workspace_create_failed", err);
  }
}

async function runWorkspacesDelete(io: CliIO, argv: readonly string[], flags: CommonHostFlags): Promise<CliExitCode> {
  const workspaceId = argv[0];
  if (!workspaceId || argv.length !== 1) {
    io.stderr("usage: aex workspaces delete <workspaceId> [common flags]\n");
    return USAGE_ERR;
  }
  const http = makeHttpClient(io, flags);
  try {
    await operations.deleteWorkspace(http, workspaceId);
    io.stdout(JSON.stringify({ ok: true, deleted: workspaceId }) + "\n");
    return SUCCESS;
  } catch (err) {
    return emitControlError(io, "workspace_delete_failed", err);
  }
}

function emitControlError(io: CliIO, code: string, err: unknown): CliExitCode {
  const d = describeApiError(err);
  return emitJsonError(io, code, d.message, {
    ...(d.status !== undefined ? { status: d.status } : {}),
    ...(d.remedy ? { remedy: d.remedy } : {})
  });
}
