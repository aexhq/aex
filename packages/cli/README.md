# Aex CLI

Node.js 22 or newer. The CLI uses `@aexhq/sdk` and the same public Aex HTTP API as
the dashboard. No dashboard cookies, site credentials or operator credentials are needed.

```sh
npm install -g @aexhq/cli
aex login
aex keys create "My application"
aex keys list
aex keys rename KEY_ID "New name"
aex keys revoke KEY_ID
aex account
aex billing
aex usage
aex docs
aex logout
```

`login` opens your browser for Google sign-in or registration. Confirm the account on
the Aex page, then return to your terminal. The browser must run on the same computer
as the CLI. `--no-browser` prints the URL instead of opening it. A loopback listener,
state and S256 PKCE bind a one-use, 60-second code to the initiating CLI. Account
credentials expire after seven days; run `login` again. `logout` revokes this session.

Results are JSON on stdout; errors and login progress go to stderr. A created key's
token is returned once. Existing keys expose only their metadata and prefix.
Billing currently reports free preview hosting with customer-paid model usage.

The credential file is `~/.aex/account.json` on Unix or `%LOCALAPPDATA%/.aex/account.json`
on Windows, restricted to the current user (and Windows SYSTEM). `AEX_CONFIG_DIR`
overrides its directory. For automation, supply `AEX_ACCOUNT_TOKEN`; a workload API
key cannot manage account credentials. `AEX_API_URL` or `--api-url` changes the API
origin; stored credentials cannot be reused against a different origin. `--site-url`
selects the matching login website for a self-hosted installation.
