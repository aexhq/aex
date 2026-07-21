import { SessionStateError } from "./sdk-errors.js";
import type {
  SessionFile,
  SessionFilePathSelector,
  SessionFileQuery,
  SessionFileSelector,
  SessionFileType
} from "./runtime-types.js";

/** Package-private path-selector guard shared by transport orchestration. */
export function isPathSelector(selector: SessionFileSelector): selector is SessionFilePathSelector {
  return Boolean(selector && typeof selector === "object" && "path" in selector);
}

function normalizeSessionFileLookupPath(path: string): string {
  return path.replace(/\\/g, "/").replace(/^\/+/, "");
}

export function resolveSessionFileSelector(
  files: readonly SessionFile[],
  selector: SessionFilePathSelector,
  sessionId?: string
): SessionFile {
  const target = normalizeSessionFileLookupPath(selector.path);
  if (!target) {
    throw new SessionStateError("files.download: file path must be non-empty", { sessionId, path: selector.path });
  }
  const matches = files.filter((file) => {
    if (typeof file.filename !== "string") return false;
    const filename = normalizeSessionFileLookupPath(file.filename);
    if (selector.match === "suffix") {
      return filename === target || filename.endsWith(`/${target}`);
    }
    return filename === target;
  });
  if (matches.length === 1) return matches[0]!;
  if (matches.length > 1) {
    throw new SessionStateError(
      `files.download: file path "${selector.path}" matched multiple files`,
      { sessionId, path: selector.path, matches: matches.map((file) => file.filename ?? file.id) }
    );
  }
  throw new SessionStateError(`files.download: file path "${selector.path}" was not found`, {
    sessionId,
    path: selector.path
  });
}

export function filterSessionFiles(files: readonly SessionFile[], query: SessionFileQuery): readonly SessionFile[] {
  return files.filter((file) => sessionFileMatchesQuery(file, query));
}

/**
 * The single filename-matcher for cross-session / per-session file SEARCH. A
 * string is a case-insensitive SUBSTRING match; a RegExp is tested as given (and
 * reset to `lastIndex = 0` so a reused `/g` regex is safe). Sharing this SSoT is
 * what closes the T16 crash class: session-file search no longer assumes `filename`
 * is a string and passes a RegExp into `escapeRegExp(...).replace(...)`.
 */
export function toFilenameMatcher(filename: string | RegExp): (name: string) => boolean {
  if (typeof filename === "string") {
    const needle = filename.toLowerCase();
    return (name: string) => name.toLowerCase().includes(needle);
  }
  return (name: string) => {
    filename.lastIndex = 0;
    return filename.test(name);
  };
}

export function classifySessionFile(file: Pick<SessionFile, "filename" | "contentType">): SessionFileType {
  const contentType = normalizeContentType(file.contentType);
  if (contentType) {
    if (contentType === "application/json" || contentType.endsWith("+json") || contentType.includes("json")) {
      return "json";
    }
    if (contentType.startsWith("text/")) return "text";
    if (contentType.startsWith("image/")) return "image";
    if (contentType.startsWith("audio/")) return "audio";
    if (contentType.startsWith("video/")) return "video";
    if (contentType === "application/pdf") return "pdf";
    if (
      contentType === "application/zip" ||
      contentType === "application/gzip" ||
      contentType === "application/x-gzip" ||
      contentType === "application/x-tar" ||
      contentType === "application/x-7z-compressed" ||
      contentType === "application/vnd.rar" ||
      contentType === "application/zstd"
    ) {
      return "archive";
    }
    if (contentType === "application/octet-stream") return "binary";
    return "unknown";
  }

  const extension = extensionOf(file.filename);
  if (!extension) return "unknown";
  if (["json", "jsonl", "ndjson"].includes(extension)) return "json";
  if (["txt", "log", "md", "markdown", "csv", "tsv", "xml", "html", "htm", "yaml", "yml"].includes(extension)) {
    return "text";
  }
  if (["png", "jpg", "jpeg", "gif", "webp", "avif", "bmp", "tif", "tiff", "svg"].includes(extension)) {
    return "image";
  }
  if (["mp3", "wav", "flac", "m4a", "aac", "ogg", "oga", "opus"].includes(extension)) return "audio";
  if (["mp4", "mov", "m4v", "webm", "mkv", "avi"].includes(extension)) return "video";
  if (extension === "pdf") return "pdf";
  if (["zip", "tar", "tgz", "gz", "bz2", "xz", "7z", "rar", "zst"].includes(extension)) return "archive";
  if (["bin", "exe", "dll", "so", "dylib", "dmg", "iso"].includes(extension)) return "binary";
  return "unknown";
}

function sessionFileMatchesQuery(file: SessionFile, query: SessionFileQuery): boolean {
  const normalizedPath = typeof file.filename === "string" ? normalizeSessionFileQueryPath(file.filename) : "";
  if (query.path !== undefined && normalizedPath !== normalizeSessionFileQueryPath(query.path)) {
    return false;
  }
  if (query.filename !== undefined) {
    const basename = basenameOf(normalizedPath);
    if (typeof query.filename === "string") {
      if (basename !== query.filename) return false;
    } else {
      query.filename.lastIndex = 0;
      if (!query.filename.test(basename)) return false;
    }
  }
  if (query.dir !== undefined && !directoryMatches(normalizedPath, query.dir, query.recursive ?? true)) {
    return false;
  }
  if (query.extension !== undefined && extensionOf(normalizedPath) !== normalizeExtension(query.extension)) {
    return false;
  }
  if (query.contentType !== undefined && !contentTypeMatches(file.contentType, query.contentType)) {
    return false;
  }
  if (query.type !== undefined && classifySessionFile(file) !== query.type) {
    return false;
  }
  return true;
}

function normalizeSessionFileQueryPath(path: string): string {
  let normalized = path.replace(/\\/g, "/").replace(/^\/+/, "");
  while (normalized === "files" || normalized.startsWith("files/")) {
    normalized = normalized === "files" ? "" : normalized.slice("files/".length);
  }
  return normalized.replace(/\/+$/, "");
}

function basenameOf(path: string): string {
  return path.split("/").filter(Boolean).pop() ?? "";
}

function directoryMatches(path: string, dir: string, recursive: boolean): boolean {
  const normalizedDir = normalizeSessionFileQueryPath(dir);
  if (normalizedDir.length === 0) return true;
  const prefix = `${normalizedDir}/`;
  if (!path.startsWith(prefix)) return false;
  const remainder = path.slice(prefix.length);
  return remainder.length > 0 && (recursive || !remainder.includes("/"));
}

function normalizeExtension(extension: string): string {
  return extension.replace(/^\.+/, "").toLowerCase();
}

function extensionOf(path: string | undefined): string {
  if (!path) return "";
  const basename = basenameOf(normalizeSessionFileQueryPath(path));
  const index = basename.lastIndexOf(".");
  return index > 0 && index < basename.length - 1 ? basename.slice(index + 1).toLowerCase() : "";
}

function normalizeContentType(contentType: string | undefined): string {
  return (contentType ?? "").split(";")[0]!.trim().toLowerCase();
}

function contentTypeMatches(actual: string | undefined, expected: string): boolean {
  const normalizedActual = normalizeContentType(actual);
  const normalizedExpected = normalizeContentType(expected);
  if (!normalizedActual || !normalizedExpected) return false;
  if (normalizedExpected.endsWith("/*")) {
    return normalizedActual.startsWith(normalizedExpected.slice(0, -1));
  }
  return normalizedActual === normalizedExpected;
}
