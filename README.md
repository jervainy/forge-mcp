# ForgeMCP

ForgeMCP is both a practical workspace-scoped MCP server and a learning project that implements MCP from the protocol layer upward.

The runtime intentionally does **not** depend on an MCP SDK. JSON-RPC, MCP lifecycle handling, OAuth integration, tool discovery/calls, and transports are implemented in this repository so the wire protocol stays visible and easy to study.

## Current protocol support

The current milestone implements the handshake-era MCP protocol defined by revision `2025-11-25`:

- JSON-RPC 2.0 request/notification/error handling
- `initialize`
- `notifications/initialized`
- `ping`
- `tools/list`
- `tools/call`
- stdio transport
- Streamable HTTP with JSON and SSE responses, session-scoped event IDs, and resumable streams
- stateful HTTP sessions via `MCP-Session-Id`
- `MCP-Protocol-Version` validation
- Origin validation
- built-in OAuth 2.1 authorization server
- OAuth protected-resource and authorization-server discovery
- Dynamic Client Registration
- Authorization Code + PKCE (S256)
- Admin PIN approval with rate limiting
- audience-bound HS256 bearer tokens
- per-tool OAuth `securitySchemes` and scope enforcement
- tool schemas generated from Rust request models with `schemars`

The HTTP transport follows the MCP 2025-11-25 Streamable HTTP profile. Each client message is POSTed to `/mcp`; initialization and ordinary requests receive JSON, while `tools/call` uses `text/event-stream` with an initial event ID, optional MCP progress notifications and a final JSON-RPC response. Accepted client notifications receive `202 Accepted`. Clients can open `GET /mcp` SSE connections for server events, or resume a disconnected POST/GET stream using `Last-Event-ID`. `DELETE /mcp` terminates a session and closes its SSE streams.

A lightweight unauthenticated health endpoint is available at `GET /health`. It returns HTTP `200 OK` with `{"status":"ok"}` and intentionally exposes no workspace, OAuth, or runtime details.

## Tool scope

ForgeMCP is designed around ten tools:

1. `shell_run`
2. `file_list`
3. `file_read`
4. `file_write`
5. `file_edit`
6. `file_delete`
7. `file_search`
8. `file_patch`
9. `workspace_info`
10. `audit_list`

All ten tools are executable through `tools/call`. File and shell operations are workspace-scoped, batch-aware, output-bounded, and audited.

## Architecture

```text
                         ForgeServer
                             ^
                   +---------+---------+
                   |                   |
                stdio           Streamable HTTP
                   |                   |
             local client          OAuth auth
                                       |
                              +--------+--------+
                              |                 |
                           ChatGPT        other remote MCP

HTTP OAuth
    |
    +--> protected resource metadata
    +--> authorization server metadata
    +--> DCR
    +--> authorization + Admin PIN
    +--> token + PKCE
    |
    v
Bearer verification
    |
    v
MCP transport
    |
    v
ForgeServer
    |
    v
ToolRegistry
```

File, shell, workspace, batch, and audit code do not depend on the OAuth or MCP transport implementation.

## Prebuilt releases

