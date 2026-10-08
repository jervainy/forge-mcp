# ForgeMCP OAuth setup

ForgeMCP includes a small OAuth 2.1 authorization server for personal ChatGPT/MCP deployments. It follows the same practical model as local-shell-mcp: Dynamic Client Registration (DCR), Authorization Code + PKCE (S256), an owner Admin PIN approval screen, and audience-bound HS256 bearer tokens.

## Endpoints

When OAuth is enabled, ForgeMCP publishes:

- `GET /.well-known/oauth-protected-resource`
- `GET /.well-known/oauth-authorization-server`
- `POST /oauth/register`
- `GET /oauth/authorize`
- `POST /oauth/authorize`
- `POST /oauth/token`
- protected MCP endpoint: `/mcp`

## Configuration

OAuth is the default HTTP authentication mode. Settings may come from YAML or `FORGE_MCP_*` environment variables. Precedence is:

```text
defaults < YAML < environment < CLI
```

For example:

```yaml
mode: serve
host: 127.0.0.1
port: 8765
auth_mode: oauth
auth_bypass_localhost: true
public_base_url: https://forge.example.com
oauth_access_token_ttl_s: 0
oauth_code_ttl_s: 300
```

Run it with:

```bash
forge-mcp --config /path/to/config.yaml
```

or set `FORGE_MCP_CONFIG`. Keep the Admin PIN and JWT secret in environment variables when practical.

OAuth is the default HTTP authentication mode. Direct localhost requests are bypassed by default for development, but a request that arrived through a reverse proxy/tunnel does not inherit that bypass.

For a public ChatGPT endpoint via ngrok or another HTTPS tunnel, the public origin can be inferred automatically and **does not have to be known at startup**:

```yaml
mode: serve
host: 127.0.0.1
port: 8765
auth_mode: oauth
auth_bypass_localhost: false
# public_base_url: omit for automatic tunnel detection
```

Provide the Admin PIN through an environment variable (mandatory in HTTP OAuth mode, even without `public_base_url`):

```bash
export FORGE_MCP_OAUTH_ADMIN_PIN="replace-with-a-long-random-pin"
forge-mcp --config ~/.config/forge-mcp/config.yaml
ngrok http 8765
```

ForgeMCP reads the public Host and HTTPS information from requests (and accepts forwarded host/protocol metadata only from a loopback-connected proxy). It uses the resulting origin for protected-resource metadata, authorization-server metadata, JWT issuer/audience, and validation. The external HTTPS URL may change between ngrok sessions; after a URL change, reconnect the ChatGPT custom MCP server using the new `https://.../mcp` URL and repeat OAuth authorization.

Security: **your tunnel or reverse proxy must enforce which public Host values reach ForgeMCP**. Host and forwarding headers are client-controlled unless verified/overwritten by your trusted ingress. For production, prefer a fixed `public_base_url` or strict proxy host validation; never expose the plain HTTP listener directly to the internet. If needed, setting `public_base_url: https://forge.example.com` still overrides automatic discovery.

ForgeMCP creates and persists a random 64-character HS256 secret in:

```text
~/.config/forge-mcp/oauth-jwt-secret
```

You can instead supply it explicitly:

```bash
export FORGE_MCP_OAUTH_JWT_SECRET="<at-least-32-bytes-of-random-data>"
```

Optional settings:

```bash
export FORGE_MCP_STATE_DIR="$HOME/.config/forge-mcp"
export FORGE_MCP_OAUTH_ISSUER="https://forge.example.com"
export FORGE_MCP_OAUTH_RESOURCE="https://forge.example.com"
export FORGE_MCP_OAUTH_ACCESS_TOKEN_TTL_S=0
export FORGE_MCP_OAUTH_CODE_TTL_S=300
export FORGE_MCP_AUTH_BYPASS_LOCALHOST=true
```

`FORGE_MCP_OAUTH_ACCESS_TOKEN_TTL_S=0` means access tokens do not automatically expire. This mirrors local-shell-mcp's default and avoids requiring refresh-token support in the first OAuth implementation. For a multi-user or internet-facing production service, short-lived access tokens plus refresh-token rotation should be added.

