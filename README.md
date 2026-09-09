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

**Homebrew** (macOS / Linux):

```bash
brew tap abnegate/tap
brew install aiusg
```

**APT** (Debian / Ubuntu):

```bash
curl -fsSL https://abnegate.github.io/apt-repo/pubkey.gpg | sudo gpg --dearmor -o /usr/share/keyrings/abnegate.gpg
echo "deb [signed-by=/usr/share/keyrings/abnegate.gpg] https://abnegate.github.io/apt-repo stable main" | sudo tee /etc/apt/sources.list.d/abnegate.list
sudo apt update && sudo apt install aiusg
```

**Binary** from [Releases](https://github.com/abnegate/aiusg/releases), or from source:

```bash
cargo install --git https://github.com/abnegate/aiusg
```

## Use

```bash
aiusg import              # adopt accounts already signed in to their CLI
aiusg login claude        # or sign in directly — repeat for each account
aiusg                     # show every account
aiusg --provider claude   # just one provider
aiusg --all               # include accounts that are signed out
aiusg --json              # machine readable, for status lines and scripts
aiusg watch               # live dashboard, r to refresh, q to quit
aiusg list                # stored accounts
aiusg remove claude:jake@example.com
aiusg mcp                 # MCP server over stdio, for agents
```

Multiple accounts on one provider are the point: run `aiusg login claude` once per
account and each is stored separately, keyed by `provider:label`.

## MCP server

`aiusg mcp` speaks MCP over stdio, so an agent can read every account's remaining
usage and pick which one to send work to.

| Tool | What it does |
|---|---|
| `route` | Ranks accounts by headroom and returns the one with the most left, plus alternatives and why the rest are out (exhausted, signed out, failing) with reset times |
| `usage` | Every account's windows — used percent, counts, reset times |
| `accounts` | Stored accounts, without fetching usage |

`route` and `usage` take an optional `provider` to scope to one of `claude`,
`codex`, `gemini`, `copilot`, `grok`, `cursor`. Headroom is `100 -` the used
percent of the account's most-consumed window, so the window closest to its cap
decides the ranking. When nothing has headroom left, `route` returns a tool error
carrying the reset times, so the caller can wait rather than retry blindly.

Register it with Claude Code:

```bash
claude mcp add aiusg -- aiusg mcp
```

Or in any client that reads `mcpServers`:

```json
{
  "mcpServers": {
    "aiusg": { "command": "aiusg", "args": ["mcp"] }
  }
}
```

## Providers

| Provider | Source | What you get |
|---|---|---|
| Claude | `GET api.anthropic.com/api/oauth/usage` | Session and weekly windows from `limits[]`, including per-model ones, plus extra usage credits |
| Codex | `GET chatgpt.com/backend-api/wham/usage` | Primary and secondary windows plus per-model buckets, with reset times |
| Copilot | `GET api.github.com/copilot_internal/user` | Premium request quota, used/entitlement, monthly reset |
| Gemini | `POST cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota` | Per-model remaining requests and reset time (needs an OAuth client, below) |
| Grok | `GET cli-chat-proxy.grok.com/v1/billing?format=credits` | Credit usage per product, billing period reset |
| Cursor | `GET cursor.com/api/usage-summary` | Included and on-demand usage, billing cycle reset |

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

### Signed-out accounts are hidden

An account whose token has expired and cannot be refreshed is dropped from the
table, with a one-line count at the bottom rather than a failure row for
something you already know about. `aiusg --all` shows them; `--json` always
includes them, each tagged with a `status` of `ok`, `signed_out` or `failed`.

### Cursor is import-only

Cursor has no CLI sign-in flow to drive, so `aiusg login cursor` reads the
session the Cursor app already holds in its `state.vscdb` — sign in to Cursor
first. Its token lasts about three months and renews the same way. Point
`AIUSG_CURSOR_DB` at the database to override where it is read from.

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
| `AIUSG_CURSOR_DB` | Path to Cursor's `state.vscdb`, if it is not in the default location |

## Releasing

Publishing a GitHub Release is the whole process. The tag sets `package.version`
and refreshes `Cargo.lock` (committed back to `main` when the release is its
tip), builds macOS and Linux binaries for both architectures, attaches them and
the `.deb` packages to the release, then updates the Homebrew tap and the APT
repo. Nothing needs bumping by hand before cutting the tag. Prerelease tags —
any tag containing `-` — build and attach binaries but skip both publish steps.

The publish jobs need four secrets on this repo: `HOMEBREW_TAP_DEPLOY_KEY` and
`APT_REPO_DEPLOY_KEY`, write deploy keys for `abnegate/homebrew-tap` and
`abnegate/apt-repo`, plus `APT_GPG_PRIVATE_KEY` and `APT_GPG_KEY_ID` for signing
the APT `Release` file. Deploy keys rather than a personal access token: each
one reaches exactly one repository and never expires.

## Licence

MIT
