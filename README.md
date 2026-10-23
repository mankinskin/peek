# peek

Token-bounded file inspection: reads targeted line windows from files instead
of pulling whole files into a model's context. Primary use case: locate and
read only the lines an agent or reviewer actually needs (`--count`, `--grep`,
then `--start`/`--end`) instead of reading entire files.

Command specifics and examples are documented in
[crates/peek-cli/README.md](crates/peek-cli/README.md).

## Quickstart

```bash
cargo build -p peek-cli
./target/debug/peek path/to/file.rs --start 42 --end 80
```

## Windows compact-terminal shells

On Windows, compact-terminal continues to execute commands through `sh -c`.
Its child shell puts its own POSIX `/usr/bin` first in `PATH`, so `bash`
resolves from the same shell distribution rather than a Windows launcher that
may appear earlier in the inherited PATH. This is child-only: it does not
change the default shell or global PATH, and does not install or configure WSL.
