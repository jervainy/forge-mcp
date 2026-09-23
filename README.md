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
- Streamable HTTP transport using JSON responses
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

The HTTP transport uses the JSON response profile of Streamable HTTP: each client message is a POST to `/mcp`, JSON-RPC requests receive `application/json`, accepted notifications receive `202 Accepted`, GET returns `405 Method Not Allowed` because server-initiated SSE is not implemented yet, and DELETE can terminate a session.

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

## Run

### stdio

```bash
FORGE_MCP_WORKSPACE=/path/to/workspace cargo run -- stdio
```

The default command remains stdio:

```bash
cargo run
```

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

For ChatGPT or another public client, configure the external HTTPS origin and Admin PIN:

```bash
export FORGE_MCP_PUBLIC_BASE_URL="https://forge.example.com"
export FORGE_MCP_AUTH_MODE="oauth"
export FORGE_MCP_OAUTH_ADMIN_PIN="replace-with-a-long-random-pin"

cargo run -- serve --host 127.0.0.1 --port 8765
```

Expose the local HTTP endpoint through a trusted HTTPS tunnel or reverse proxy. Requests carrying forwarded proxy headers do not receive the localhost authentication bypass.

See [OAUTH_SETUP.md](OAUTH_SETUP.md) for the complete OAuth flow and settings.

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
forge-mcp [stdio]
forge-mcp serve [--host <host>] [--port <port>]
forge-mcp --help
```

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
5. SSE/server-to-client Streamable HTTP support where needed
6. MCP `2026-07-28` stateless lifecycle and `server/discover`
7. Cross-check compatibility with official MCP SDK clients
