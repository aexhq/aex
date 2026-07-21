/**
 * `aex orgs` — control-plane management of the orgs the account principal
 * belongs to. Reached with an ACCOUNT credential (the device-flow token from
 * `aex login`, or an account PAT via `--api-key`), never a workspace key.
 *
 *   aex orgs [list]                     List your orgs (JSON)
 *   aex orgs create --name <name>       Create an org (you become its admin)
 *   aex orgs members <orgId>            List an org's members + pending invites
 *   aex orgs invite <orgId> --email E [--role admin|member]
 */
import { operations } from "@aexhq/contracts/internal";
import type { CliIO } from "../internal.js";
import {
  type CliExitCode,
  type CommonHostFlags,
  SUCCESS,
  USAGE_ERR,
  emitApiError,
  makeHttpClient,
  prepareHostCommand,
  takeOptionFlag
} from "./common.js";

const USAGE =
  "usage: aex orgs [list] | aex orgs create --name <name> | aex orgs members <orgId> | " +
  "aex orgs invite <orgId> --email <email> [--role admin|member] [common flags]";

export async function executeOrgsCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  const common = await prepareHostCommand(io, argv, { verb: "orgs", auth: "control" });
  if (!common.ok) return common.exit;
  const sub = common.rest[0];
  if (sub === "create") return runOrgsCreate(io, common.rest.slice(1), common.flags);
  if (sub === "members") return runOrgsMembers(io, common.rest.slice(1), common.flags);
  if (sub === "invite") return runOrgsInvite(io, common.rest.slice(1), common.flags);
  if (sub === undefined || sub === "list") return runOrgsList(io, common.rest.slice(sub === "list" ? 1 : 0), common.flags);

  io.stderr(`unknown orgs subcommand: ${sub}\n${USAGE}\n`);
  return USAGE_ERR;
}

async function runOrgsList(io: CliIO, rest: readonly string[], flags: CommonHostFlags): Promise<CliExitCode> {
  if (rest.length > 0) {
    io.stderr(`unexpected arguments: ${rest.join(" ")}\n${USAGE}\n`);
    return USAGE_ERR;
  }
  const http = makeHttpClient(io, flags);
  try {
    io.stdout(JSON.stringify(await operations.listOrgs(http)) + "\n");
    return SUCCESS;
  } catch (err) {
    return emitApiError(io, "orgs_list_failed", err);
  }
}

async function runOrgsCreate(io: CliIO, argv: readonly string[], flags: CommonHostFlags): Promise<CliExitCode> {
  const nameFlag = takeOptionFlag(argv, "--name");
  if (nameFlag.error) { io.stderr(`${nameFlag.error}\n`); return USAGE_ERR; }
  const name = nameFlag.value;
  if (!name || nameFlag.remaining.length > 0) {
    io.stderr(`usage: aex orgs create --name <name> [common flags]\n`);
    return USAGE_ERR;
  }
  const http = makeHttpClient(io, flags);
  try {
    io.stdout(JSON.stringify(await operations.createOrg(http, { name })) + "\n");
    return SUCCESS;
  } catch (err) {
    return emitApiError(io, "org_create_failed", err);
  }
}

async function runOrgsMembers(io: CliIO, argv: readonly string[], flags: CommonHostFlags): Promise<CliExitCode> {
  const orgId = argv[0];
  if (!orgId || argv.length !== 1) {
    io.stderr(`usage: aex orgs members <orgId> [common flags]\n`);
    return USAGE_ERR;
  }
  const http = makeHttpClient(io, flags);
  try {
    io.stdout(JSON.stringify(await operations.listOrgMembers(http, orgId)) + "\n");
    return SUCCESS;
  } catch (err) {
    return emitApiError(io, "org_members_failed", err);
  }
}

async function runOrgsInvite(io: CliIO, argv: readonly string[], flags: CommonHostFlags): Promise<CliExitCode> {
  const emailFlag = takeOptionFlag(argv, "--email");
  const roleFlag = takeOptionFlag(emailFlag.remaining, "--role");
  const optionError = emailFlag.error ?? roleFlag.error;
  if (optionError) { io.stderr(`${optionError}\n`); return USAGE_ERR; }
  const orgId = roleFlag.remaining[0];
  const email = emailFlag.value;
  if (!orgId || !email || roleFlag.remaining.length !== 1) {
    io.stderr(`usage: aex orgs invite <orgId> --email <email> [--role admin|member] [common flags]\n`);
    return USAGE_ERR;
  }
  const role = roleFlag.value;
  if (role !== undefined && role !== "admin" && role !== "member") {
    io.stderr(`--role must be admin or member (got: ${role})\n`);
    return USAGE_ERR;
  }
  const http = makeHttpClient(io, flags);
  try {
    const invite = await operations.createOrgInvite(http, orgId, { email, ...(role ? { role } : {}) });
    io.stdout(JSON.stringify(invite) + "\n");
    return SUCCESS;
  } catch (err) {
    return emitApiError(io, "org_invite_failed", err);
  }
}
