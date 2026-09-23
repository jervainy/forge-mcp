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

`workspace_info` is already executable through `tools/call`. The other tool contracts are exposed so the next milestones can implement their application/runtime behavior without changing the MCP protocol layer.

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

## Roadmap

1. JSON-RPC 2.0 + MCP 2025 handshake lifecycle
2. stdio + Streamable HTTP
3. OAuth 2.1 for ChatGPT
4. Implement the ten workspace tools and common batch executor
5. Workspace security, limits, and audit persistence
6. SSE/server-to-client Streamable HTTP support where needed
7. MCP `2026-07-28` stateless lifecycle and `server/discover`
8. Cross-check compatibility with official MCP SDK clients
