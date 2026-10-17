use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CompactTerminalError {
    #[error("{0}")]
    InvalidRequest(String),

    #[error("cannot spawn command '{command}': {source}")]
    CannotSpawn {
        command: String,
        #[source]
        source: std::io::Error,
    },

    #[error("cannot create spill directory '{path}': {source}")]
    CannotCreateSpillDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("cannot write spill file '{path}': {source}")]
    CannotWriteSpillFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("cannot read spill file '{path}': {source}")]
    CannotReadSpillFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("command timed out after {timeout_secs} seconds")]
    Timeout { timeout_secs: u64 },

    #[error("task error: {0}")]
    TaskError(String),
}
