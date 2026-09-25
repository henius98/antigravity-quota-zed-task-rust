# Antigravity Quota — Rust + Zed Task

Checks Google Antigravity quota without MCP, without `agy`, and without an LLM call.

## Data path

```text
Zed task
  -> ~/.local/bin/antigravity-quota
  -> ~/.gemini/antigravity-acp/acp_token.json
  -> Google Cloud Code PA internal API
  -> terminal output in Zed
```

No model request is made, so running this task does not consume Gemini/Claude/GPT
chat tokens.

## Build

```bash
cargo build --release
```

Or:

```bash
./scripts/install.sh
```

The installer copies the binary to:

```text
~/.local/bin/antigravity-quota
```

## Zed task

Open the global tasks file:

```text
Ctrl+Shift+P
zed: open tasks
```

Merge the entries from:

```text
zed/tasks.json
```

Run:

```text
Ctrl+Shift+P
task: spawn
Antigravity: Check quota
```

Optional: merge `zed/keymap.json` into `~/.config/zed/keymap.json` for:

```text
Ctrl+Alt+Q
```

## CLI

```bash
antigravity-quota
antigravity-quota --json
antigravity-quota --raw
```

`--json` prints normalized quota data. `--raw` prints Google's quota response.
The two flags cannot be combined, and unknown arguments return an error.

Custom token file:

```bash
antigravity-quota --token-file /path/to/acp_token.json
```

or:

```bash
export ANTIGRAVITY_ACP_TOKEN_FILE=/path/to/acp_token.json
```

## Expected credential

Default:

```text
~/.gemini/antigravity-acp/acp_token.json
```

The program only reads it. It does not modify or copy OAuth credentials.

## Google calls

1. `POST /v1internal:loadCodeAssist`
2. Read `cloudaicompanionProject`
3. `POST /v1internal:retrieveUserQuotaSummary`
4. Print grouped remaining quota and reset timestamps

Primary base URL:

```text
https://daily-cloudcode-pa.googleapis.com
```

Fallback:

```text
https://cloudcode-pa.googleapis.com
```

If either API call fails on the primary host, the checker retries both calls
on the fallback host. A successful response without a `groups` field is
reported as an unexpected API response instead of an empty quota.

These are internal/undocumented Google APIs. They can change without notice.

## Authentication & Token Handling

The program supports both direct access tokens and OAuth refresh token credentials
stored in `acp_token.json`:

- If an `access_token` is present in the file, it is used directly.
- If only OAuth refresh credentials (`refresh_token`, `client_id`, etc.) are present,
  the program automatically exchanges the refresh token with Google's OAuth endpoint
  in memory to obtain a fresh access token without modifying `acp_token.json`.
- If Google returns 401/403 or the refresh token expires, open or re-authenticate the
  official Antigravity ACP agent in Zed, then run the quota task again.

## Security

- OAuth access and refresh tokens are never printed.
- Token is only sent to Google's Cloud Code PA endpoint as a Bearer token.
- No MCP server.
- No AI request.
- No token/quota consumed by the quota checker itself, apart from ordinary
  HTTPS quota-accounting API traffic.
