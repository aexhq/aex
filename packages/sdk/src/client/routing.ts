import { AccountToken, type ParsedCredential, WorkspaceApiKey } from "./credentials.js";
import { AexConfigError } from "../transport/errors.js";

export interface RegionalRoutingOptions {
  readonly regionalBaseUrl?: string;
  readonly workspaceId?: string;
}

function validateBaseUrl(value: string): string {
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    throw new AexConfigError("base URL must be an absolute HTTPS URL");
  }
  if (url.protocol !== "https:" || url.username || url.password || url.search || url.hash) {
    throw new AexConfigError("base URL must be HTTPS without credentials, query, or fragment");
  }
  return url.toString().replace(/\/$/, "");
}

export function resolveRegionalBaseUrl(
  credential: ParsedCredential,
  options: RegionalRoutingOptions,
): string {
  if (options.regionalBaseUrl) return validateBaseUrl(options.regionalBaseUrl);
  if (credential instanceof WorkspaceApiKey) return credential.regionalBaseUrl();
  if (credential instanceof AccountToken && options.workspaceId) {
    throw new AexConfigError(
      "an account token with workspaceId must resolve placement through workspace_get before regional I/O",
    );
  }
  throw new AexConfigError(
    "an account token requires regionalBaseUrl or workspaceId for regional routes",
  );
}

export function resolveCentralBaseUrl(value = "https://api.aex.dev"): string {
  return validateBaseUrl(value);
}
