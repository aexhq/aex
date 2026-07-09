/**
 * aex CLI — internal IO surface.
 *
 * This module exports the dependency-injection types and a thin
 * `createCli()` factory that the entrypoint (`cli.ts`) calls with real
 * IO. Tests import this module directly and inject fakes — there are
 * NO `process.env.AEX_*` reads in the production code paths
 * (mechanical test in Phase 9 enforces this against the built bundle).
 */
import type {
  WebSocketLike
} from "@aexhq/contracts";

/**
 * Manifest file path inside the session container (always present).
 *
 * The runner writes the manifest to this fixed path before invoking
 * in-container `aex` commands.
 */
export const AEX_INDEX_PATH = "/mnt/session/uploads/aex/index.json";

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
   * that the hosted runtime invokes from inside a managed session container at
   * session terminal. Returns `null` (not throws) when the directory
   * does not exist or is unreadable — `outputs sync` records the
   * miss in its structured output and continues to the next dir.
   */
  readonly walkDirectory?: (root: string) => Promise<readonly OutputsSyncFileEntry[] | null>;
  /**
   * Persistent CLI config store (token + default `--aex-url`). Wired ONLY by
   * the host entrypoint (`cli.ts`), which is the single file allowed to touch
   * the OS/home/env to resolve the config path — keeping the pure command layer
   * env-free and the `no-env-vars` bundle grep clean. Optional: in-container and
   * test fakes omit it, in which case `aex login` is unavailable and host verbs
   * fall back to requiring `--api-key`.
   */
  readonly configStore?: CliConfigStore;
  /**
   * Open a live coordinator WebSocket. Wired ONLY by `cli.ts` from the global
   * `WebSocket` (Bun / Node ≥ 22). Optional: omitted when no global `WebSocket`
   * exists (older Node) or in fakes — `aex tail`/`aex inspect` then emit an
   * actionable error. Tests inject a fake socket.
   */
  readonly webSocketFactory?: (url: string) => WebSocketLike;
  /**
   * Register a process-signal handler. Wired ONLY by `cli.ts`
   * (`process.on(sig, handler)`); the long-lived stream verbs use it for
   * graceful Ctrl-C. Optional: fakes/other verbs omit it.
   */
  readonly onSignal?: (signal: "SIGINT", handler: () => void) => void;
}

/**
 * Persisted CLI config (written by `aex login`). Forward/back compatible:
 * unknown keys are ignored, and `schemaVersion` guards future shape changes.
 */
export interface StoredCliConfig {
  readonly schemaVersion?: number;
  readonly apiKey?: string;
  readonly aexUrl?: string;
}

/**
 * Read/write the persisted CLI config. Implemented by `cli.ts` over
 * `node:fs/promises`; `read()` resolves `null` (never throws) when the file is
 * absent or unreadable so an unauthenticated CLI degrades to "no stored token".
 */
export interface CliConfigStore {
  /** The resolved on-disk config path (for display; never prints the token). */
  location(): string;
  read(): Promise<StoredCliConfig | null>;
  write(config: StoredCliConfig): Promise<void>;
  clear(): Promise<void>;
}

export interface OutputsSyncFileEntry {
  /** Absolute path inside the container. */
  readonly path: string;
  /** File size in bytes. */
  readonly sizeBytes: number;
}
