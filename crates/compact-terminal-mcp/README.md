# compact-terminal-mcp

MCP server that provides a compact terminal tool. Long command outputs are
automatically truncated and spilled to a transient file so that only a short
summary is returned inline, keeping token consumption bounded.

## Tools

### `run`

Run a shell command (`sh -c`). Returns:

- **Short outputs** (≤ `inline_limit`, default 4096 bytes): full stdout/stderr inline.
- **Long outputs**: truncated preview + path to a transient spill file containing the
  full output stream.

```json
{
  "command": "cargo test -p context-read 2>&1",
  "cwd": "/path/to/repo",
  "inline_limit": 4096,
  "timeout_secs": 60
}
```

**Response when short:**

```json
{
  "kind": "inline",
  "exit_code": 0,
  "stdout": "...",
  "stderr": "",
  "elapsed_ms": 1234
}
```

**Response when long (spilled):**

```json
{
  "kind": "spilled",
  "exit_code": 1,
  "stdout_preview": "...first 2048 chars...",
  "stderr_preview": "...first 2048 chars...",
  "total_bytes": 98432,
  "total_lines": 1823,
  "spill_file": "/tmp/compact-terminal-mcp/abc123.txt",
  "elapsed_ms": 8321,
  "next_steps": [
    "peek \"/tmp/.../abc123.txt\" --count",
    "peek \"/tmp/.../abc123.txt\" --grep \"error\" --window 10",
    "peek \"/tmp/.../abc123.txt\" --head 30",
    "Use read_spill with start/end or grep to inspect targeted sections"
  ]
}
```

### `read_spill`

Read a bounded window from a spill file returned by `run`. Use this instead of
re-running the command to inspect specific sections.

```json
{
  "spill_file": "/tmp/compact-terminal-mcp/abc123.txt",
  "start": 100,
  "end": 130
}
```

Or search by pattern:

```json
{
  "spill_file": "/tmp/compact-terminal-mcp/abc123.txt",
  "grep": "FAILED"
}
```

## Usage Pattern

```
1. run("cargo test -p my-crate")
   → spilled: preview + spill_file path

2. read_spill(spill_file, grep="FAILED")
   → matching line numbers: 142, 287, 891

3. read_spill(spill_file, start=140, end=155)
   → the failing test details

4. Fix the issue; re-run only the targeted test
```

## Configuration

| Env var | Default | Description |
|---|---|---|
| `COMPACT_TERMINAL_SPILL_DIR` | system temp | Directory for transient spill files |

## Build

```bash
cargo build -p compact-terminal-mcp
./target/debug/compact-terminal-mcp
```

## MCP Config

```json
{
  "servers": {
    "compact-terminal": {
      "type": "stdio",
      "command": "./target/debug/compact-terminal-mcp"
    }
  }
}
```