Versioned binaries for macOS, Windows, and Linux are published on the [GitHub Releases page](https://github.com/jervainy/forge-mcp/releases).

Download the archive for your CPU (`x86_64` or `aarch64`/ARM64), unpack it and run `forge-mcp --help` (or `forge-mcp.exe --help` on Windows). Each archive includes a YAML example and the OAuth setup guide. Release checksums are in `SHA256SUMS.txt`.

The release workflow builds six native targets from a `release/vX.Y.Z` staging branch, runs a CLI smoke test on each target, and **only after all six succeed** publishes a GitHub Release plus the matching `vX.Y.Z` tag. For each release, update `Cargo.toml` to the new semantic version before creating the `release/vX.Y.Z` branch.

## Configuration

ForgeMCP supports an optional YAML configuration file, following the same precedence model as local-shell-mcp:

```text
built-in defaults
    <
YAML selected by --config or FORGE_MCP_CONFIG
    <
FORGE_MCP_* environment variables
    <
CLI command/flags
```

Start from [`forge-mcp.example.yaml`](forge-mcp.example.yaml):

```bash
cp forge-mcp.example.yaml ~/.config/forge-mcp/config.yaml
forge-mcp --config ~/.config/forge-mcp/config.yaml
```

The same file can be selected with an environment variable:

```bash
export FORGE_MCP_CONFIG=~/.config/forge-mcp/config.yaml
forge-mcp
```

A minimal HTTP configuration is:

```yaml
mode: serve
log_level: INFO
host: 127.0.0.1
port: 8765
workspace_root: /path/to/workspace

auth_mode: oauth
auth_bypass_localhost: true
public_base_url: https://forge.example.com

max_batch_items: 100
max_concurrency: 16
```

For secrets such as `oauth_admin_pin` and `oauth_jwt_secret`, prefer environment variables even though YAML accepts the fields.

Important environment aliases:

```text
FORGE_MCP_CONFIG
FORGE_MCP_MODE
FORGE_MCP_LOG_LEVEL
FORGE_MCP_HOST
FORGE_MCP_PORT
FORGE_MCP_WORKSPACE_ROOT
FORGE_MCP_WORKSPACE          # legacy alias
FORGE_MCP_STATE_DIR
```

`mode: stdio` selects stdio. `mode: serve`, `mode: http`, and `mode: mcp` select Streamable HTTP.

`log_level` supports `ERROR`, `WARN`, `INFO`, and `DEBUG`; the default is `INFO`. The same setting can be overridden with `FORGE_MCP_LOG_LEVEL`. After the HTTP listener is successfully bound, ForgeMCP always prints the actual listening IP/port and MCP endpoint to stderr, even when `log_level: ERROR` is configured.

## Run

### stdio

```bash
FORGE_MCP_WORKSPACE=/path/to/workspace cargo run -- stdio
```

With no command and no configured `mode`, the default remains stdio:

```bash
cargo run
```

If a YAML file sets `mode: serve`, this is enough:

```bash
cargo run -- --config ~/.config/forge-mcp/config.yaml
```

An explicit `stdio` or `serve` command overrides the YAML/environment mode.

### Streamable HTTP

Direct local development works without an OAuth login because the localhost bypass is enabled by default:

```bash
FORGE_MCP_WORKSPACE=/path/to/workspace \
cargo run -- serve --host 127.0.0.1 --port 8765
```

MCP endpoint:

```text
http://127.0.0.1:8765/mcp
```

Health check:

```bash
curl http://127.0.0.1:8765/health
```

Response:

```json
{"status":"ok"}
```

The health endpoint does not require OAuth and is suitable for Docker, Kubernetes, reverse-proxy, or tunnel health probes.

For ChatGPT or another public client, configure the external HTTPS origin and Admin PIN:

```bash
export FORGE_MCP_PUBLIC_BASE_URL="https://forge.example.com"
export FORGE_MCP_AUTH_MODE="oauth"
export FORGE_MCP_OAUTH_ADMIN_PIN="replace-with-a-long-random-pin"

cargo run -- serve --host 127.0.0.1 --port 8765
```

Expose the local HTTP endpoint through a trusted HTTPS tunnel or reverse proxy. Requests carrying forwarded proxy headers do not receive the localhost authentication bypass.

See [OAUTH_SETUP.md](OAUTH_SETUP.md) for the complete OAuth flow and settings.

## SSE over Streamable HTTP

MCP clients must send `Accept: application/json, text/event-stream` on POST and `Accept: text/event-stream` on GET. Existing `initialize`, `ping` and `tools/list` calls remain JSON responses; `tools/call` returns an SSE stream. The stream starts with an empty `data` event containing its unique ID, then sends a final JSON-RPC response and closes. If the request includes `params._meta.progressToken` (a string or integer), it also emits `notifications/progress` when execution starts and finishes.

```http
POST /mcp
Content-Type: application/json
Accept: application/json, text/event-stream
MCP-Session-Id: <session-id>
```

A separate authenticated GET listener remains open, with periodic SSE keepalive comments:

```http
GET /mcp
Accept: text/event-stream
MCP-Session-Id: <session-id>
```

SSE event IDs use `<stream-uuid>:<sequence>`. To replay only events from the same stream after a disconnect, reconnect with `GET /mcp`, `Last-Event-ID: <stream-uuid>:<sequence>`, and the same authenticated session. The handler returns `404` for an unknown stream, `410` if replay history expired, and `409` if the same stream still has an active connection. Replay records are in memory only (up to 256 events per stream, kept for approximately 10 minutes), so restarting ForgeMCP invalidates all cursors. Per-session stream creation is limited to prevent unbounded memory use.

The transport can deliver server-initiated MCP messages on GET SSE streams once a server feature emits them; no background file-watch/resource subscriptions are currently implemented. SSE does **not** yet stream raw shell stdout/stderr or granular batch-item progress. The tool still returns its final bounded result; the optional progress messages indicate start and completion only. Disconnection does not cancel the running command.

`GET /health` stays a normal JSON endpoint, with no authentication or SSE.

## OAuth scopes

```text
forge:read       file_list, file_read, file_search, workspace_info
forge:write      file_write, file_edit, file_delete, file_patch
forge:execute    shell_run
forge:audit      audit_list
```

OAuth client registrations and the generated JWT signing secret are persisted in the ForgeMCP state directory. Authorization codes remain short-lived and memory-only.

## CLI

```text
forge-mcp [--config <path>]
forge-mcp [--config <path>] stdio
forge-mcp [--config <path>] serve [--host <host>] [--port <port>]
forge-mcp --help
```

`--config` may also appear after `serve`, for example `forge-mcp serve --config forge.yaml --port 9000`.

## Runtime behavior

- `shell_run`: one-shot shell commands with workspace-scoped cwd, timeout, bounded stdout/stderr, exit code, and batch concurrency.
- `file_list`: bounded-depth directory listing without following symlinks.
- `file_read`: line-range UTF-8 reads with output truncation metadata.
- `file_write`: create/overwrite modes with optional parent-directory creation.
- `file_edit`: string replacement and 1-based inclusive line-range replacement.
- `file_delete`: file/directory deletion with recursive and ignore-missing options.
- `file_search`: glob plus literal/regex text search with bounded result counts.
- `file_patch`: ordered unified-diff application, including create/modify/delete/rename cases.
- `workspace_info`: workspace/platform/shell/Git context.
- `audit_list`: filtered recent action history from the persisted JSONL audit log.

Read-oriented batches run concurrently by default. Mutation batches preserve input order. Setting `fail_fast=true` executes in order and marks later items as skipped after the first failure.

All path-bearing tools pass through `WorkspaceGuard`, which rejects lexical escapes and symlink resolutions outside `FORGE_MCP_WORKSPACE`. ForgeMCP also enforces configurable limits:

```bash
FORGE_MCP_MAX_BATCH_ITEMS=100
FORGE_MCP_MAX_CONCURRENCY=16
FORGE_MCP_MAX_READ_BYTES=1048576
FORGE_MCP_MAX_WRITE_BYTES=4194304
FORGE_MCP_MAX_SHELL_OUTPUT_BYTES=1048576
FORGE_MCP_SHELL_TIMEOUT_MS=30000
```

Audit records are stored under `FORGE_MCP_STATE_DIR` (or the default ForgeMCP config directory) in `audit.jsonl`. File contents, shell stdout/stderr, and environment-variable values are not written to audit records.

## Roadmap

1. JSON-RPC 2.0 + MCP 2025 handshake lifecycle
2. stdio + Streamable HTTP
3. OAuth 2.1 for ChatGPT
4. Ten workspace tools + batch executor + WorkspaceGuard + audit
5. Incremental shell output, dynamic resource subscriptions, and server-originated MCP notifications
6. MCP `2026-07-28` stateless lifecycle and `server/discover`
7. Cross-check compatibility with official MCP SDK clients
