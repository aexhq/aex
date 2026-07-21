/**
 * SDK-private compatibility adapter for the dependency-light canonical asset
 * bundle owner. Keep callers on this module while the implementation remains
 * single-sourced in `@aexhq/contracts/internal`.
 */
import {
  bundleSkillFiles as bundleSkillFilesInternal,
  bundleToolFiles as bundleToolFilesInternal,
  hashSkillBundle as hashSkillBundleInternal
} from "@aexhq/contracts/internal";
import type {
  BundledSkill,
  BundledTool,
  BundleMeta,
  SkillFiles,
  ToolBundleManifest
} from "@aexhq/contracts/internal";

export { splitSkillBundleMetadata } from "@aexhq/contracts/internal";
export type {
  BundledSkill,
  BundledTool,
  BundleMeta,
  ParsedSkillBundle,
  SkillFiles,
  ToolBundleManifest
} from "@aexhq/contracts/internal";

/** Preserve the established SDK signature while delegating to the shared owner. */
export function bundleSkillFiles(files: SkillFiles, meta?: BundleMeta): BundledSkill {
  return bundleSkillFilesInternal(files, meta);
}

/** Preserve the established SDK signature while delegating to the shared owner. */
export function bundleToolFiles(
  files: SkillFiles,
  manifest: ToolBundleManifest,
  meta?: BundleMeta
): BundledTool {
  return bundleToolFilesInternal(files, manifest, meta);
}

/** Preserve the established SDK signature while delegating to the shared owner. */
export function hashSkillBundle(zipBytes: Uint8Array): Promise<string> {
  return hashSkillBundleInternal(zipBytes);
}
