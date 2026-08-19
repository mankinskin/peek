//! peek — token-bounded file inspection utility
//!
//! Reads a targeted line window from a file rather than pulling the whole file.
//! Bounded reads are the default; full-file reads require an explicit opt-in flag.
//!
//! ## Usage
//!
//! ```text
//! # Read lines 42–80 (bounded, recommended)
//! peek path/to/file.rs --start 42 --end 80
//!
//! # Read 20 lines from line 100 (window shorthand)
//! peek path/to/file.rs --start 100 --window 20
//!
//! # Read the first N lines (head-style)
//! peek path/to/file.rs --head 30
//!
//! # Read the last N lines (tail-style)
//! peek path/to/file.rs --tail 30
//!
//! # Find a pattern and show a window around the match
//! peek path/to/file.rs --grep "fn my_function" --window 15
//!
//! # Count total lines (useful for planning bounded reads)
//! peek path/to/file.rs --count
//!
//! # Escape hatch: full file (add --all to acknowledge the cost)
//! peek path/to/file.rs --all
//! ```

use std::path::PathBuf;

use clap::Parser;
use peek_api::{
    InspectMode,
    InspectRequest,
    PeekError,
    PeekRequest,
    execute,
    write_output,
};

/// peek — token-bounded file inspection.
///
/// Reads a targeted slice of a file. Bounded reads are the default interaction
/// pattern; use `--all` only when the entire file is genuinely needed.
#[derive(Debug, Parser)]
#[command(name = "peek", version, about)]
struct Args {
    /// Path to the file to inspect.
    file: PathBuf,

    /// Generate the repository structural map for the target root.
    ///
    /// This emits a compact tree-shaped workspace map suitable for
    /// root-level `repo_map.toon` regeneration.
    #[arg(long)]
    repo_map: bool,

    /// Write generated tree output to a file instead of stdout.
    ///
    /// Currently supported with `--repo-map`.
    #[arg(long)]
    output: Option<PathBuf>,

    /// First line to include (1-based, inclusive).
    #[arg(long, short = 's')]
    start: Option<usize>,

    /// Last line to include (1-based, inclusive). Requires --start.
    #[arg(long, short = 'e')]
    end: Option<usize>,

    /// Number of lines to show after --start (alternative to --end).
    /// When used without --start, anchors at line 1.
    #[arg(long, short = 'w')]
    window: Option<usize>,

    /// Show the first N lines (like `head -n N`).
    #[arg(long)]
    head: Option<usize>,

    /// Show the last N lines (like `tail -n N`).
    #[arg(long)]
    tail: Option<usize>,

    /// Search for a regex pattern and print `{line_no} | {content}` for every match.
    ///
    /// The pattern is a Rust `regex` expression — alternation (`fn foo|fn bar`),
    /// character classes, anchors, and other regex syntax are supported.
    ///
    /// Combine with --window to show a context window around the first match
    /// instead of listing all matches.
    #[arg(long, short = 'g')]
    grep: Option<String>,

    /// Print total line count only (useful for planning bounded reads before
    /// choosing --start/--end coordinates).
    #[arg(long, short = 'c')]
    count: bool,

    /// Escape hatch: read the entire file.
    ///
    /// Prefer bounded reads whenever possible. Only use this flag when the
    /// complete file content is genuinely required and no targeted slice will
    /// suffice. The flag name is intentional — it makes the token cost visible
    /// in command history.
    #[arg(long)]
    all: bool,

    /// Skeletonize: strip function/method bodies and return only structural
    /// signatures, type definitions, trait bounds, and doc comments.
    ///
    /// Supports Rust and Python files (detected by extension).
    /// Use this to map the architecture of a file before deciding which
    /// function bodies to read in detail.
    #[arg(long, short = 'k')]
    skeleton: bool,
}

fn main() -> Result<(), PeekError> {
    let args = Args::parse();
    let output = args.output.clone();
    let request = build_request(&args)?;
    let response = execute(&request)?;

    if matches!(request, PeekRequest::RepoMap { .. }) && output.is_some() {
        write_output(&response.stdout, output.as_deref())?;
    } else if !response.stdout.is_empty() {
        print!("{}", response.stdout);
    }

    if !response.stderr.is_empty() {
        eprint!("{}", response.stderr);
    }

    Ok(())
}

