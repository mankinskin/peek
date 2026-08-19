# peek-api

`peek-api` owns the shared token-bounded inspection behavior used by `peek` transports.

## Responsibilities

- bounded file inspection modes such as count, grep, head, tail, and explicit ranges
- file and directory skeleton rendering
- repository map generation primitives reused by `peek-cli`
- shared request, response, and error types for transports

## Intended layering

- `memory-api/crates/peek-api`: domain behavior and validation
- `memory-api/tools/cli/peek-cli`: clap parsing and text rendering
- `memory-api/tools/mcp/peek-mcp`: named MCP tools delegating to `peek-api`