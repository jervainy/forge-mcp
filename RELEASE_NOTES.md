# ForgeMCP v0.1.0

First public binary release of ForgeMCP, a from-scratch Rust MCP server.

## Highlights

- MCP 2025-11-25 JSON-RPC lifecycle over stdio and Streamable HTTP.
- Built-in OAuth Authorization Code + PKCE (S256), Dynamic Client Registration, Admin PIN approval, JWT access tokens and per-tool scopes.
- Automatic HTTPS tunnel origin discovery when `public_base_url` is omitted.
- Workspace-scoped shell/file tools, batch operations, audit logging, YAML configuration and `GET /health`.
- Four configurable log levels: ERROR, WARN, INFO and DEBUG.

## Binaries

| System | CPU architecture | Asset suffix |
| --- | --- | --- |
| macOS | Intel / x86_64 | `x86_64-apple-darwin.tar.gz` |
| macOS | Apple Silicon / ARM64 | `aarch64-apple-darwin.tar.gz` |
| Windows | x86_64 | `x86_64-pc-windows-msvc.zip` |
| Windows | ARM64 | `aarch64-pc-windows-msvc.zip` |
| Linux | x86_64 (glibc) | `x86_64-unknown-linux-gnu.tar.gz` |
| Linux | ARM64 (glibc) | `aarch64-unknown-linux-gnu.tar.gz` |

Assets contain `forge-mcp` (or `forge-mcp.exe`), `README.md`, `OAUTH_SETUP.md`, and `forge-mcp.example.yaml`.

Verify file integrity with `SHA256SUMS.txt`.

**Limitations:** GET /mcp responds with HTTP 405; server-initiated SSE isn't implemented yet. OAuth access tokens default to no expiry. Filesystem tools are workspace-scoped; `shell_run` executes actual shell commands with the permissions of the running user. Avoid exposing the HTTP listener directly to the internet and use a trusted HTTPS tunnel.
