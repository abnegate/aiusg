# aiusg

One place to see how much of your AI subscription you have left.

Authenticate any number of accounts — including several on the same provider — and
`aiusg` fetches every account's limits in parallel and prints them as one table.

```
  Codex     jake@example.com  pro
    7d                     ████████████████████ 100%              resets in 5d 21h
    GPT-5.3-Codex-Spark 5h ░░░░░░░░░░░░░░░░░░░░   0%              resets in 4h 59m

  Copilot   octocat  individual
    Premium requests       ████████████████████ 100%  1506/1500   resets in 21d 19h

  Grok      jake@example.com  SuperGrok Heavy
    GrokBuild              ████████████████████ 100%              resets in 1d 21h
```

## Install

```bash
cargo install --git https://github.com/abnegate/aiusg
```

## Use

```bash
aiusg import              # adopt accounts already signed in to their CLI
aiusg login claude        # or sign in directly — repeat for each account
aiusg                     # show every account
aiusg --provider claude   # just one provider
aiusg --json              # machine readable, for status lines and scripts
aiusg watch               # live dashboard, r to refresh, q to quit
aiusg list                # stored accounts
aiusg remove claude:jake@example.com
```

Multiple accounts on one provider are the point: run `aiusg login claude` once per
account and each is stored separately, keyed by `provider:label`.

## Providers

| Provider | Source | What you get |
|---|---|---|
| Claude | `GET api.anthropic.com/api/oauth/usage` | 5h and 7d windows, per-model 7d windows, extra usage credits |
| Codex | `GET chatgpt.com/backend-api/wham/usage` | Primary and secondary windows plus per-model buckets, with reset times |
| Copilot | `GET api.github.com/copilot_internal/user` | Premium request quota, used/entitlement, monthly reset |
| Gemini | `POST cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota` | Per-model remaining requests and reset time (needs an OAuth client, below) |
| Grok | `GET cli-chat-proxy.grok.com/v1/billing?format=credits` | Credit usage per product, billing period reset |

Reading usage never spends quota — every endpoint above is a plain read.

### Gemini needs its own OAuth client

Google's Code Assist API has no public client, so `aiusg` does not ship one.
`aiusg import gemini` works without setup and reads usage until the imported
token expires, but `aiusg login gemini` and token refresh need a client of your
own:

```bash
export AIUSG_GEMINI_CLIENT_ID=...
export AIUSG_GEMINI_CLIENT_SECRET=...
```

Create a **Desktop app** OAuth client in a Google Cloud project with the Gemini
for Cloud API enabled, or reuse the public installed-app client that the
`gemini-cli` project publishes in its own source.

## Where credentials live

By default, tokens are stored in `~/.config/aiusg/credentials.json` with `0600`
permissions inside a `0700` directory — the same approach the Codex, Gemini and
Grok CLIs already take with their own tokens.

Set `AIUSG_KEYCHAIN=1` to use the OS keychain (macOS Keychain, Windows Credential
Manager, Linux Secret Service) instead. Be aware of the trade-off on macOS: a
keychain ACL is bound to the requesting binary's code signature, so an unsigned
binary — anything built locally or installed with `cargo install` — is treated as
a new application after every rebuild, and "Always Allow" will not stick. You get
a password prompt per account, per build. The file backend is the default for
exactly this reason.

`aiusg` refreshes expired tokens itself and writes the new token back to its own
store. It never writes to another CLI's credential files.

## Caveats worth knowing

**These endpoints are undocumented.** Every provider here exposes subscription
usage through internal endpoints their own CLI uses. They are not covered by any
API stability promise and can change without warning. Responses are parsed
permissively so a new field will not break the table, but a renamed one will show
as `no limits reported` or a failed row. Set `AIUSG_DEBUG=1` to dump raw
responses when that happens.

**Importing Grok can log out the Grok CLI.** xAI rotates refresh tokens: when
`aiusg` refreshes an imported Grok credential, the copy in `~/.grok/auth.json`
becomes stale and the `grok` CLI may need `grok login` again. Use
`aiusg login grok` instead of `aiusg import` to keep the two independent.

**One CLI slot is not one account.** Each provider's own CLI stores a single
signed-in account, so `aiusg import` can only ever adopt one account per
provider. Use `aiusg login` for the rest.

## Configuration

| Variable | Effect |
|---|---|
| `AIUSG_HOME` | Where accounts and credentials are stored (default `~/.config/aiusg`) |
| `AIUSG_KEYCHAIN` | Set to `1` to store tokens in the OS keychain instead of a `0600` file |
| `AIUSG_DEBUG` | Dump raw provider responses to stderr |
| `CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `GEMINI_CLI_HOME`, `GROK_HOME` | Honoured when importing |

## Licence

MIT
