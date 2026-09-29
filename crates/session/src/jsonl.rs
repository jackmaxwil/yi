use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::error::Category;
use yi_types::wire::{HeaderKind, JsonlV4Header, Mutation};

use crate::error::SessionError;
use crate::id::{IdGenerator, now_ms, validate_session_id};
use crate::query::{CreateOptions, ForkScope, SessionMetadata};
use crate::repo::{SessionRepo, SharedSession, lock_session};
use crate::store::SessionStore;

fn storage_error(context: &str, path: &Path, error: impl std::fmt::Display) -> SessionError {
    SessionError::Storage(format!("{context} {}: {error}", path.display()))
}

fn invalid_file(path: &Path, line: usize, message: impl std::fmt::Display) -> SessionError {
    SessionError::InvalidEntry(format!(
        "Invalid JSONL v4 session {}: line {line} {message}",
        path.display()
    ))
}

pub fn session_directory_name(cwd: &str) -> String {
    let trimmed = cwd.strip_prefix(['/', '\\']).unwrap_or(cwd);
    let encoded: String = trimmed
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':') {
                '-'
            } else {
                c
            }
        })
        .collect();
    format!("--{encoded}--")
}

fn session_file_name(created_at: u64, id: &str) -> String {
    format!("{created_at}_{id}.jsonl")
}

pub(crate) fn metadata_from_header(header: &JsonlV4Header) -> SessionMetadata {
    SessionMetadata {
        id: header.id.clone(),
        created_at: header.created_at,
        parent_session_id: header.parent_session_id.clone(),
        name: None,
    }
}

fn encode_header(header: &JsonlV4Header) -> Result<String, SessionError> {
    serde_json::to_string(header)
        .map(|json| format!("{json}\n"))
        .map_err(|error| SessionError::Storage(format!("Failed to encode session header: {error}")))
}

fn parse_header(line: &str, path: &Path) -> Result<JsonlV4Header, SessionError> {
    let header: JsonlV4Header = serde_json::from_str(line)
        .map_err(|error| invalid_file(path, 1, format!("is not a valid header: {error}")))?;
    if header.kind != HeaderKind::Header || header.version != 4 {
        return Err(invalid_file(
            path,
            1,
            format!("has unsupported version {}", header.version),
        ));
    }
    Ok(header)
}

pub fn load_session(path: &Path) -> Result<SessionStore, SessionError> {
    let content = fs::read_to_string(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            SessionError::NotFound(format!("Session not found: {}", path.display()))
        } else {
            storage_error("Failed to read session", path, error)
        }
    })?;
    let mut lines: Vec<&str> = content.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    let Some(first) = lines.first() else {
        return Err(invalid_file(path, 1, "is missing a header"));
    };
    let header = parse_header(first, path)?;
    let mut store = SessionStore::file_backed(metadata_from_header(&header), path.to_path_buf());
    let parsed = parse_lines(lines.get(1..).unwrap_or_default());
    for (index, parsed) in parsed.into_iter().enumerate() {
        let index = index + 1;
        match parsed {
            Ok(mutation) => {
                store
                    .replay(mutation)
                    .map_err(|error| invalid_file(path, index + 1, error))?;
            }
            Err(error) => {
                let torn_tail = index == lines.len() - 1
                    && matches!(error.classify(), Category::Syntax | Category::Eof);
                if torn_tail {
                    let valid_prefix = format!("{}\n", lines[..index].join("\n"));
                    publish_atomically(path, valid_prefix.as_bytes())?;
                    return Ok(store);
                }
                return Err(invalid_file(path, index + 1, error));
            }
        }
    }
    if !content.ends_with('\n') && !content.is_empty() {
        append_bytes(path, b"\n")?;
    }
    Ok(store)
}

fn parse_lines(lines: &[&str]) -> Vec<serde_json::Result<Mutation>> {
    let parse = |chunk: &[&str]| -> Vec<serde_json::Result<Mutation>> {
        chunk
            .iter()
            .map(|line| serde_json::from_str(line))
            .collect()
    };
    let cores = std::thread::available_parallelism().map_or(1, usize::from);
    let per_core = lines.len().div_ceil(cores).max(128);
    if lines.len() <= per_core {
        return parse(lines);
    }
    std::thread::scope(|scope| {
        let chunks: Vec<_> = lines
            .chunks(per_core)
            .map(|chunk| (chunk, scope.spawn(move || parse(chunk))))
            .collect();
        chunks
            .into_iter()
            .flat_map(|(chunk, parsing)| parsing.join().unwrap_or_else(|_| parse(chunk)))
            .collect()
    })
}

fn append_bytes(path: &Path, bytes: &[u8]) -> Result<(), SessionError> {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(path)
        .map_err(|error| storage_error("Failed to append session", path, error))?;
    file.write_all(bytes)
        .map_err(|error| storage_error("Failed to append session", path, error))
}

fn publish_atomically(path: &Path, bytes: &[u8]) -> Result<(), SessionError> {
    replace_file(path, bytes)
        .map_err(|error| storage_error("Failed to publish staged file", path, error))
}

/// Incident: a pid-only temp name was shared by every writer in the process, so two saves at
/// once wrote one temp file and the loser's rename failed.
pub fn replace_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let temp = path.with_file_name(format!(".{name}.{}.tmp", crate::id::nonce()));
    let staged = fs::File::create(&temp)
        .and_then(|mut file| file.write_all(bytes).and_then(|()| file.sync_data()))
        .and_then(|()| fs::rename(&temp, path));
    if staged.is_err() {
        let _staged_file_is_litter_only = fs::remove_file(&temp);
    }
    staged?;
    match path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        Some(dir) => fs::File::open(dir)?.sync_all(),
        None => Ok(()),
    }
}

