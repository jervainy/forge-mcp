# ForgeMCP v0.2.0

## What's new

- **Streamable HTTP SSE:** `POST /mcp` tool calls now return `text/event-stream` with the final JSON-RPC response.
- **SSE listeners:** Authenticated `GET /mcp` supports an open SSE connection with periodic keepalive comments (replacing the previous `405 Method Not Allowed` behavior for valid requests).
- **Reconnect and replay:** SSE events have stream-scoped IDs, and `Last-Event-ID` can resume missed events for the authenticated MCP session.
- **Progress notifications:** If the client supplies `params._meta.progressToken`, ForgeMCP sends start and completion `notifications/progress`.
- **Session safety:** Event streams are isolated per session, stored in bounded memory, and stopped when the associated session is deleted.
- SSE transport behavior and tests documented in `README.md`.

## Download

This version provides **six native binaries**, all built and smoke-tested on matching OS/architecture runners:

| Operating system | Architecture | Asset format |
| --- | --- | --- |
| macOS | x86_64 (Intel) | `.tar.gz` |
| macOS | aarch64 (Apple Silicon) | `.tar.gz` |
| Windows | x86_64 | `.zip` |
| Windows | aarch64 (ARM64) | `.zip` |
| Linux (glibc) | x86_64 | `.tar.gz` |
| Linux (glibc) | aarch64 (ARM64) | `.tar.gz` |

Each archive includes the executable, README, OAuth setup instructions, and YAML configuration example. Use `SHA256SUMS.txt` to verify downloads.

## Compatibility

- MCP protocol version: `2025-11-25`.
- `stdio`, JSON responses for initialization/ordinary calls, OAuth, the ten workspace tools, YAML configuration, and `GET /health` remain available.
- **SSE is a transport enhancement**, not a real-time shell terminal: `shell_run` output is still returned on completion; optional progress notifications indicate only start and completion.
- No background filesystem watch/resource subscriptions are provided yet.
- OAuth JWT access tokens default to non-expiring, as in the previous release. Treat exposed shell/file access as sensitive and use trusted HTTPS proxying.

**Upgrade:** Replace your existing `forge-mcp` executable with the v0.2.0 binary for your platform and restart the service; the YAML configuration format is unchanged.