fn build_request(args: &Args) -> Result<PeekRequest, PeekError> {
    if args.repo_map {
        validate_repo_map(args)?;
        return Ok(PeekRequest::RepoMap {
            root: args.file.clone(),
        });
    }

    if args.skeleton {
        return Ok(PeekRequest::Skeleton {
            path: args.file.clone(),
        });
    }

    validate_inspection(args)?;

    if args.count {
        return Ok(PeekRequest::Inspect(InspectRequest {
            path: args.file.clone(),
            mode: InspectMode::Count,
        }));
    }

    if let Some(ref pattern) = args.grep {
        return Ok(PeekRequest::Inspect(InspectRequest {
            path: args.file.clone(),
            mode: InspectMode::Grep {
                pattern: pattern.clone(),
                window: args.window,
            },
        }));
    }

    if args.all {
        return Ok(PeekRequest::Inspect(InspectRequest {
            path: args.file.clone(),
            mode: InspectMode::All,
        }));
    }

    if let Some(n) = args.head {
        return Ok(PeekRequest::Inspect(InspectRequest {
            path: args.file.clone(),
            mode: InspectMode::Head { lines: n },
        }));
    }

    if let Some(n) = args.tail {
        return Ok(PeekRequest::Inspect(InspectRequest {
            path: args.file.clone(),
            mode: InspectMode::Tail { lines: n },
        }));
    }

    if let Some(start) = args.start {
        return Ok(PeekRequest::Inspect(InspectRequest {
            path: args.file.clone(),
            mode: InspectMode::Range {
                start,
                end: args.end,
                window: args.window,
            },
        }));
    }

    Err(PeekError::InvalidRequest(format!(
        "peek requires a bounded read mode.\n\
         \n\
         Common patterns:\n\
         \n\
         # Targeted window (recommended)\n\
         peek {path} --start 42 --end 80\n\
         \n\
         # Window from start\n\
         peek {path} --start 100 --window 20\n\
         \n\
         # Head / tail\n\
         peek {path} --head 30\n\
         peek {path} --tail 30\n\
         \n\
         # Find a function, then show context\n\
         peek {path} --grep \"fn my_fn\" --window 15\n\
         \n\
         # Count lines before choosing coordinates\n\
         peek {path} --count\n\
         \n\
         # Architecture map (signatures only, no bodies)\n\
         peek {path} --skeleton\n\
         \n\
         # Full file (token-expensive — explicit opt-in)\n\
         peek {path} --all",
        path = args.file.display(),
    )))
}

fn validate_repo_map(args: &Args) -> Result<(), PeekError> {
    if args.skeleton
        || args.start.is_some()
        || args.end.is_some()
        || args.window.is_some()
        || args.head.is_some()
        || args.tail.is_some()
        || args.grep.is_some()
        || args.count
        || args.all
    {
        return Err(PeekError::InvalidRequest(
            "--repo-map cannot be combined with file inspection flags"
                .to_string(),
        ));
    }
    Ok(())
}

fn has_bounded_inspection_flags(args: &Args) -> bool {
    args.start.is_some()
        || args.end.is_some()
        || args.window.is_some()
        || args.head.is_some()
        || args.tail.is_some()
        || args.grep.is_some()
        || args.count
}

fn validate_inspection(args: &Args) -> Result<(), PeekError> {
    if args.all && has_bounded_inspection_flags(args) {
        return Err(PeekError::InvalidRequest(
            "--all cannot be combined with bounded inspection flags"
                .to_string(),
        ));
    }

    if args.end.is_some() && args.start.is_none() {
        return Err(PeekError::InvalidRequest(
            "--end requires --start".to_string(),
        ));
    }

    if args.end.is_some() && args.window.is_some() {
        return Err(PeekError::InvalidRequest(
            "--end and --window are mutually exclusive".to_string(),
        ));
    }

    Ok(())
}