pub struct JsonlRepo {
    root: PathBuf,
    cwd: String,
    /// A shared root files sessions under an encoded cwd; a directory that
    /// holds one session does not, so its file is where the caller looks.
    nested: bool,
    ids: IdGenerator,
}

/// Creates the one session a private directory holds, skipping the per-cwd segment
/// [`JsonlRepo::new`] adds: there it names nothing and hides the file (D78).
pub fn create_flat_session(
    dir: PathBuf,
    cwd: impl Into<String>,
    parent_session_id: Option<String>,
) -> Result<SharedSession, SessionError> {
    JsonlRepo {
        root: dir,
        cwd: cwd.into(),
        nested: false,
        ids: IdGenerator::new(),
    }
    .create(CreateOptions {
        parent_session_id,
        ..CreateOptions::default()
    })
}

impl JsonlRepo {
    pub fn new(root: PathBuf, cwd: impl Into<String>) -> Self {
        Self {
            root,
            cwd: cwd.into(),
            nested: true,
            ids: IdGenerator::new(),
        }
    }

    fn session_dir(&self) -> PathBuf {
        if self.nested {
            self.root.join(session_directory_name(&self.cwd))
        } else {
            self.root.clone()
        }
    }

    fn find_session_file(&self, id: &str) -> Result<Option<PathBuf>, SessionError> {
        let dir = self.session_dir();
        let suffix = format!("_{id}.jsonl");
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(storage_error(
                    "Failed to list sessions directory",
                    &dir,
                    error,
                ));
            }
        };
        for entry in entries {
            let entry = entry
                .map_err(|error| storage_error("Failed to list sessions directory", &dir, error))?;
            let name = entry.file_name();
            if name.to_string_lossy().ends_with(&suffix) {
                return Ok(Some(entry.path()));
            }
        }
        Ok(None)
    }

    fn prepare_create(
        &mut self,
        options: &CreateOptions,
    ) -> Result<(JsonlV4Header, PathBuf), SessionError> {
        let id = options.id.clone().unwrap_or_else(|| self.ids.next_id());
        validate_session_id(&id)?;
        if self.find_session_file(&id)?.is_some() {
            return Err(SessionError::AlreadyExists(format!(
                "Session already exists: {id}"
            )));
        }
        let created_at = now_ms();
        let dir = self.session_dir();
        fs::create_dir_all(&dir)
            .map_err(|error| storage_error("Failed to create sessions directory", &dir, error))?;
        let path = dir.join(session_file_name(created_at, &id));
        let header = JsonlV4Header {
            kind: HeaderKind::Header,
            version: 4,
            id,
            created_at,
            cwd: self.cwd.clone(),
            parent_session_id: options.parent_session_id.clone(),
            legacy_parent_session_path: None,
            metadata: options.metadata.clone(),
        };
        Ok((header, path))
    }
}

impl SessionRepo for JsonlRepo {
    fn create(&mut self, options: CreateOptions) -> Result<SharedSession, SessionError> {
        let (header, path) = self.prepare_create(&options)?;
        let line = encode_header(&header)?;
        fs::write(&path, line)
            .map_err(|error| storage_error("Failed to initialize session", &path, error))?;
        let store = SessionStore::file_backed(metadata_from_header(&header), path);
        Ok(Arc::new(Mutex::new(store)))
    }

    fn open(&mut self, id: &str) -> Result<SharedSession, SessionError> {
        let path = self
            .find_session_file(id)?
            .ok_or_else(|| SessionError::NotFound(format!("Session not found: {id}")))?;
        let store = load_session(&path)?;
        if store.metadata().id != id {
            return Err(SessionError::InvalidEntry(format!(
                "Session id does not match header: {id}"
            )));
        }
        Ok(Arc::new(Mutex::new(store)))
    }

    fn list(&mut self) -> Result<Vec<SessionMetadata>, SessionError> {
        crate::listing::list(&self.session_dir())
    }

    fn delete(&mut self, id: &str) -> Result<(), SessionError> {
        if let Some(path) = self.find_session_file(id)? {
            let held = path.with_extension("");
            if held.is_dir() {
                fs::remove_dir_all(&held).map_err(|error| {
                    storage_error("Failed to delete its children", &held, error)
                })?;
            }
            fs::remove_file(&path)
                .map_err(|error| storage_error("Failed to delete session", &path, error))?;
        }
        Ok(())
    }

    fn fork(
        &mut self,
        source_id: &str,
        scope: &ForkScope,
        options: CreateOptions,
    ) -> Result<SharedSession, SessionError> {
        let source = self.open(source_id)?;
        let mutations = lock_session(&source).fork_mutations(scope)?;
        let create_options = CreateOptions {
            parent_session_id: options
                .parent_session_id
                .or_else(|| Some(source_id.to_owned())),
            ..options
        };
        let (header, path) = self.prepare_create(&create_options)?;
        let mut staged = encode_header(&header)?;
        for mutation in &mutations {
            let json = serde_json::to_string(mutation).map_err(|error| {
                SessionError::Storage(format!("Failed to encode session mutation: {error}"))
            })?;
            staged.push_str(&json);
            staged.push('\n');
        }
        publish_atomically(&path, staged.as_bytes())?;
        Ok(Arc::new(Mutex::new(load_session(&path)?)))
    }
}
