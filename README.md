# ForgeMCP

ForgeMCP is both a practical workspace-scoped MCP server and a learning project that implements MCP from the protocol layer upward.

The runtime intentionally does **not** depend on an MCP SDK. JSON-RPC, MCP lifecycle handling, tool discovery/calls, and transports are implemented in this repository so the wire protocol stays visible and easy to study.

## Current protocol support

The first protocol milestone implements the handshake-era MCP protocol defined by revision `2025-11-25`:

- JSON-RPC 2.0 request/notification/error handling
- `initialize`
- `notifications/initialized`
- `ping`
- `tools/list`
- `tools/call`
- stdio transport
- tool schemas generated from Rust request models with `schemars`

The stdio transport keeps stdout protocol-clean. Logs are written to stderr.

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
stdin/stdout
    |
    v
transport::stdio
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

```bash
FORGE_MCP_WORKSPACE=/path/to/workspace cargo run
```

A client normally launches ForgeMCP and communicates over stdin/stdout. For learning, you can also send raw newline-delimited JSON-RPC messages manually.

Example initialization:

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"manual-client","version":"0.1.0"}}}
```

Then send:

```json
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"workspace_info","arguments":{}}}
```

## Roadmap

1. JSON-RPC 2.0 + MCP 2025 handshake lifecycle + stdio
2. Implement the ten workspace tools and common batch executor
3. Workspace security, limits, and audit persistence
4. Streamable HTTP
5. MCP `2026-07-28` stateless lifecycle and `server/discover`
6. Cross-check compatibility with official MCP SDK clients
