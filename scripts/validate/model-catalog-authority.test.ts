import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const root = resolve(import.meta.dir, "../..");
const read = (path: string): string => readFileSync(resolve(root, path), "utf8");

describe("protected model-catalog authority", () => {
  test("the manual workflow consumes reviewed metadata and never installs bindings", () => {
    const source = read(".github/workflows/model-catalog-publish.yml");
    const workflow = Bun.YAML.parse(source) as { readonly on: any; readonly jobs: Record<string, any> };
    const inputs = workflow.on.workflow_dispatch.inputs;
    const job = workflow.jobs.publish;

    expect(Object.keys(inputs).sort()).toEqual([
      "catalog_document_path",
      "catalog_document_sha256",
      "source_sha"
    ]);
    expect(workflow.on).not.toHaveProperty("push");
    expect(workflow.on).not.toHaveProperty("schedule");
    expect(job.environment).toBe("aex-model-catalog-publisher");
    expect(job.permissions).toEqual({ contents: "write", "id-token": "write" });
    expect(source).toContain('[[ "$REVIEWED_SOURCE_SHA" == "$GITHUB_SHA" ]]');
    expect(source).toContain("git ls-files --error-unmatch");
    expect(source).toContain("release/model-catalog/");
    expect(source).toContain('[[ -n "$AEX_MODEL_CATALOG_PUBLISH_CONFIRMATION" ]]');
    expect(source).not.toContain("gh variable set");
    expect(source).not.toContain("gh secret set");
    expect(source).not.toContain("--clobber");
  });

  test("the workflow validates KMS shape, signs DIGEST, and read-backs immutable assets", () => {
    const source = read(".github/workflows/model-catalog-publish.yml");
    expect(source.match(/aws-actions\/configure-aws-credentials/g)).toHaveLength(1);
    expect(source).toContain("allowed-account-ids:");
    expect(source).toContain("aws kms describe-key --key-id");
    expect(source).toContain("aws kms get-public-key --key-id");
    expect(source).toContain('.KeySpec == "ECC_NIST_P256"');
    expect(source).toContain('.KeyUsage == "SIGN_VERIFY"');
    expect(source).toContain('.SigningAlgorithms == ["ECDSA_SHA_256"]');
    expect(source).toContain("aws kms sign --key-id");
    expect(source).toContain("--signing-algorithm ECDSA_SHA_256 --message-type DIGEST");
    expect(source).toContain('--message "fileb://$PWD/$PUBLISH_DIR/message.digest"');
    expect(source).toContain("assemble-genesis");
    expect(source).toContain("gh release create");
    expect(source).toContain("--draft --prerelease");
    expect(source).toContain("gh release download");
    expect(source).toContain("cmp --silent");
    expect(source).toContain("gh release edit");
    expect(source).toContain("model-catalog-collection-${collection_digest#sha256:}.json");
    expect(source).toContain("aex.model-catalog-repository-bindings.v1");
    for (const variable of [
      "AEX_MODEL_CATALOG_TRUST_ROOTS_JSON",
      "AEX_MODEL_CATALOG_TRUST_ROOTS_SHA256",
      "AEX_MODEL_CATALOG_COLLECTION_URI",
      "AEX_MODEL_CATALOG_COLLECTION_SHA256"
    ]) {
      expect(source).toContain(variable);
    }
  });

  test("the reusable Terraform module creates only one dedicated signer key and role", () => {
    const main = read("infra/modules/model-catalog-authority/main.tf");
    const variables = read("infra/modules/model-catalog-authority/variables.tf");
    const outputs = read("infra/modules/model-catalog-authority/outputs.tf");
    expect(main).toContain('customer_master_key_spec           = "ECC_NIST_P256"');
    expect(main).toContain('key_usage                          = "SIGN_VERIFY"');
    expect(main).toContain('variable = "kms:SigningAlgorithm"');
    expect(main).toContain('values   = ["ECDSA_SHA_256"]');
    expect(main).toContain('actions   = ["kms:DescribeKey", "kms:GetPublicKey"]');
    expect(main).toContain('actions   = ["kms:Sign"]');
    expect(main).toContain('variable = "token.actions.githubusercontent.com:sub"');
    expect(main).toContain('values   = ["repo:${var.repository}:environment:${var.environment}"]');
    expect(main).toContain('variable = "token.actions.githubusercontent.com:repository"');
    expect(main).toContain('variable = "token.actions.githubusercontent.com:ref"');
    expect(main).toContain('values   = ["refs/heads/main"]');
    expect(main).toContain('variable = "token.actions.githubusercontent.com:workflow"');
    expect(main).toContain("values   = [var.workflow_name]");
    expect(main).not.toContain("job_workflow_ref");
    expect(main).not.toContain("workflow_ref");
    expect(main).toContain("prevent_destroy = true");
    expect(main).not.toMatch(/kms:(?:Encrypt|Decrypt|GenerateDataKey|Verify)"/);
    expect(variables).toContain('default     = "aexhq/aex"');
    expect(variables).toContain('default     = "aex-model-catalog-publisher"');
    expect(outputs).toContain('output "kms_key_arn"');
    expect(outputs).toContain('output "publisher_role_arn"');
  });
});
