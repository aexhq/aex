import type { FetchLike } from "@aexhq/contracts";

export interface StoredCliConfig {
  readonly apiKey?: string;
  readonly aexUrl?: string;
}

export interface CliConfigStore {
  read(): Promise<StoredCliConfig | null>;
}

export interface CliIO {
  readonly argv: readonly string[];
  readonly cwd: () => string;
  readonly readFile: (path: string) => Promise<string>;
  readonly readFileBytes?: (path: string) => Promise<Uint8Array>;
  readonly readStdin?: () => Promise<string>;
  readonly stdinIsTTY: boolean | undefined;
  readonly writeFile: (path: string, data: Uint8Array) => Promise<void>;
  readonly appendFile?: (path: string, data: Uint8Array) => Promise<void>;
  readonly fileSize?: (path: string) => Promise<number>;
  readonly sha256File?: (path: string) => Promise<string>;
  readonly renameFile?: (from: string, to: string) => Promise<void>;
  readonly fileExists?: (path: string) => Promise<boolean>;
  readonly fetchImpl: FetchLike;
  readonly stdout: (chunk: string) => void;
  readonly stdoutBytes?: (chunk: Uint8Array) => void;
  readonly stderr: (chunk: string) => void;
  readonly exit: (code: number) => void;
  readonly configStore?: CliConfigStore;
}
