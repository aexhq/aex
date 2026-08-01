# `account-backbone`

The first root applied in a new account, and the only one that can be applied
before a state backend exists.

It creates the Terraform state bucket and key, the infrastructure artifact
bucket, the operational notification topic, the OIDC publish role and the
account budgets. Everything else in this directory assumes it has already run.

Apply it once with local state, then move that state into the bucket it created:

```hcl
terraform {
  backend "s3" {
    bucket       = "aex-tfstate-dev-<suffix>"
    key          = "account-backbone/terraform.tfstate"
    region       = "eu-west-1"
    encrypt      = true
    kms_key_id   = "alias/aex-tfstate-dev"
    use_lockfile = true
  }
}
```

## Sanitized values

Every value here is a variable with no default. There is no account id, no ARN
and no domain in any `.tf` file in this directory. Where the documentation needs
an example, `000000000000` stands for an account id and `example.invalid` for a
hostname; neither ever appears in configuration.

## Modules used

| Module | Purpose |
| --- | --- |
| `tfstate-backend` | State bucket and key. |
| `artifact-bucket` | Versioned bucket for Lambda ZIPs and module bundles. |
| `sns-ops-topic` | Operational notifications. |
| `github-oidc-role` | The publish role, which carries no deploy action. |
| `budget-and-cost` | Budgets and cost anomaly detection. |

## Outputs

`state_bucket`, `state_kms_key_arn`, `artifact_bucket`, `ops_topic_arn`,
`publish_role_arn`, `budget_arns`.

## Test

`tests/account-backbone.tftest.hcl` plans the whole root against a mock AWS
provider and asserts the derived bucket names, the publish/deploy disjointness
and the completeness of the mandatory cost tag set.
