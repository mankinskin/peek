use std::{
    collections::BTreeMap,
    fs::{
        self,
        File,
    },
    io::{
        BufRead,
        BufReader,
    },
    path::{
        Path,
        PathBuf,
    },
};

use regex::Regex;
use serde::{
    Deserialize,
    Serialize,
};
use serde_json::{
    Value,
    json,
};
use thiserror::Error;

pub const DEFAULT_WINDOW: usize = 40;
pub const REPO_MAP_FILE: &str = "repo_map.toon";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeekResponse {
    pub stdout: String,
    pub stderr: String,
}

impl PeekResponse {
    fn stdout(stdout: String) -> Self {
        Self {
            stdout,
            stderr: String::new(),
        }
    }

    fn stderr(stderr: String) -> Self {
        Self {
            stdout: String::new(),
            stderr,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectRequest {
    pub path: PathBuf,
    pub mode: InspectMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum InspectMode {
    Count,
    Grep {
        pattern: String,
        window: Option<usize>,
    },
    All,
    Head {
        lines: usize,
    },
    Tail {
        lines: usize,
    },
    Range {
        start: usize,
        end: Option<usize>,
        window: Option<usize>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PeekRequest {
    Inspect(InspectRequest),
    Skeleton { path: PathBuf },
    RepoMap { root: PathBuf },
}

#[derive(Debug, Error)]
pub enum PeekError {
    #[error("{0}")]
    InvalidRequest(String),
    #[error("cannot open '{path}': {source}")]
    CannotOpen {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("error reading '{path}': {source}")]
    CannotRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot resolve '{path}': {source}")]
    CannotResolve {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot create '{path}': {source}")]
    CannotCreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot write '{path}': {source}")]
    CannotWrite {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid regex pattern: {pattern:?}: {source}")]
    InvalidRegex {
        pattern: String,
        #[source]
        source: regex::Error,
    },
    #[error("bounded file inspection requires a file path: '{path}'")]
    UnsupportedFileTarget { path: PathBuf },
    #[error("--start is 1-based; use --start 1 for the first line")]
    StartMustBePositive,
    #[error("--start {start} exceeds file length ({total} lines)")]
    StartExceedsFileLength { start: usize, total: usize },
    #[error("--end ({end}) must be >= --start ({start})")]
    EndBeforeStart { start: usize, end: usize },
    #[error("failed to encode repo map as TOON: {0}")]
    RepoMapEncode(String),
}

pub fn execute(request: &PeekRequest) -> Result<PeekResponse, PeekError> {
    match request {
        PeekRequest::Inspect(request) => inspect(request),
        PeekRequest::Skeleton { path } => skeletonize_target(path),
        PeekRequest::RepoMap { root } => {
            let map = generate_repo_map(root)?;
            Ok(PeekResponse::stdout(map))
        },
    }
}

pub fn write_output(
    text: &str,
    output: Option<&Path>,
) -> Result<(), PeekError> {
    if let Some(path) = output {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| {
                PeekError::CannotCreateDir {
                    path: parent.to_path_buf(),
                    source,
                }
            })?;
        }
        fs::write(path, text).map_err(|source| PeekError::CannotWrite {
            path: path.to_path_buf(),
            source,
        })?;
    }
    Ok(())
}

fn inspect(request: &InspectRequest) -> Result<PeekResponse, PeekError> {
    if request.path.is_dir() {
        return Err(PeekError::UnsupportedFileTarget {
            path: request.path.clone(),
        });
    }

    let lines = read_lines(&request.path)?;
    let total = lines.len();

    match &request.mode {
        InspectMode::Count => Ok(PeekResponse::stdout(format!("{total}\n"))),
        InspectMode::Grep { pattern, window } =>
            grep_lines(&request.path, &lines, total, pattern, *window),
        InspectMode::All => Ok(PeekResponse::stdout(render_all(&lines))),
        InspectMode::Head { lines: count } => {
            let end = (*count).min(total);
            Ok(PeekResponse::stdout(render_window(&lines, 1, end, total)))
        },
        InspectMode::Tail { lines: count } => {
            let start = total.saturating_sub(*count) + 1;
            Ok(PeekResponse::stdout(render_window(
                &lines, start, total, total,
            )))
        },
        InspectMode::Range { start, end, window } => {
            let rendered =
                render_requested_range(&lines, total, *start, *end, *window)?;
            Ok(PeekResponse::stdout(rendered))
        },
    }
}

fn grep_lines(
    path: &Path,
    lines: &[String],
    total: usize,
    pattern: &str,
    window: Option<usize>,
) -> Result<PeekResponse, PeekError> {
    let regex =
        Regex::new(pattern).map_err(|source| PeekError::InvalidRegex {
            pattern: pattern.to_string(),
            source,
        })?;

    let matches: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| regex.is_match(line))
        .map(|(index, _)| index + 1)
        .collect();

    if matches.is_empty() {
        return Ok(PeekResponse::stderr(format!(
            "peek: no match for {:?} in {}\n",
            pattern,
            path.display()
        )));
    }

    if let Some(width) = window {
        let first = matches[0];
        let start = first.saturating_sub(width / 2).max(1);
        let end = (first + width / 2).min(total);
        return Ok(PeekResponse::stdout(render_window(
            lines, start, end, total,
        )));
    }

    let mut stdout = String::new();
    for line_no in matches {
        stdout.push_str(&format!("{:>6} | {}\n", line_no, lines[line_no - 1]));
    }
    Ok(PeekResponse::stdout(stdout))
}

fn render_requested_range(
    lines: &[String],
    total: usize,
    start: usize,
    end: Option<usize>,
    window: Option<usize>,
) -> Result<String, PeekError> {
    if start == 0 {
        return Err(PeekError::StartMustBePositive);
    }
    if start > total {
        return Err(PeekError::StartExceedsFileLength { start, total });
    }

    let end = if let Some(end) = end {
        if end < start {
            return Err(PeekError::EndBeforeStart { start, end });
        }
        end.min(total)
    } else if let Some(window) = window {
        (start + window.saturating_sub(1)).min(total)
    } else {
        (start + DEFAULT_WINDOW - 1).min(total)
    };

    Ok(render_window(lines, start, end, total))
}

fn skeletonize_target(path: &Path) -> Result<PeekResponse, PeekError> {
    if path.is_dir() {
        return Ok(PeekResponse::stdout(skeletonize_directory(path)?));
    }

    let lines = read_lines(path)?;
    let text = skeletonize(path, &lines)?;
    Ok(PeekResponse::stdout(text))
}

fn read_lines(path: &Path) -> Result<Vec<String>, PeekError> {
    let file = File::open(path).map_err(|source| PeekError::CannotOpen {
        path: path.to_path_buf(),
        source,
    })?;
    let reader = BufReader::new(file);
    reader
        .lines()
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(|source| PeekError::CannotRead {
            path: path.to_path_buf(),
            source,
        })
}

fn render_all(lines: &[String]) -> String {
    let mut stdout = String::new();
    for (index, line) in lines.iter().enumerate() {
        stdout.push_str(&format!("{:>6} {line}\n", index + 1));
    }
    stdout
}

fn render_window(
    lines: &[String],
    start: usize,
    end: usize,
    total: usize,
) -> String {
    let start = start.max(1);
    let end = end.min(total);
    let mut stdout = String::new();
    for line_no in start..=end {
        stdout.push_str(&format!("{:>6} {}\n", line_no, lines[line_no - 1]));
    }
    stdout
}

fn skeletonize(
    path: &Path,
    lines: &[String],
) -> Result<String, PeekError> {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_lowercase();

    match ext.as_str() {
        "rs" => Ok(skeletonize_rust(lines)),
        "py" => Ok(skeletonize_python(lines)),
        _ => Ok(skeletonize_generic(lines)),
    }
}

fn is_structural_rust_line(trimmed: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "///",
        "//!",
        "#[",
        "#!",
        "use ",
        "pub use ",
        "mod ",
        "pub mod ",
        "pub struct",
        "struct ",
        "pub enum",
        "enum ",
        "pub type",
        "type ",
        "pub trait",
        "trait ",
        "pub const",
        "const ",
        "pub static",
        "static ",
        "impl ",
        "pub impl",
        "pub fn",
        "fn ",
        "async fn",
        "pub async fn",
        "extern ",
    ];
    trimmed.is_empty()
        || PREFIXES.iter().any(|prefix| trimmed.starts_with(prefix))
}

fn skeletonize_rust(lines: &[String]) -> String {
    let mut depth: i64 = 0;
    let mut collapse_start_depth: Option<i64> = None;
    let mut stdout = String::new();

    for (index, line) in lines.iter().enumerate() {
        let line_no = index + 1;
        let trimmed = line.trim();
        let is_structural = is_structural_rust_line(trimmed);

        let open = line.chars().filter(|&ch| ch == '{').count() as i64;
        let close = line.chars().filter(|&ch| ch == '}').count() as i64;

        if let Some(collapse_depth) = collapse_start_depth {
            depth += open - close;
            if depth <= collapse_depth {
                collapse_start_depth = None;
            }
            continue;
        }

        if is_structural || depth == 0 {
            stdout.push_str(&format!("{line_no:>6} {line}\n"));
            if open > close && trimmed.ends_with('{') {
                collapse_start_depth = Some(depth + open - close - 1);
                depth += open - close;
                stdout.push_str("       // ...\n");
            } else {
                depth += open - close;
            }
        } else {
            depth += open - close;
        }
    }

    stdout
}

fn skeletonize_python(lines: &[String]) -> String {
    let mut collapse_indent: Option<usize> = None;
    let mut stdout = String::new();

    for (index, line) in lines.iter().enumerate() {
        let line_no = index + 1;
        let indent = line.len() - line.trim_start().len();
        let trimmed = line.trim();

        if trimmed.is_empty() {
            stdout.push_str(&format!("{line_no:>6} {line}\n"));
            continue;
        }

        if let Some(collapse_indent) = collapse_indent {
            if indent > collapse_indent {
                continue;
            }
        }
        collapse_indent = None;

        let starts_body = trimmed.starts_with("def ")
            || trimmed.starts_with("async def ")
            || trimmed.starts_with("class ");

        stdout.push_str(&format!("{line_no:>6} {line}\n"));
        if starts_body && trimmed.ends_with(':') {
            collapse_indent = Some(indent);
            stdout.push_str("       # ...\n");
        }
    }

    stdout
}

fn skeletonize_generic(lines: &[String]) -> String {
    let mut in_body = false;
    let mut stdout = String::new();
    for (index, line) in lines.iter().enumerate() {
        let line_no = index + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            in_body = false;
            stdout.push_str(&format!("{line_no:>6} {line}\n"));
        } else if line.starts_with(' ') || line.starts_with('\t') {
            if !in_body {
                stdout.push_str("       // ...\n");
                in_body = true;
            }
        } else {
            in_body = false;
            stdout.push_str(&format!("{line_no:>6} {line}\n"));
        }
    }
    stdout
}

#[derive(Debug, Default)]
struct TreeNode {
    children: BTreeMap<String, TreeNode>,
    note: Option<String>,
    is_file: bool,
}

pub fn generate_repo_map(root: &Path) -> Result<String, PeekError> {
    let root =
        root.canonicalize()
            .map_err(|source| PeekError::CannotResolve {
                path: root.to_path_buf(),
                source,
            })?;
    let cargo_toml = root.join("Cargo.toml");
    let cargo_text = fs::read_to_string(&cargo_toml).map_err(|source| {
        PeekError::CannotRead {
            path: cargo_toml.clone(),
            source,
        }
    })?;

    let members = parse_workspace_members(&cargo_text);
    let mut crate_tree = TreeNode::default();
    for member in members {
        let crate_name = read_crate_name(&root.join(&member))?;
        insert_tree_path(
            &mut crate_tree,
            &member,
            crate_name.map(|name| format!("crate={name}")),
            false,
        );
    }

    let top_dirs = collect_top_level_dirs(&root)?;
    let agent_guidance = collect_existing_paths(
        &root,
        &[
            "AGENTS.md",
            ".agents/instructions/token-efficiency.instructions.md",
        ],
    );
    let agent_files = collect_agent_files(&root)?;
    let hooks = collect_hook_files(&root)?;

    let repo_map = json!({
        "format": "repo_map_toon_v1",
        "description": "Compact workspace structural map for low-token orientation",
        "usage": "Read this before opening source files for structural orientation",
        "refresh_command": format!(
            "cargo run -p peek-cli -- . --repo-map --output {REPO_MAP_FILE}"
        ),
        "workspace": {
            "root": display_path(&root),
        },
        "top_level_dirs": top_dirs,
        "crates": tree_to_value("crates", &crate_tree, "crates"),
        "agent_guidance": tree_to_value(
            "agent-guidance",
            &tree_from_paths(&agent_guidance),
            "agent-guidance",
        ),
        "agent_files": tree_to_value(
            "agent-files",
            &tree_from_paths(&agent_files),
            "agent-files",
        ),
        "hooks": tree_to_value("hooks", &tree_from_paths(&hooks), "hooks"),
        "key_tools": [
            {
                "path": "target/debug/ticket.exe",
                "description": "ticket (state machine, board, deps)",
            },
            {
                "path": "target/debug/spec.exe",
                "description": "spec-cli",
            },
            {
                "path": "target/debug/peek",
                "description": "bounded inspection + repo-map generation",
            },
            {
                "path": "rtk <cmd>",
                "description": "token-optimized CLI proxy (auto-compress output)",
            }
        ],
        "bounded_inspection_pattern": [
            "peek <file> --count  # 1. learn size",
            "peek <file> --grep <pattern>  # 2. locate target line",
            "peek <file> --start N --end M  # 3. bounded read",
            format!(
                "peek . --repo-map --output {REPO_MAP_FILE}  # refresh structural map"
            ),
        ],
    });

    toon_format::encode_default(&repo_map)
        .map_err(|error| PeekError::RepoMapEncode(error.to_string()))
}

fn skeletonize_directory(root: &Path) -> Result<String, PeekError> {
    let root =
        root.canonicalize()
            .map_err(|source| PeekError::CannotResolve {
                path: root.to_path_buf(),
                source,
            })?;
    let mut tree = TreeNode::default();
    collect_directory_tree(&root, &root, &mut tree)?;

    let root_name = root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.display().to_string());

    let mut lines = vec![format!("{root_name}/")];
    lines.extend(render_tree_lines(&tree, 1));
    Ok(lines.join("\n") + "\n")
}

fn collect_directory_tree(
    root: &Path,
    current: &Path,
    tree: &mut TreeNode,
) -> Result<(), PeekError> {
    let mut entries = read_dir_sorted(current)?;

    for entry in entries.drain(..) {
        let path = entry.path();
        let metadata =
            entry.metadata().map_err(|source| PeekError::CannotRead {
                path: path.clone(),
                source,
            })?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let relative =
            path.strip_prefix(root)
                .map_err(|_| PeekError::CannotResolve {
                    path: path.clone(),
                    source: std::io::Error::other("cannot relativize path"),
                })?;

        if should_skip_directory_entry(relative, &name, metadata.is_dir()) {
            continue;
        }

        if metadata.is_dir() {
            insert_tree_path(tree, &to_slash_path(relative), None, false);
            collect_directory_tree(root, &path, tree)?;
        } else if should_include_structural_file(relative) {
            insert_tree_path(tree, &to_slash_path(relative), None, true);
        }
    }

    Ok(())
}

fn read_dir_sorted(path: &Path) -> Result<Vec<fs::DirEntry>, PeekError> {
    let mut entries = fs::read_dir(path)
        .map_err(|source| PeekError::CannotRead {
            path: path.to_path_buf(),
            source,
        })?
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(|source| PeekError::CannotRead {
            path: path.to_path_buf(),
            source,
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    Ok(entries)
}

fn should_skip_directory_entry(
    relative: &Path,
    name: &str,
    is_dir: bool,
) -> bool {
    let noisy_names = [
        "target",
        "node_modules",
        "dist",
        "build",
        ".git",
        ".jj",
        ".idea",
        ".vscode",
        "__pycache__",
    ];
    if noisy_names.contains(&name) {
        return true;
    }

    if name.starts_with('.')
        && !matches!(name, ".agents" | ".githooks" | ".github" | ".rule")
    {
        return true;
    }

    if !is_dir {
        let path = to_slash_path(relative);
        return path.ends_with(".toon") && path != REPO_MAP_FILE;
    }

    false
}

fn should_include_structural_file(relative: &Path) -> bool {
    let path = to_slash_path(relative);
    let Some(name) = relative.file_name().and_then(|name| name.to_str()) else {
        return false;
    };

    matches!(
        name,
        "Cargo.toml"
            | "README.md"
            | "HIGH_LEVEL_GUIDE.md"
            | "AGENTS.md"
            | "Makefile.toml"
            | "rust-toolchain.toml"
            | "rustfmt.toml"
            | "viewer-ctl.toml"
            | "repo_map.toon"
    ) || path.ends_with(".instructions.md")
        || path.ends_with(".prompt.md")
        || path.ends_with(".SKILL.md")
        || path.ends_with(".agent.md")
        || path.ends_with(".sh")
        || path.ends_with(".yaml")
        || path.ends_with(".yml")
        || path.ends_with("hooks.json")
}

fn collect_top_level_dirs(root: &Path) -> Result<Vec<String>, PeekError> {
    let mut dirs = Vec::new();
    let entries = read_dir_sorted(root)?;

    for entry in entries {
        let path = entry.path();
        let metadata =
            entry.metadata().map_err(|source| PeekError::CannotRead {
                path: path.clone(),
                source,
            })?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if metadata.is_dir() && !name.starts_with('.') && name != "target" {
            dirs.push(name);
        }
    }

    Ok(dirs)
}

fn collect_existing_paths(
    root: &Path,
    paths: &[&str],
) -> Vec<String> {
    paths
        .iter()
        .filter_map(|path| {
            root.join(path).exists().then(|| path.replace('\\', "/"))
        })
        .collect()
}

fn collect_agent_files(root: &Path) -> Result<Vec<String>, PeekError> {
    let mut files = Vec::new();
    for subdir in ["instructions", "prompts", "skills"] {
        let dir = root.join(".agents").join(subdir);
        if !dir.exists() {
            continue;
        }

        for entry in read_dir_sorted(&dir)? {
            let path = entry.path();
            if entry
                .metadata()
                .map_err(|source| PeekError::CannotRead {
                    path: path.clone(),
                    source,
                })?
                .is_file()
            {
                let relative = path.strip_prefix(root).map_err(|_| {
                    PeekError::CannotResolve {
                        path: path.clone(),
                        source: std::io::Error::other("cannot relativize path"),
                    }
                })?;
                files.push(to_slash_path(relative));
            }
        }
    }
    Ok(files)
}

fn collect_hook_files(root: &Path) -> Result<Vec<String>, PeekError> {
    let mut files = Vec::new();
    for dir in [root.join(".githooks"), root.join(".github").join("hooks")] {
        if !dir.exists() {
            continue;
        }

        for entry in read_dir_sorted(&dir)? {
            let path = entry.path();
            if entry
                .metadata()
                .map_err(|source| PeekError::CannotRead {
                    path: path.clone(),
                    source,
                })?
                .is_file()
            {
                let relative = path.strip_prefix(root).map_err(|_| {
                    PeekError::CannotResolve {
                        path: path.clone(),
                        source: std::io::Error::other("cannot relativize path"),
                    }
                })?;
                files.push(to_slash_path(relative));
            }
        }
    }
    Ok(files)
}

fn parse_workspace_members(cargo_toml: &str) -> Vec<String> {
    let mut members = Vec::new();
    let mut inside_members = false;

    for line in cargo_toml.lines() {
        let trimmed = line.trim();
        if !inside_members {
            if let Some(index) = trimmed.find("members") {
                let remainder = &trimmed[index..];
                if remainder.starts_with("members") && remainder.contains('[') {
                    inside_members = true;
                    members.extend(extract_quoted_strings(remainder));
                    if remainder.contains(']') {
                        inside_members = false;
                    }
                }
            }
        } else {
            members.extend(extract_quoted_strings(trimmed));
            if trimmed.contains(']') {
                inside_members = false;
            }
        }
    }

    members
}

fn extract_quoted_strings(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch == '"' {
            let mut value = String::new();
            for next in chars.by_ref() {
                if next == '"' {
                    break;
                }
                value.push(next);
            }
            if !value.is_empty() {
                out.push(value);
            }
        }
    }
    out
}

fn read_crate_name(member_dir: &Path) -> Result<Option<String>, PeekError> {
    let cargo_toml = member_dir.join("Cargo.toml");
    if !cargo_toml.exists() {
        return Ok(None);
    }

    let text = fs::read_to_string(&cargo_toml).map_err(|source| {
        PeekError::CannotRead {
            path: cargo_toml.clone(),
            source,
        }
    })?;
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("name = ") {
            let value = rest.trim().trim_matches('"');
            if !value.is_empty() {
                return Ok(Some(value.to_string()));
            }
        }
    }
    Ok(None)
}

fn insert_tree_path(
    tree: &mut TreeNode,
    path: &str,
    note: Option<String>,
    is_file: bool,
) {
    let mut current = tree;
    let mut segments = path.split('/').peekable();
    while let Some(segment) = segments.next() {
        current = current.children.entry(segment.to_string()).or_default();
        if segments.peek().is_none() {
            current.note = note.clone();
            current.is_file = is_file;
        }
    }
}

fn tree_from_paths(paths: &[String]) -> TreeNode {
    let mut tree = TreeNode::default();
    for path in paths {
        insert_tree_path(&mut tree, path, None, true);
    }
    tree
}

fn render_tree_lines(
    tree: &TreeNode,
    indent: usize,
) -> Vec<String> {
    let mut lines = Vec::new();
    for (name, child) in &tree.children {
        let mut line = format!("{}{}", "  ".repeat(indent), name);
        if !child.is_file {
            line.push('/');
        }
        if let Some(note) = &child.note {
            line.push_str(&format!("  {note}"));
        }
        lines.push(line);
        lines.extend(render_tree_lines(child, indent + 1));
    }
    lines
}

fn tree_to_value(
    name: &str,
    tree: &TreeNode,
    path: &str,
) -> Value {
    let children: Vec<Value> = tree
        .children
        .iter()
        .map(|(child_name, child)| {
            let child_path = if path.is_empty() {
                child_name.to_string()
            } else {
                format!("{path}/{child_name}")
            };
            tree_to_value(child_name, child, &child_path)
        })
        .collect();

    let kind = if tree.is_file { "file" } else { "dir" };
    let mut value = json!({
        "name": name,
        "kind": kind,
        "path": path,
    });

    if let Some(note) = &tree.note {
        value["note"] = Value::String(note.clone());
    }
    if !children.is_empty() {
        value["children"] = Value::Array(children);
    }
    value
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn to_slash_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write_file(
        path: &Path,
        content: &str,
    ) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent dirs");
        }
        fs::write(path, content).expect("write file");
    }

