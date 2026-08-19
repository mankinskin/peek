# peek — Token-Bounded File Inspection

`peek` is a command-line utility that reads **targeted line windows** from files
instead of pulling whole files into a token context. Bounded reads are the
**default interaction pattern**; full-file reads require an explicit `--all`
flag so the cost is always visible in command history.

## Architecture

`peek` now follows the repository's standard transport layering:

- `memory-api/crates/peek-api` owns the shared inspection and skeletonization behavior
- `memory-api/tools/cli/peek-cli` owns clap parsing and text output
- `memory-api/tools/mcp/peek-mcp` exposes the same core operations as named MCP tools

## Why

AI agents that read entire source files to locate a single function waste 50–90%
of their token budget on irrelevant context. `peek` enforces a "coordinates
first, content second" discipline:

1. Use `--count` to learn the file size.
2. Use `--grep` to find the target line number.
3. Use `--start`/`--end` or `--start`/`--window` to read only the needed slice.
4. Reserve `--all` for the rare case where the full file is genuinely required.

## Installation

```bash
cargo install --path memory-api/tools/cli/peek-cli
```

Or build locally:

```bash
cargo build -p peek-cli
./target/debug/peek --help
```

## Usage

### Targeted window (recommended)

```bash
# Read lines 42–80
peek path/to/file.rs --start 42 --end 80

# Read 20 lines starting at line 100
peek path/to/file.rs --start 100 --window 20
```

### Head / tail

```bash
peek path/to/file.rs --head 30
peek path/to/file.rs --tail 30
```

### Pattern search + context window

```bash
# Find all lines matching "fn my_function" (returns line numbers)
peek path/to/file.rs --grep "fn my_function"

# Find pattern and show 15 lines of context around the first match
peek path/to/file.rs --grep "fn my_function" --window 15
```

### Count lines (plan your bounded read)

```bash
peek path/to/file.rs --count
# → 412
```

### Escape hatch: full file

Only use `--all` when the entire file is genuinely needed:

```bash
peek path/to/file.rs --all
```

The `--all` flag name is intentional — it makes the token cost explicit and
visible in command history and reviews.

## Agent Usage Pattern

```bash
# Step 1: learn size
peek src/lib.rs --count

# Step 2: find the function
peek src/lib.rs --grep "fn process_batch"

# Step 3: read a tight window
peek src/lib.rs --start 187 --window 35

# Step 4: if more context is needed, expand the window
peek src/lib.rs --start 187 --end 240
```

Prefer this four-step flow over `read_file src/lib.rs` (full file). Full-file
reads should become the exception, not the default.

## Output Format

Lines are printed with 1-based line numbers:

```
    42 pub fn process_batch(items: &[Item]) -> Result<Summary> {
    43     let mut acc = Summary::default();
   ...
    80 }
```

### Skeletonize: architecture map (signatures only)

Map the structure of a file without reading implementation bodies:

```bash
# Show Rust function signatures and type definitions, collapse bodies
peek src/lib.rs --skeleton

# Show Python class/function signatures, collapse method bodies
peek scripts/process.py --skeleton
```

Output example:

```
   100 fn main() -> Result<()> {
        // ...
   260 fn print_window(lines: &[String], start: usize, end: usize, total: usize) -> Result<()> {
        // ...
```

Use `--skeleton` to map a file's architecture before deciding which function
bodies to read in detail with `--start`/`--end`.

### Repository map generation

Generate a compact tree-shaped workspace map without the old Python parser:

```bash
peek . --repo-map --output repo_map.toon

# Inspect a directory tree directly
peek .agents --skeleton
```

`--repo-map` compacts shared path prefixes into nested trees and emits the
crate, agent-file, and hook sections consumed by the root-level `repo_map.toon`.

The generated file is TOON-encoded and can be decoded or queried from Rust with
`toon-format` plus JQ-style filters over the decoded JSON structure.

## Flags

| Flag | Short | Description |
|---|---|---|
| `--start N` | `-s` | First line (1-based, inclusive) |
| `--end N` | `-e` | Last line (inclusive); requires `--start` |
| `--window N` | `-w` | Lines to show after `--start` |
| `--head N` | | First N lines |
| `--tail N` | | Last N lines |
| `--grep PATTERN` | `-g` | Find matching lines; combine with `--window` for context |
| `--count` | `-c` | Print total line count only |
| `--skeleton` | `-k` | Architecture map: signatures only, bodies collapsed |
| `--repo-map` | | Generate compact workspace repo map |
| `--output PATH` | | Write generated repo-map output to a file |
| `--all` | | Escape hatch: full file (explicit opt-in) |