When `FORGE_MCP_PUBLIC_BASE_URL` is configured, ForgeMCP requires HTTPS. For all HTTP OAuth deployments, an Admin PIN of at least 8 non-placeholder characters is required whether the public URL is configured or automatically discovered.

## Scopes

ForgeMCP advertises four scopes:

- `forge:read` — file listing/read/search and workspace information
- `forge:write` — file create/edit/delete/patch
- `forge:execute` — shell execution
- `forge:audit` — audit history

Each MCP tool declares its OAuth requirement through `securitySchemes`. ForgeMCP also enforces scopes at call time.

## ChatGPT flow

```text
ChatGPT
  |
  | GET /.well-known/oauth-protected-resource
  v
ForgeMCP
  |
  | OAuth server metadata
  v
/.well-known/oauth-authorization-server
  |
  | Dynamic Client Registration
  v
POST /oauth/register
  |
  | Authorization Code + PKCE (S256)
  v
GET /oauth/authorize
  |
  | owner enters Admin PIN
  v
POST /oauth/authorize
  |
  | authorization code
  v
POST /oauth/token
  |
  | audience-bound bearer token
  v
POST /mcp
Authorization: Bearer <token>
```

OAuth client registrations are persisted under the ForgeMCP state directory so a registered ChatGPT client remains valid across server restarts. Authorization codes and PIN rate-limit state are intentionally memory-only.

## Local development

With the defaults, a direct request from loopback can use `/mcp` without OAuth. The bypass is deliberately strict: the TCP peer must be loopback, the Host header must also be loopback, and no `Forwarded`/`X-Forwarded-*` metadata may be present. This prevents a Cloudflare Tunnel or reverse proxy connected over localhost from accidentally bypassing OAuth for public users.

Disable the local bypass when testing the OAuth flow itself:

```bash
export FORGE_MCP_AUTH_BYPASS_LOCALHOST=false
export FORGE_MCP_OAUTH_ADMIN_PIN="replace-with-a-long-random-pin"
cargo run -- serve
```

## Troubleshooting ChatGPT client registration

If ChatGPT reports that it could not register an OAuth client, the failure occurs before the Admin PIN screen. Verify the public HTTPS URL and metadata first:

```bash
BASE="https://your-public-host.example.com"
curl -i "$BASE/health"
curl -sS "$BASE/.well-known/oauth-protected-resource"
curl -sS "$BASE/.well-known/oauth-authorization-server"
```

The authorization-server metadata must advertise `registration_endpoint` pointing to the public origin, not `localhost`. In automatic mode, verify the discovered external HTTPS origin and confirm your ingress passes a public Host (or an `X-Forwarded-Host` from a trusted local proxy). If the returned issuer is localhost or HTTP, check proxy host/HTTPS forwarding; setting `public_base_url` remains an explicit fallback but is not required for normal ngrok usage.

To test Dynamic Client Registration without accessing ChatGPT, use a test redirect URI:

```bash
curl -i -X POST "$BASE/oauth/register" \
  -H 'Content-Type: application/json' \
  --data '{
    "client_name": "DCR test",
    "redirect_uris": ["https://chatgpt.com/connector/oauth/test"],
    "grant_types": ["authorization_code", "refresh_token"],
    "response_types": ["code"],
    "token_endpoint_auth_method": "none"
  }'
```

A successful registration returns `201 Created`, a JSON `client_id`, and `grant_types: ["authorization_code"]`. ForgeMCP intentionally does **not** advertise refresh-token support or issue refresh tokens. Enabling `log_level: DEBUG` shows the requested grant types, while rejected registrations are logged at WARN with a reason.

If the response is HTML rather than JSON, check your tunnel/proxy configuration, including possible ngrok browser-warning or error pages. If it returns HTTP `400` with `invalid_client_metadata`, inspect the logged rejected metadata reason. Avoid configuring `auth_bypass_localhost: true` for public endpoints; explicit `false` is safest behind tunnels or reverse proxies.
