# aiusg

One place to see how much of your AI subscription you have left.

Authenticate any number of accounts — including several on the same provider — and
`aiusg` fetches every account's limits in parallel and prints them as one table.

```text
  Codex     jake@example.com  pro
    7d                     ████████████████████ 100%              resets in 5d 21h
    GPT-5.3-Codex-Spark 5h ░░░░░░░░░░░░░░░░░░░░   0%              resets in 4h 59m

  Copilot   octocat  individual
    Premium requests       ████████████████████ 100%  1506/1500   resets in 21d 19h

  Grok      jake@example.com  SuperGrok Heavy
    GrokBuild              ████████████████████ 100%              resets in 1d 21h

  Grok Bot  jake@example.com  Grok Bot Plan
    Included usage         █████████░░░░░░░░░░░  43%              resets in 3d 20h
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

**Binary** from [Releases](https://github.com/abnegate/aiusg/releases), or build it with Cargo:

```bash
cargo install aiusg
cargo install --git https://github.com/abnegate/aiusg   # unreleased main
```

Installing the binary uses the default features, so it always has every
provider, sign-in and the MCP server.

## Use

```bash
aiusg import              # adopt accounts already signed in to their CLI
aiusg login claude        # or sign in directly — repeat for each account
aiusg                     # show every account
aiusg --provider claude   # just one provider
aiusg --all               # include accounts that are signed out
aiusg --sort usable       # most usable right now first (`--sort` on its own does too)
aiusg --json              # machine readable, for status lines and scripts
aiusg --watch             # live dashboard every 30s, r refresh, s reorder, q quit
aiusg --watch 10          # ...or every 10s (--follow and --interval also work)
aiusg list                # stored accounts
aiusg remove claude:jake@example.com
aiusg mcp                 # MCP server over stdio, for agents
```

Multiple accounts on one provider are the point: run `aiusg login claude` once per
account and each is stored separately, keyed by `provider:label`.

`aiusg --sort usable` ranks the table by what is worth reaching for right now:
most headroom first, then the accounts that are spent — soonest back first — then
the ones that failed or are signed out. Headroom is the same measure the MCP
`route` tool uses. Watch mode re-ranks on every refresh, and `s` switches between
that order and the stored one without waiting for the next fetch.

## MCP server

`aiusg mcp` speaks MCP over stdio, so an agent can read every account's remaining
usage and pick which one to send work to.

| Tool | What it does |
|---|---|
| `route` | Ranks accounts by headroom and returns the one with the most left, plus alternatives and why the rest are out (exhausted, signed out, failing) with reset times |
| `usage` | Every account's windows — used percent, counts, reset times |
| `accounts` | Stored accounts, without fetching usage |

`route` and `usage` take an optional `provider` to scope to one of `claude`,
`codex`, `gemini`, `copilot`, `grok`, `grokbot`, `cursor`. Headroom is `100 -` the used
percent of the account's most-consumed window, so the window closest to its cap
decides the ranking. When nothing has headroom left, `route` returns a tool error
carrying the reset times, so the caller can wait rather than retry blindly. The
reset time on an exhausted account is when its *last* spent window comes back,
not its most-consumed one, so waiting that long is enough.

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

## Use as a library

The fetchers are a library too. Turn the default features off to leave the CLI,
its terminal UI and the MCP server out of your build:

```toml
[dependencies]
aiusg = { version = "0.5", default-features = false }
```

With no features you get Claude, Codex, Gemini and Grok usage, credential
discovery from each provider's own CLI directory, and token refresh. Add only what
you need:

| Feature | Adds |
|---|---|
| `default` | `cli` |
| `cli` | The `aiusg` binary: table, watch mode and MCP server (`clap`, `crossterm`, `rmcp`, `schemars`), plus every feature below |
| `login` | Interactive browser sign-in for each provider (`webbrowser`) |
| `keychain` | The OS keychain credential backend, and reading CLI tokens kept in the keychain (`keyring`) |
| `copilot` | The Copilot provider, including the Copilot CLI's `~/.copilot/data.db` session store (`rusqlite`) |
| `cursor` | The Cursor provider, which reads the Cursor app's `state.vscdb` (`rusqlite`) |
| `grokbot` | The Grok Bot provider; implies `cursor`, whose session it rides on |

A provider left out of the build is still in `Provider::ALL`, and calling it
returns an error naming the feature that adds it.

The Claude and Codex fetchers have an `_at` variant that takes the API base, so
you can point them at a proxy or a local mock. The base is an origin, with or
without a trailing slash; `BASE` is the real one. `claude::fetch_at` reads the
plan from the account profile too, so it needs no separate `profile_at` call:

```rust,no_run
use aiusg::model::{Account, Availability, Provider};
use aiusg::provider::{claude, is_signed_out};
use aiusg::store::Credential;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let http = reqwest::Client::new();
    let credential = Credential::bearer(std::env::var("CLAUDE_ACCESS_TOKEN")?);
    let account = Account::new(Provider::Claude, "jake@example.com", None);

    match claude::fetch_at(&http, claude::BASE, &credential).await {
        Ok(fetched) => {
            let usage = fetched.into_usage(&account);
            for window in &usage.windows {
                let used = window
                    .used_percent
                    .map_or_else(|| "unknown".to_owned(), |percent| format!("{percent:.0}%"));
                let resets = window
                    .resets_at
                    .map_or_else(|| "no reset reported".to_owned(), |at| format!("resets {at}"));
                println!("{}: {used} used, {resets}", window.name);
            }
            match usage.availability() {
                Availability::Now => println!("{:.0}% headroom", usage.headroom()),
                Availability::At(time) => println!("spent until {time}"),
                Availability::Unknown => println!("spent, with no reset reported"),
            }
        }
        Err(error) if is_signed_out(&error) => eprintln!("the token was refused; sign in again"),
        Err(error) => return Err(error),
    }
    Ok(())
}
```

`claude::discover_in`, `codex::discover_in` and `grok::discover_in` read the
credential a provider's CLI stored in that CLI's own directory, the one
`CLAUDE_CONFIG_DIR`, `CODEX_HOME` or `GROK_HOME` names (such as `~/.codex`),
not the user's home. A missing or blank credentials file gives no logins, and
one that cannot be read or parsed is an error. `Fetched::into_usage` turns a
fetch into the same `Usage` the CLI ranks, with `headroom()` and
`availability()`; `into_usage_at` takes the fetch time instead of using now.
`is_signed_out` tells a refused token (a 401 or 403 anywhere in the error
chain) apart from any other failure. Set `AIUSG_DEBUG=1` in the calling process to
print every raw provider response, with its HTTP status, to stderr.

## Providers

| Provider | Source | What you get |
|---|---|---|
| Claude | `GET api.anthropic.com/api/oauth/usage` | Session and weekly windows from `limits[]`, including per-model ones, plus extra usage credits |
| Codex | `GET chatgpt.com/backend-api/wham/usage` | Primary and secondary windows plus per-model buckets, with reset times |
| Copilot | `GET api.github.com/copilot_internal/user` | Premium request quota, used/entitlement, monthly reset |
| Gemini | `POST cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota` | Per-model remaining requests and reset time (needs an OAuth client, below) |
| Grok | `GET cli-chat-proxy.grok.com/v1/billing?format=credits` | Credit usage per product, billing period reset |
| Grok Bot | `POST api2.cursor.sh/aiserver.v1.DashboardService/GetSandUsageStatus` | Included usage percent for the current period, plan label and reset time |
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

### Grok Bot rides on the Cursor session

The Grok Bot desktop app is an Anysphere product, so it signs in with the same
account the Cursor app uses and reports through Cursor's dashboard API. Like
Cursor, it is import-only: `aiusg login grokbot` reads a session that already
exists rather than driving a sign-in of its own.

The app keeps its tokens in `sand-secrets.json`, encrypted with the OS keychain
on macOS and Windows, so `aiusg` reads them only where they are stored in the
clear and otherwise falls back to the session the Cursor app holds. Point
`AIUSG_GROKBOT_SECRETS` at the file to override where it is read from.

Usage pooled into an enterprise allowance is not this account's to report, so
those accounts show no window rather than a misleading one.

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
| `AIUSG_GROKBOT_SECRETS` | Path to the Grok Bot app's `sand-secrets.json`, if it is not in the default location |

## Releasing

Publishing a GitHub Release is the whole process. The tag sets `package.version`
and refreshes `Cargo.lock` (committed back to `main` when the release is its
tip), builds macOS and Linux binaries for both architectures, attaches them and
the `.deb` packages to the release, updates the Homebrew tap and the APT repo,
and publishes the crate to crates.io. Nothing needs bumping by hand before
cutting the tag. Prerelease tags — any tag containing `-` — build and attach
binaries but skip every publish step.

The crates.io job needs `CARGO_REGISTRY_TOKEN`, a crates.io API token scoped to
publishing `aiusg`. Without it, or when that version is already on crates.io,
the job skips with a notice rather than failing the release.

The Homebrew and APT jobs need four more secrets: `HOMEBREW_TAP_DEPLOY_KEY` and
`APT_REPO_DEPLOY_KEY`, write deploy keys for `abnegate/homebrew-tap` and
`abnegate/apt-repo`, plus `APT_GPG_PRIVATE_KEY` and `APT_GPG_KEY_ID` for signing
the APT `Release` file. Deploy keys rather than a personal access token: each
one reaches exactly one repository and never expires.

## Licence

MIT