    #[test]
    fn inspect_count_returns_total_lines() {
        let dir = tempdir().expect("temp dir");
        let file = dir.path().join("sample.rs");
        write_file(&file, "one\ntwo\nthree\n");

        let response = execute(&PeekRequest::Inspect(InspectRequest {
            path: file,
            mode: InspectMode::Count,
        }))
        .expect("count should succeed");

        assert_eq!(response.stdout, "3\n");
        assert!(response.stderr.is_empty());
    }

    #[test]
    fn inspect_range_validates_bounds() {
        let dir = tempdir().expect("temp dir");
        let file = dir.path().join("sample.rs");
        write_file(&file, "one\ntwo\nthree\n");

        let error = execute(&PeekRequest::Inspect(InspectRequest {
            path: file,
            mode: InspectMode::Range {
                start: 0,
                end: None,
                window: None,
            },
        }))
        .expect_err("start=0 should fail");

        assert!(matches!(error, PeekError::StartMustBePositive));
    }

    #[test]
    fn skeleton_directory_renders_tree() {
        let dir = tempdir().expect("temp dir");
        let root = dir.path().join("workspace");
        write_file(
            &root.join("memory-api/tools/cli/peek-cli/Cargo.toml"),
            "[package]\nname = \"peek-cli\"\n",
        );
        write_file(&root.join("README.md"), "hello\n");

        let response = execute(&PeekRequest::Skeleton { path: root.clone() })
            .expect("directory skeleton should succeed");

        assert!(response.stdout.contains("workspace/"));
        assert!(response.stdout.contains("  memory-api/"));
        assert!(response.stdout.contains("    tools/"));
        assert!(response.stdout.contains("      cli/"));
        assert!(response.stdout.contains("        peek-cli/"));
    }
}
