# @aexhq/cli

```sh
npm install --global @aexhq/cli
aex login
aex session list
aex session send ses_... "Summarise the repository."
aex session output ses_... --schema result.schema.json "Return the key findings."
```

The CLI stores the pasted API key in `~/.aex/config.json`, requests mode `0600` on POSIX systems,
and uses `https://api.aex.dev` by default.
