use std::{
    fs,
    process::Command,
};

use tempfile::tempdir;
use viewer_api::query::JqFilter;

fn write_file(
    path: &std::path::Path,
    content: &str,
) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dirs");
    }
    fs::write(path, content).expect("write file");
}

#[test]
fn repo_map_generation_writes_root_toon_file() {
    let dir = tempdir().expect("temp dir");
    let root = dir.path();

    write_file(
        &root.join("Cargo.toml"),
        r#"[workspace]
resolver = "2"
members = [
  "memory-api/tools/cli/peek-cli",
  "viewer-api/viewer-api",
]
"#,
    );
    write_file(
        &root.join("memory-api/tools/cli/peek-cli/Cargo.toml"),
        r#"[package]
name = "peek-cli"
version = "0.1.0"
"#,
    );
    write_file(
        &root.join("viewer-api/viewer-api/Cargo.toml"),
        r#"[package]
name = "viewer-api"
version = "0.1.0"
"#,
    );
    write_file(&root.join("AGENTS.md"), "# Agent rules\n");
    write_file(
        &root.join(".agents/instructions/token-efficiency.instructions.md"),
        "token guidance\n",
    );
    write_file(
        &root.join(".agents/prompts/implement.prompt.md"),
        "implement prompt\n",
    );
    write_file(&root.join(".githooks/pre-commit"), "#!/usr/bin/env bash\n");

    let out = Command::new(env!("CARGO_BIN_EXE_peek"))
        .arg(".")
        .arg("--repo-map")
        .arg("--output")
        .arg("repo_map.toon")
        .current_dir(root)
        .output()
        .expect("peek binary should spawn");

    assert!(
        out.status.success(),
        "peek --repo-map failed ({})\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let rendered = fs::read_to_string(root.join("repo_map.toon"))
        .expect("repo_map.toon should be written at the repo root");
    let parsed: serde_json::Value = toon_format::decode_default(&rendered)
        .expect("repo_map.toon should decode as TOON");

    assert_eq!(parsed["format"], "repo_map_toon_v1");
    let workspace_root = parsed["workspace"]["root"]
        .as_str()
        .expect("workspace root should be a string");
    let root_name = root
        .file_name()
        .and_then(|name| name.to_str())
        .expect("tempdir should have a file name");
    assert!(workspace_root.replace('\\', "/").ends_with(root_name));
    assert_eq!(
        parsed["refresh_command"],
        "cargo run -p peek-cli -- . --repo-map --output repo_map.toon"
    );
    assert_eq!(parsed["crates"]["name"], "crates");
    assert!(parsed["top_level_dirs"].is_array());

    let crate_filter =
        JqFilter::compile(r#"select(.note == "crate=peek-cli")"#)
            .expect("jq filter should compile");
    let crate_nodes = flatten_json_nodes(&parsed["crates"]);
    let matches: Vec<_> = crate_nodes
        .iter()
        .filter(|value| crate_filter.matches(value))
        .collect();
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0]["path"], "crates/memory-api/tools/cli/peek-cli");
}

#[test]
fn repo_map_toon_text_supports_jq_queries() {
    let dir = tempdir().expect("temp dir");
    let repo_map = dir.path().join("repo_map.toon");
    let repo_map_value = serde_json::json!({
        "format": "repo_map_toon_v1",
        "crates": {
            "name": "crates",
            "kind": "dir",
            "path": "crates",
            "children": [
                {
                    "name": "tools",
                    "kind": "dir",
                    "path": "crates/memory-api/tools",
                    "children": [
                        {
                            "name": "cli",
                            "kind": "dir",
                            "path": "crates/memory-api/tools/cli",
                            "children": [
                                {
                                    "name": "peek-cli",
                                    "kind": "dir",
                                    "path": "crates/memory-api/tools/cli/peek-cli",
                                    "note": "crate=peek-cli"
                                }
                            ]
                        }
                    ]
                },
                {
                    "name": "memory-viewers",
                    "kind": "dir",
                    "path": "crates/memory-viewers",
                    "children": [
                        {
                            "name": "viewer-api",
                            "kind": "dir",
                            "path": "crates/memory-viewers/viewer-api",
                            "children": [
                                {
                                    "name": "viewer-api",
                                    "kind": "dir",
                                    "path": "crates/viewer-api/viewer-api",
                                    "note": "crate=viewer-api"
                                }
                            ]
                        }
                    ]
                }
            ]
        }
    });
    fs::write(
        &repo_map,
        toon_format::encode_default(&repo_map_value)
            .expect("encode TOON fixture"),
    )
    .expect("write repo_map fixture");

    let parsed: serde_json::Value = toon_format::decode_default(
        &fs::read_to_string(&repo_map).expect("read repo_map fixture"),
    )
    .expect("decode repo_map fixture");
    let values = flatten_json_nodes(&parsed["crates"]);

    let filter = JqFilter::compile(r#"select(.note | contains("crate="))"#)
        .expect("jq filter should compile");
    let matches: Vec<_> = values
        .iter()
        .filter(|value| filter.matches(value))
        .collect();

    assert_eq!(matches.len(), 2);
    let notes: Vec<_> = matches
        .iter()
        .map(|value| value["note"].as_str().expect("note string"))
        .collect();
    assert!(notes.contains(&"crate=peek-cli"));
    assert!(notes.contains(&"crate=viewer-api"));
}

#[test]
fn skeleton_directory_output_stays_queryable() {
    let dir = tempdir().expect("temp dir");
    let root = dir.path().join("workspace");
    write_file(
        &root.join("memory-api/tools/cli/peek-cli/Cargo.toml"),
        "[package]\nname = \"peek-cli\"\n",
    );
    write_file(
        &root.join(".agents/instructions/token-efficiency.instructions.md"),
        "token guidance\n",
    );
    write_file(&root.join("README.md"), "hello\n");

    let out = Command::new(env!("CARGO_BIN_EXE_peek"))
        .arg(&root)
        .arg("--skeleton")
        .output()
        .expect("peek binary should spawn");

    assert!(
        out.status.success(),
        "peek --skeleton on dir failed ({})\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let rendered =
        String::from_utf8(out.stdout).expect("skeleton output should be utf-8");
    assert!(rendered.contains("workspace/"));
    assert!(rendered.contains("  tools/"));
    assert!(rendered.contains("    cli/"));
    assert!(rendered.contains("      peek-cli/"));

    let entries: Vec<serde_json::Value> = rendered
        .lines()
        .enumerate()
        .map(|(index, line)| {
            serde_json::json!({
                "line": index + 1,
                "text": line,
            })
        })
        .collect();

    let filter = JqFilter::compile(r#"select(.text | contains("peek-cli"))"#)
        .expect("jq filter should compile");
    let matches: Vec<_> = entries
        .iter()
        .filter(|value| filter.matches(value))
        .collect();

    assert_eq!(matches.len(), 1);
}

fn flatten_json_nodes(value: &serde_json::Value) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    flatten_json_nodes_into(value, &mut out);
    out
}

fn flatten_json_nodes_into(
    value: &serde_json::Value,
    out: &mut Vec<serde_json::Value>,
) {
    out.push(value.clone());
    if let Some(children) = value.get("children").and_then(|v| v.as_array()) {
        for child in children {
            flatten_json_nodes_into(child, out);
        }
    }
}
