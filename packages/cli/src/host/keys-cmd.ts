/**
 * `aex keys` — control-plane management of API keys (data-plane workspace keys
 * AND account PATs). Reached with an ACCOUNT credential.
 *
 *   aex keys [list]                                 List key metadata (never values)
 *   aex keys create <workspaceId> [--name N]        Mint a workspace (data-plane) key
 *   aex keys create --account [--name N]            Mint an account PAT (control-plane)
 *   aex keys delete <keyId>                          Revoke a key
 *
 * `create` prints the freshly minted value ONCE. A PAT cannot mint another PAT
 * (anti-escalation, enforced server-side). If no workspace id is given, the
 * stored `defaultWorkspaceId` (from `aex login`) is used.
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
  takeBooleanFlag,
  takeOptionFlag
} from "./common.js";

const USAGE =
  "usage: aex keys [list] | aex keys create <workspaceId> [--name N] | " +
  "aex keys create --account [--name N] | aex keys delete <keyId> [common flags]";

export async function executeKeysCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  const common = await prepareHostCommand(io, argv, { verb: "keys", auth: "control" });
  if (!common.ok) return common.exit;
  const sub = common.rest[0];
  if (sub === "create") {
    return runKeysCreate(io, common.rest.slice(1), common.flags, common.defaultWorkspaceId);
  }
  if (sub === "delete") return runKeysDelete(io, common.rest.slice(1), common.flags);
  if (sub === undefined || sub === "list") {
    return runKeysList(io, common.rest.slice(sub === "list" ? 1 : 0), common.flags);
  }

  io.stderr(`unknown keys subcommand: ${sub}\n${USAGE}\n`);
  return USAGE_ERR;
}

async function runKeysList(io: CliIO, rest: readonly string[], flags: CommonHostFlags): Promise<CliExitCode> {
  if (rest.length > 0) {
    io.stderr(`unexpected arguments: ${rest.join(" ")}\n${USAGE}\n`);
    return USAGE_ERR;
  }
  const http = makeHttpClient(io, flags);
  try {
    io.stdout(JSON.stringify(await operations.listApiKeys(http)) + "\n");
    return SUCCESS;
  } catch (err) {
    return emitApiError(io, "keys_list_failed", err);
  }
}

async function runKeysCreate(
  io: CliIO,
  argv: readonly string[],
  flags: CommonHostFlags,
  defaultWorkspaceId: string | undefined
): Promise<CliExitCode> {
  const accountFlag = takeBooleanFlag(argv, "--account");
  const nameFlag = takeOptionFlag(accountFlag.remaining, "--name");
  if (nameFlag.error) { io.stderr(`${nameFlag.error}\n`); return USAGE_ERR; }
  const account = accountFlag.present;
  const positionalWorkspaceId = nameFlag.remaining[0];
  if (nameFlag.remaining.length > 1) {
    io.stderr(`unexpected arguments: ${nameFlag.remaining.slice(1).join(" ")}\n${USAGE}\n`);
    return USAGE_ERR;
  }
  const workspaceId = positionalWorkspaceId ?? (account ? undefined : defaultWorkspaceId);
  if (account && workspaceId !== undefined) {
    io.stderr("pass either a <workspaceId> or --account, not both\n");
    return USAGE_ERR;
  }
  if (!account && !workspaceId) {
    io.stderr("usage: aex keys create <workspaceId> [--name N]  |  aex keys create --account [--name N]\n");
    io.stderr("no workspace — pass <workspaceId>, set a default via `aex login`, or use --account\n");
    return USAGE_ERR;
  }
  const name = nameFlag.value;
  const request = account
    ? { account: true as const, ...(name ? { name } : {}) }
    : { workspaceId: workspaceId!, ...(name ? { name } : {}) };
  const http = makeHttpClient(io, flags);
  try {
    // Response carries the ONE-TIME key value; print it verbatim.
    io.stdout(JSON.stringify(await operations.createApiKey(http, request)) + "\n");
    return SUCCESS;
  } catch (err) {
    return emitApiError(io, "key_create_failed", err);
  }
}

async function runKeysDelete(io: CliIO, argv: readonly string[], flags: CommonHostFlags): Promise<CliExitCode> {
  const keyId = argv[0];
  if (!keyId || argv.length !== 1) {
    io.stderr("usage: aex keys delete <keyId> [common flags]\n");
    return USAGE_ERR;
  }
  const http = makeHttpClient(io, flags);
  try {
    await operations.deleteApiKey(http, keyId);
    io.stdout(JSON.stringify({ ok: true, deleted: keyId }) + "\n");
    return SUCCESS;
  } catch (err) {
    return emitApiError(io, "key_delete_failed", err);
  }
}
