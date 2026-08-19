# peek-mcp

`peek-mcp` is the MCP transport for `peek-api`.

## Named tools

- `peek_read`
- `peek_grep`
- `peek_count`
- `peek_skeleton`

Each tool delegates to `peek-api` so range validation, regex handling, filesystem behavior, and structural rendering stay consistent with `peek-cli`.