# ForgeMCP

ForgeMCP is both a practical workspace-scoped MCP server and a learning project that implements MCP from the protocol layer upward.

The runtime intentionally does **not** depend on an MCP SDK. JSON-RPC, MCP lifecycle handling, tool discovery/calls, and transports are implemented in this repository so the wire protocol stays visible and easy to study.

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
- `MCP-Protocol-Version` validation for HTTP session requests
- Origin validation for browser-originated HTTP requests
- tool schemas generated from Rust request models with `schemars`

The HTTP transport intentionally starts with the simplest valid Streamable HTTP profile: each client message is a POST to `/mcp`, JSON-RPC requests receive `application/json`, accepted notifications receive `202 Accepted`, GET returns `405 Method Not Allowed` because server-initiated SSE is not implemented yet, and DELETE can terminate a session.

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

Batch-capable tools use a common `BatchRequest<T>` shape. Read operations may execute concurrently; mutation operations will preserve input order by default.

## Architecture

```text
                     ForgeServer
                         ^
              +----------+----------+
              |                     |
           stdio              Streamable HTTP
              ^                     ^
              |                     |
         local host             remote client

transport
    |
    v
protocol::jsonrpc
    |
    v
server::ForgeServer
    |
    +--> protocol::mcp
    |
    v
server::tool_registry
    |
    v
application/runtime
```

The dependency direction is intentional: file, shell, workspace, batch, and audit code must not depend on MCP protocol types.

## Run

### stdio

The default remains stdio for backwards compatibility:

```bash
FORGE_MCP_WORKSPACE=/path/to/workspace cargo run
```

or explicitly:

```bash
FORGE_MCP_WORKSPACE=/path/to/workspace cargo run -- stdio
```

### Streamable HTTP

```bash
FORGE_MCP_WORKSPACE=/path/to/workspace \
cargo run -- serve --host 127.0.0.1 --port 8765
```

The MCP endpoint is:

```text
http://127.0.0.1:8765/mcp
```

ForgeMCP binds to `127.0.0.1` and port `8765` by default:

```bash
cargo run -- serve
```

Binding to a non-loopback interface exposes command/file capabilities to the network. Authentication is not implemented yet, so keep the default loopback binding unless another trusted layer provides authentication and access control.

## Raw HTTP example

Initialize a session:

```bash
curl -i http://127.0.0.1:8765/mcp \
  -X POST \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"manual-client","version":"0.1.0"}}}'
```

The response includes an `MCP-Session-Id` header. Use that value for later requests:

```bash
curl -i http://127.0.0.1:8765/mcp \
  -X POST \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -H 'MCP-Session-Id: <session-id>' \
  -H 'MCP-Protocol-Version: 2025-11-25' \
  -d '{"jsonrpc":"2.0","method":"notifications/initialized"}'
```

Then call tools:

```bash
curl -i http://127.0.0.1:8765/mcp \
  -X POST \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -H 'MCP-Session-Id: <session-id>' \
  -H 'MCP-Protocol-Version: 2025-11-25' \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}'
```

## CLI

```text
forge-mcp [stdio]
forge-mcp serve [--host <host>] [--port <port>]
forge-mcp --help
```

## Roadmap

1. JSON-RPC 2.0 + MCP 2025 handshake lifecycle
2. stdio + Streamable HTTP
3. Implement the ten workspace tools and common batch executor
4. Workspace security, limits, and audit persistence
5. SSE/server-to-client Streamable HTTP support where needed
6. MCP `2026-07-28` stateless lifecycle and `server/discover`
7. Cross-check compatibility with official MCP SDK clients
