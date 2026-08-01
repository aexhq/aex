export type ArtifactSelection =
  | { readonly kind: "workspace" }
  | { readonly kind: "files"; readonly sdkTarball: string; readonly cliArchive: string }
  | { readonly kind: "versions"; readonly sdkVersion: string; readonly cliVersion: string };

export function resolveArtifactSelection(env: Readonly<Record<string, string | undefined>>): ArtifactSelection {
  const sdkTarball = env.AEX_USER_TEST_SDK_TARBALL;
  const cliArchive = env.AEX_USER_TEST_CLI_ARCHIVE;
  const sdkVersion = env.AEX_USER_TEST_SDK_VERSION;
  const cliVersion = env.AEX_USER_TEST_CLI_VERSION;
  const files = sdkTarball !== undefined || cliArchive !== undefined;
  const versions = sdkVersion !== undefined || cliVersion !== undefined;
  if (files && versions) throw new Error("artifact file and version selection families are mutually exclusive");
  if (files) {
    if (!sdkTarball || !cliArchive) throw new Error("SDK tarball and CLI archive must be supplied together");
    return { kind: "files", sdkTarball, cliArchive };
  }
  if (versions) {
    if (!sdkVersion || !cliVersion) throw new Error("SDK and CLI versions must be supplied together");
    const exact = /^0\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)$/;
    if (!exact.test(sdkVersion) || !exact.test(cliVersion)) throw new Error("artifact versions must be exact 0.x semver");
    if (sdkVersion !== cliVersion) throw new Error("SDK and CLI versions must be identical");
    return { kind: "versions", sdkVersion, cliVersion };
  }
  return { kind: "workspace" };
}
