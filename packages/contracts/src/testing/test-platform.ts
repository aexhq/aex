/**
 * B6 OS-capability helper: named capabilities resolved ONCE per process,
 * loudly (one `[test-platform]` line on first use), consumed as visible
 * `it.skipIf(!caps.x)` guards — never OS ternaries around `it` and never
 * `catch { return }` probe swallowing.
 */
import { chmodSync, mkdtempSync, rmSync, statSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

export interface TestPlatformCapabilities {
  /** File symlinks can be created (Windows requires Developer Mode or admin). */
  readonly canSymlinkFiles: boolean;
  /** True directory symlinks (type "dir") can be created — junctions excluded. */
  readonly canSymlinkDirectories: boolean;
  /** NTFS junctions can be created (a win32-only concept; always false elsewhere). */
  readonly canJunction: boolean;
  /** POSIX permission bits round-trip through chmod/stat on this filesystem. */
  readonly posixFileModes: boolean;
  /**
   * A `$!` pid recorded by a spawned bash is visible to `process.kill`.
   * False under Git Bash on Windows, where `$!` is an MSYS-namespace pid.
   */
  readonly spawnedPidsVisible: boolean;
  /** Windows drive-letter path semantics (`C:relative`, `D:\\outside`) apply. */
  readonly windowsDrivePaths: boolean;
}

let resolved: TestPlatformCapabilities | undefined;

/** Resolve (memoized) the capability record, logging the outcome on first use. */
export function testPlatformCapabilities(): TestPlatformCapabilities {
  if (resolved !== undefined) return resolved;

  const isWindows = process.platform === "win32";
  const probeRoot = mkdtempSync(join(tmpdir(), "aex-test-platform-"));
  let canSymlinkFiles = false;
  let canSymlinkDirectories = false;
  let canJunction = false;
  let posixFileModes = false;
  try {
    const targetFile = join(probeRoot, "target.txt");
    writeFileSync(targetFile, "probe", "utf8");

    canSymlinkFiles = probe(() => symlinkSync(targetFile, join(probeRoot, "file-link"), "file"));
    canSymlinkDirectories = probe(() => symlinkSync(probeRoot, join(probeRoot, "dir-link"), "dir"));
    // Node treats "junction" as a plain dir symlink off Windows — the
    // capability is meaningful (and probed) only where junctions exist.
    canJunction =
      isWindows && probe(() => symlinkSync(probeRoot, join(probeRoot, "junction-link"), "junction"));
    posixFileModes = probe(() => {
      chmodSync(targetFile, 0o741);
      if ((statSync(targetFile).mode & 0o777) !== 0o741) {
        throw new Error("mode did not round-trip");
      }
    });
  } finally {
    rmSync(probeRoot, { recursive: true, force: true });
  }

  resolved = Object.freeze({
    canSymlinkFiles,
    canSymlinkDirectories,
    canJunction,
    posixFileModes,
    // Known-environment facts, not probes (matches the bash-bg PID_VISIBLE probe).
    spawnedPidsVisible: !isWindows,
    windowsDrivePaths: isWindows
  });

  console.error(
    `[test-platform] resolved capabilities (${process.platform}/${process.arch}): ` +
      Object.entries(resolved)
        .map(([name, value]) => `${name}=${value}`)
        .join(" ")
  );
  return resolved;
}

/** Lazy view for `it.skipIf(!caps.x)` — resolves the full record at first property access. */
export const caps: TestPlatformCapabilities = Object.freeze({
  get canSymlinkFiles() {
    return testPlatformCapabilities().canSymlinkFiles;
  },
  get canSymlinkDirectories() {
    return testPlatformCapabilities().canSymlinkDirectories;
  },
  get canJunction() {
    return testPlatformCapabilities().canJunction;
  },
  get posixFileModes() {
    return testPlatformCapabilities().posixFileModes;
  },
  get spawnedPidsVisible() {
    return testPlatformCapabilities().spawnedPidsVisible;
  },
  get windowsDrivePaths() {
    return testPlatformCapabilities().windowsDrivePaths;
  }
});

function probe(attempt: () => void): boolean {
  try {
    attempt();
    return true;
  } catch {
    // A failed probe RESOLVES the capability to false — this is the one place
    // a catch may absorb the error, because the result is reported loudly and
    // consumed as a visible skip condition rather than silently passing a test.
    return false;
  }
}
