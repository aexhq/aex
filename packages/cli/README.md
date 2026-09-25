# Aex CLI

Manage your Aex API keys, account and usage from the terminal. To run an agent, use the
[SDK quickstart](https://aex.dev/docs).

## Get started

Requires Node.js 22 or newer:

```sh
npm install -g @aexhq/cli@0.50.0
aex login
aex keys create "My application"
```

`login` opens your browser. Sign in, confirm the account, and return to the terminal on the same
computer. Save the new key's token as `AEX_API_KEY` in your application's server environment.
The secret is shown only once.

## Common commands

| Task | Command |
| --- | --- |
| List keys | `aex keys list` |
| Rename a key | `aex keys rename KEY_ID "New name"` |
| Revoke a key | `aex keys revoke KEY_ID` |
| Read account details | `aex account` |
| Check credits, prices and limits | `aex billing` |
| Read account usage | `aex usage` |
| Read one session's usage | `aex usage SESSION_ID` |
| Open the docs | `aex docs` |
| Sign out | `aex logout` |

Results are JSON on stdout; errors and login progress go to stderr. Account logins expire after
seven days; run `aex login` again. Use `--no-browser` to print the login URL instead.

## Billing

`aex billing ledger`, `aex billing topups` and `aex billing refunds` show account history.
`aex billing set PRICEBOOK MICRO_USD` accepts the offered prices and sets a monthly limit.
One dollar is 1,000,000 micro-USD.

`aex billing topup CENTS KEY` returns a Checkout URL; `aex billing refund TOPUP CENTS KEY`
requests a refund of unused credits. Reuse the same operation key when recovering the same
payment request. See [billing](https://github.com/aexhq/aex/blob/main/docs/billing.md).

## Automation and configuration

Use `AEX_ACCOUNT_TOKEN` for account automation; a workload API key cannot manage other keys.
The credential file is `~/.aex/account.json` on Unix or `%LOCALAPPDATA%/.aex/account.json`
on Windows. `AEX_CONFIG_DIR` overrides the directory.

For self-hosting, `AEX_API_URL` or `--api-url` selects the API and `--site-url` selects its login
website. Stored credentials are bound to their API origin.
