import { AexConfigError } from "../transport/errors.js";

const BODY = "([0-9a-hjkmnp-tv-z]{26})";
const SECRET = "([A-Za-z0-9_-]{42}[AEIMQUYcgkosw048])";
// `aex_wk_<region>_<workspace>_<key>_<secret>`: a workspace key names both the
// region that serves it and the workspace it authorizes, so a regional endpoint
// can address every row it must read without a central round trip first.
const WORKSPACE_KEY = new RegExp(
  `^aex_wk_(use1|use2|usw2|apne1|euw1)_${BODY}_${BODY}_${SECRET}$`,
);
const DASHBOARD_SESSION = new RegExp(`^aex_ds_${BODY}_${SECRET}$`);
const CROCKFORD = "0123456789abcdefghjkmnpqrstvwxyz";

export type RegionCode = "use1" | "use2" | "usw2" | "apne1" | "euw1";

const HOSTS: Readonly<Record<RegionCode, string>> = Object.freeze({
  use1: "https://us-east-1.api.aex.dev",
  use2: "https://us-east-2.api.aex.dev",
  usw2: "https://us-west-2.api.aex.dev",
  apne1: "https://ap-northeast-1.api.aex.dev",
  euw1: "https://eu-west-1.api.aex.dev",
});

abstract class Credential {
  readonly #value: string;

  protected constructor(value: string) {
    this.#value = value;
  }

  authorizationHeader(): string {
    return `Bearer ${this.#value}`;
  }

  redacted(): string {
    return `${this.#value.slice(0, this.#value.indexOf("_", 7) + 1)}[redacted]`;
  }

  toJSON(): string {
    return this.redacted();
  }
}

export class WorkspaceApiKey extends Credential {
  readonly #region: RegionCode;
  readonly #workspaceId: string;

  private constructor(value: string, region: RegionCode, workspaceId: string) {
    super(value);
    this.#region = region;
    this.#workspaceId = workspaceId;
  }

  static parse(value: string): WorkspaceApiKey {
    const match = WORKSPACE_KEY.exec(value);
    if (!match || !isUuid7Body(match[2] ?? "") || !isUuid7Body(match[3] ?? "")) {
      throw new AexConfigError(
        "invalid credential: expected an aex_wk_ workspace key with UUIDv7 workspace and key ids",
      );
    }
    return new WorkspaceApiKey(value, match[1] as RegionCode, `wsp_${match[2]}`);
  }

  regionCode(): RegionCode {
    return this.#region;
  }

  /** The workspace the key authorizes, as the public prefixed identifier. */
  workspaceId(): string {
    return this.#workspaceId;
  }

  regionalBaseUrl(): string {
    return HOSTS[this.#region];
  }
}

export class DashboardSession extends Credential {
  private constructor(value: string) {
    super(value);
  }

  static parse(value: string): DashboardSession {
    const match = DASHBOARD_SESSION.exec(value);
    if (!match || !isUuid7Body(match[1] ?? "")) {
      throw new AexConfigError("invalid credential: expected an aex_ds_ dashboard session");
    }
    return new DashboardSession(value);
  }
}

export type ParsedCredential = WorkspaceApiKey | DashboardSession;

export function parseCredential(value: string): ParsedCredential {
  if (value.startsWith("aex_wk_")) return WorkspaceApiKey.parse(value);
  if (value.startsWith("aex_ds_")) return DashboardSession.parse(value);
  throw new AexConfigError("invalid credential: expected prefix aex_wk_ or aex_ds_");
}

export function regionalHost(region: RegionCode): string {
  return HOSTS[region];
}

function isUuid7Body(body: string): boolean {
  let value = 0n;
  for (const character of body) {
    const digit = CROCKFORD.indexOf(character);
    if (digit < 0) return false;
    value = (value << 5n) | BigInt(digit);
  }
  if (value >= (1n << 128n)) return false;
  const bytes = new Uint8Array(16);
  for (let index = bytes.length - 1; index >= 0; index -= 1) {
    bytes[index] = Number(value & 0xffn);
    value >>= 8n;
  }
  return (bytes[6]! & 0xf0) === 0x70 && (bytes[8]! & 0xc0) === 0x80;
}
