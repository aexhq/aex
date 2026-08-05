import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const root = resolve(import.meta.dir, "../..");
const read = (path: string): string => readFileSync(resolve(root, path), "utf8");

const terraformStatementActions = (source: string, sid: string): readonly string[] => {
  const sidMatch = new RegExp(`sid\\s*=\\s*"${sid}"`).exec(source);
  if (sidMatch?.index === undefined) {
    throw new Error(`Terraform statement ${sid} is missing`);
  }
  const nextStatement = source.indexOf("\n  statement {", sidMatch.index);
  const statement = source.slice(
    sidMatch.index,
    nextStatement === -1 ? source.length : nextStatement
  );
  const actions = /actions\s*=\s*\[([\s\S]*?)\]/.exec(statement)?.[1];
  if (actions === undefined) {
    throw new Error(`Terraform statement ${sid} has no action list`);
  }
  return Array.from(actions.matchAll(/"(kms:[^"]+)"/g), (match) => match[1]!);
};

describe("protected model-catalog authority", () => {
  test("the protected publisher consumes only reviewed static compatibility metadata", () => {
    const source = read(".github/workflows/model-catalog-publish.yml");
    const workflow = Bun.YAML.parse(source) as { readonly on: any; readonly jobs: Record<string, any> };
    const inputs = workflow.on.workflow_dispatch.inputs;
    const job = workflow.jobs.publish;

    expect(Object.keys(inputs).sort()).toEqual([
      "catalog_source_path",
      "catalog_source_sha256",
      "source_sha"
    ]);
    expect(workflow.on.push).toEqual({
      branches: ["main"],
      paths: ["release/model-catalog/*.source.json"]
    });
    expect(workflow.on).not.toHaveProperty("schedule");
    expect(job.environment).toBe("aex-model-catalog-publisher");
    expect(job.permissions).toEqual({ contents: "write", "id-token": "write" });
    expect(source).toContain("git ls-files --error-unmatch");
    expect(source).toContain("release/model-catalog/*.source.json");
    expect(source).toContain("git diff --name-only --no-renames --diff-filter=ACMRT");
    expect(source).toMatch(/generate \\\r?\n\s+--source "\$CATALOG_SOURCE_PATH"/);
    expect(source).toMatch(/validate \\\r?\n\s+--source "\$CATALOG_SOURCE_PATH"/);
    expect(source).toContain('--document "$PUBLISH_DIR/catalog-document.json"');
    expect(source).toContain('--previous-collection "$PREVIOUS_COLLECTION_PATH"');
    expect(source).toContain("AEX_MODEL_CATALOG_BINDING_JSON");
    expect(source).toContain('[[ -n "$AEX_MODEL_CATALOG_PUBLISH_CONFIRMATION" ]]');
    expect(source.indexOf("Resolve and verify reviewed catalog source before any AWS call")).toBeLessThan(
      source.indexOf("aws-actions/configure-aws-credentials")
    );
    expect(source).not.toMatch(/qualification|DEEPSEEK|provider/i);
    expect(source).not.toContain("gh attestation verify");
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
    expect(source).toContain("od -An -v -tx1");
    expect(source).toContain('[[ "sha256:$decoded_digest" == "$request_digest" ]]');
    expect(source).not.toContain('sha256sum "$PUBLISH_DIR/message.digest"');
    expect(source).toContain('--message "fileb://$PWD/$PUBLISH_DIR/message.digest"');
    expect(source).toContain('aex-model-catalog-publisher" assemble');
    expect(source).toContain("gh release create");
    expect(source).toContain("--draft --prerelease");
    expect(source).toContain("gh release download");
    expect(source).toContain("cmp --silent");
    expect(source).toContain("gh release edit");
    expect(source).toContain("model-catalog-collection-${collection_digest#sha256:}.json");
    expect(source).toContain("aex.model-catalog-build-binding.v1");
    expect(source).toContain("model-catalog-build-binding-${build_binding_digest#sha256:}.json");
    expect(source).toContain("atomically replacing AEX_MODEL_CATALOG_BINDING_JSON");
    expect(source).not.toContain("AEX_MODEL_CATALOG_TRUST_ROOTS_JSON");
    expect(source).not.toContain("AEX_MODEL_CATALOG_COLLECTION_URI");
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
    expect(terraformStatementActions(main, "AdministerByExactOwnerPrincipals")).toEqual([
      "kms:CancelKeyDeletion",
      "kms:CreateAlias",
      "kms:DeleteAlias",
      "kms:DescribeKey",
      "kms:DisableKey",
      "kms:EnableKey",
      "kms:GetKeyPolicy",
      "kms:GetKeyRotationStatus",
      "kms:ListGrants",
      "kms:ListKeyPolicies",
      "kms:ListResourceTags",
      "kms:PutKeyPolicy",
      "kms:RevokeGrant",
      "kms:ScheduleKeyDeletion",
      "kms:TagResource",
      "kms:UntagResource",
      "kms:UpdateAlias",
      "kms:UpdateKeyDescription"
    ]);
    expect(terraformStatementActions(main, "InspectByExactPublisher")).toEqual([
      "kms:DescribeKey",
      "kms:GetPublicKey"
    ]);
    expect(terraformStatementActions(main, "SignEcdsaSha256ByExactPublisher")).toEqual([
      "kms:Sign"
    ]);
    expect(terraformStatementActions(main, "InspectExactCatalogKey")).toEqual([
      "kms:DescribeKey",
      "kms:GetPublicKey"
    ]);
    expect(terraformStatementActions(main, "SignExactCatalogKeyWithEcdsaSha256")).toEqual([
      "kms:Sign"
    ]);
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

  test("the Terraform signer declares delivery-graph ownership", () => {
    const metadata = Bun.TOML.parse(
      read("infra/modules/model-catalog-authority/aex.toml")
    ) as {
      readonly aex: {
        readonly owner: string;
        readonly role: string;
        readonly artifact: string;
        readonly layers: readonly string[];
        readonly concerns: readonly string[];
        readonly seams: readonly string[];
        readonly security_tier: string;
        readonly risk: readonly string[];
        readonly scenarios: readonly string[];
        readonly not_applicable: { readonly live_suite: string };
      };
    };

    expect(metadata.aex).toEqual({
      owner: "delivery",
      role: "infra_module",
      artifact: "terraform_module",
      layers: ["unit", "integration"],
      concerns: ["security"],
      seams: [],
      security_tier: "internal",
      risk: ["iam"],
      scenarios: [],
      not_applicable: {
        live_suite:
          "The dedicated signing key and OIDC trust are exercised only by the protected publication workflow; this module has no deployed product endpoint to probe."
      }
    });
  });
});
