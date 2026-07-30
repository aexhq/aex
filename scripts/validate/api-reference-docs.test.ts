import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "bun:test";
import {
  API_REFERENCE_PAGE_PATH,
  API_REFERENCE_SPEC_PATH,
  renderApiReferenceMarkdown,
  type OpenApiDocument
} from "../docs/api-reference.js";

const repoRoot = resolve(import.meta.dirname, "..", "..");
const specPath = resolve(repoRoot, API_REFERENCE_SPEC_PATH);
const document = JSON.parse(readFileSync(specPath, "utf8")) as OpenApiDocument;

function operationIds(spec: OpenApiDocument): readonly string[] {
  return Object.values(spec.paths).flatMap((item) =>
    Object.values(item).map((operation) => operation.operationId ?? "")
  );
}

/** A structured clone the fixtures below can mutate without touching the committed spec. */
function specCopy(): OpenApiDocument {
  return JSON.parse(readFileSync(specPath, "utf8")) as OpenApiDocument;
}

describe("generated HTTP API reference page", () => {
  it("documents every operation and path the spec declares", () => {
    const page = renderApiReferenceMarkdown(document);
    const missingOperations = operationIds(document).filter((id) => !page.includes(`\`${id}\``));
    const missingPaths = Object.keys(document.paths).filter((path) => !page.includes(`\`${path}\``));

    expect(missingOperations).toEqual([]);
    expect(missingPaths).toEqual([]);
    expect(operationIds(document).length).toBeGreaterThan(0);
  });

  it("publishes the strict-v1 registered-content wire contract", () => {
    const files = document.paths["/api/workspace/files/{name}"];
    const download = document.paths["/api/workspace/files/{name}/downloads"];
    const list = document.paths["/api/workspace/files"]?.get;
    const schemas = document.components?.schemas;

    expect(files?.put?.requestBody?.content?.["application/json"]?.schema?.$ref)
      .toBe("#/components/schemas/RegisteredFileInput");
    expect(files?.put?.responses?.["2XX"]?.content?.["application/json"]?.schema?.$ref)
      .toBe("#/components/schemas/RegistryPutResult");
    expect(download?.post?.operationId).toBe("registry.files.download");
    expect(download?.post?.responses?.["2XX"]?.content?.["application/json"]?.schema?.$ref)
      .toBe("#/components/schemas/DownloadGrant");
    expect(list?.description).toContain("Current-view name-keyset pagination");
    expect(list?.description).toContain("not a snapshot");
    expect(schemas?.BlobDescriptor?.required).toEqual(["sha256", "sizeBytes"]);
    expect(schemas?.RegisteredFileValue?.properties?.content?.$ref)
      .toBe("#/components/schemas/BlobDescriptor");
    expect(schemas?.RegistryPutResult?.properties?.status?.enum)
      .toEqual(["created", "replaced", "unchanged"]);
    expect(schemas?.UploadPartsRequest?.properties?.parts?.items?.required)
      .toEqual(["partNumber", "sizeBytes", "sha256"]);
    expect(schemas?.UploadCompleteRequest?.properties?.parts?.items?.required)
      .toEqual(["partNumber", "etag", "sizeBytes", "sha256"]);
    expect(Object.keys(document.paths).some(
      (path) => /\/(?:versions|copy|history)(?:\/|$)/.test(path)
    )).toBe(false);
  });

  it("renders the same bytes for the same document", () => {
    expect(renderApiReferenceMarkdown(document)).toBe(renderApiReferenceMarkdown(document));
  });

  it("states the request-body gap rather than implying the routes take none", () => {
    const withoutBodies = specCopy();
    for (const item of Object.values(withoutBodies.paths)) {
      for (const operation of Object.values(item)) {
        delete (operation as { requestBody?: unknown }).requestBody;
      }
    }
    const page = renderApiReferenceMarkdown(withoutBodies);

    expect(page).toContain("None of these operations declares a request body");
    expect(page).not.toContain("| Method | Path | Operation | Scope | Request body |");
  });

  it("gains a linked request-body column as operations declare one", () => {
    const withBody = specCopy();
    const sessions = withBody.paths["/api/sessions"];
    expect(sessions?.post?.operationId).toBe("sessions.create");
    Object.assign(sessions?.post ?? {}, {
      requestBody: {
        required: true,
        content: {
          "application/json": { schema: { $ref: "#/components/schemas/SubmissionEnvironment" } }
        }
      }
    });
    const page = renderApiReferenceMarkdown(withBody);

    expect(page).toContain("| Method | Path | Operation | Scope | Request body |");
    expect(page).toContain("[`SubmissionEnvironment`](#submissionenvironment) |");
    expect(page).not.toContain("None of these operations declares a request body");
  });

  it("is wired into the docs generator and the reference navigation", () => {
    const generator = readFileSync(resolve(repoRoot, "scripts/docs/generate-all.mjs"), "utf8");

    expect(generator).toContain("renderApiReferenceMarkdown");
    expect(generator).toContain("await generateApiReference();");
    expect(generator).toMatch(/const referencePages = \[[^\]]*"api"/);
    expect(API_REFERENCE_PAGE_PATH).toBe("apps/docs/content/docs/reference/api.md");
  });
});
