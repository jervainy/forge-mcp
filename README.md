# ForgeMCP

ForgeMCP is a workspace-scoped MCP server for local shell and file operations.

## Scope

ForgeMCP intentionally starts with ten tools:

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

Batch-capable tools accept a common `BatchRequest<T>` model. Read operations may execute concurrently; mutation operations preserve item order by default.

## Design goals

- Workspace-scoped access
- Batch-native tool APIs
- Structured per-item results
- Auditable operations
- Bounded concurrency
- No transactional guarantee for a batch in v0.1
- stdio first, Streamable HTTP next

## Status

Initial project skeleton.
