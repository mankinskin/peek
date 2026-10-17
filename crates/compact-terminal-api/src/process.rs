//! Background process registry.
//!
//! Keeps spawned children alive beyond a single `execute()` call so that a stalled
//! process (e.g. one blocked on an interactive input prompt) can be polled again or
//! written to via `send_input`, instead of the caller hanging indefinitely with no
//! visibility into what went wrong.

use std::{
    collections::HashMap,
    io::Read,
    path::PathBuf,
    process::{
        Child,
        ChildStdin,
    },
    sync::{
        Arc,
        Mutex,
        OnceLock,
    },
    time::Instant,
};

/// Shared, continuously-updated view of a process's captured output.
///
/// Reader threads write into this from the moment the process is spawned until it
/// exits (or panics), independent of whether anyone is currently waiting on it.
pub struct OutputState {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub last_activity: Instant,
    /// Diagnostics from reader threads (I/O errors, panics) — surfaced to callers
    /// instead of being silently swallowed.
    pub reader_errors: Vec<String>,
    pub stdout_done: bool,
    pub stderr_done: bool,
}

impl OutputState {
    pub fn new() -> Self {
        Self {
            stdout: Vec::new(),
            stderr: Vec::new(),
            last_activity: Instant::now(),
            reader_errors: Vec::new(),
            stdout_done: false,
            stderr_done: false,
        }
    }

    pub fn readers_done(&self) -> bool {
        self.stdout_done && self.stderr_done
    }

    /// Last `max_bytes` of combined stdout+stderr, for stall/timeout diagnostics.
    pub fn tail(&self, max_bytes: usize) -> String {
        let mut combined =
            Vec::with_capacity(self.stdout.len() + self.stderr.len());
        combined.extend_from_slice(&self.stdout);
        combined.extend_from_slice(&self.stderr);
        let start = combined.len().saturating_sub(max_bytes);
        String::from_utf8_lossy(&combined[start..]).into_owned()
    }
}

pub struct ManagedProcess {
    pub child: Child,
    pub command: String,
    pub started_at: Instant,
    pub timeout_secs: u64,
    pub stall_timeout_secs: u64,
    pub kill_timeout_secs: u64,
    pub inline_limit: usize,
    pub spill_dir: PathBuf,
    /// Present only when the caller opted into `interactive` mode; otherwise stdin
    /// was closed immediately after spawn (EOF), matching non-interactive execution.
    pub stdin: Option<ChildStdin>,
    pub output: Arc<Mutex<OutputState>>,
    #[cfg(windows)]
    pub pid_file: PathBuf,
}

#[derive(Default)]
pub struct ProcessRegistry {
    inner: Mutex<HashMap<String, ManagedProcess>>,
}

static REGISTRY: OnceLock<ProcessRegistry> = OnceLock::new();

pub fn registry() -> &'static ProcessRegistry {
    REGISTRY.get_or_init(ProcessRegistry::default)
}

impl ProcessRegistry {
    pub fn insert(&self, handle_id: String, process: ManagedProcess) {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.insert(handle_id, process);
    }

    pub fn remove(&self, handle_id: &str) -> Option<ManagedProcess> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.remove(handle_id)
    }

    pub fn contains(&self, handle_id: &str) -> bool {
        let guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.contains_key(handle_id)
    }

    pub fn with_mut<T>(
        &self,
        handle_id: &str,
        f: impl FnOnce(&mut ManagedProcess) -> T,
    ) -> Option<T> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.get_mut(handle_id).map(f)
    }
}

/// Spawn a reader thread that drains `reader` into `output`, catching panics so a
/// misbehaving read never silently disappears — the failure is recorded instead.
pub fn spawn_reader(
    mut reader: impl Read + Send + 'static,
    is_stdout: bool,
    output: Arc<Mutex<OutputState>>,
) {
    std::thread::spawn(move || {
        let outcome =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut buffer = [0u8; 8192];
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(n) => {
                            let mut state = output
                                .lock()
                                .unwrap_or_else(|e| e.into_inner());
                            if is_stdout {
                                state.stdout.extend_from_slice(&buffer[..n]);
                            } else {
                                state.stderr.extend_from_slice(&buffer[..n]);
                            }
                            state.last_activity = Instant::now();
                        },
                        Err(_) => break,
                    }
                }
            }));

        let mut state = output.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(panic) = outcome {
            let label = if is_stdout { "stdout" } else { "stderr" };
            state.reader_errors.push(format!(
                "{label} reader thread panicked: {}",
                panic_message(&panic)
            ));
        }
        if is_stdout {
            state.stdout_done = true;
        } else {
            state.stderr_done = true;
        }
    });
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}
