/**
 * antpath CLI — internal IO surface.
 *
 * This module exports the dependency-injection types and a thin
 * `createCli()` factory that the entrypoint (`cli.ts`) calls with real
 * IO. Tests import this module directly and inject fakes — there are
 * NO `process.env.ANTPATH_*` reads in the production code paths
 * (mechanical test in Phase 9 enforces this against the built bundle).
 */
import type {
  ProxyIndexFile,
  ProxyIndexEntry,
  ProxyMethod,
  ProxyResponseMode
} from "@antpath/contracts";

/**
 * Manifest file path inside the run container (always present).
 *
 * The runner writes the manifest to this fixed path before invoking
 * in-container `antpath` commands.
 */
export const ANTPATH_INDEX_PATH = "/mnt/session/uploads/antpath/index.json";

/** Per-run bearer file path (only present when proxyEndpoints declared). */
export const ANTPATH_RUN_TOKEN_PATH = "/mnt/session/uploads/antpath/run-token";

/**
 * IO surface the CLI depends on. The production entrypoint passes real
 * implementations; tests pass fakes. New IO methods must be additive
 * and orthogonal: a fake that lacks a method should still satisfy the
 * subset of subcommands that don't touch that method.
 */
export interface CliIO {
  readonly readFile: (path: string) => Promise<string>;
  readonly writeFile: (path: string, data: Uint8Array) => Promise<void>;
  readonly fetchImpl: typeof fetch;
  readonly stdout: (chunk: string) => void;
  readonly stderr: (chunk: string) => void;
  readonly exit: (code: number) => void;
  readonly argv: readonly string[];
  /** Current working directory; used to resolve relative `--out` paths. */
  readonly cwd: () => string;
  /**
   * Walk a directory and return every regular-file entry beneath it
   * (recursive). Used only by the internal `outputs sync` subcommand
   * that the worker invokes from inside a managed run container at
   * session terminal. Returns `null` (not throws) when the directory
   * does not exist or is unreadable — `outputs sync` records the
   * miss in its structured output and continues to the next dir.
   */
  readonly walkDirectory?: (root: string) => Promise<readonly OutputsSyncFileEntry[] | null>;
}

export interface OutputsSyncFileEntry {
  /** Absolute path inside the container. */
  readonly path: string;
  /** File size in bytes. */
  readonly sizeBytes: number;
}

/** Re-exported helpers used by CLI consumers. */
export type { ProxyIndexFile, ProxyIndexEntry, ProxyMethod, ProxyResponseMode };
