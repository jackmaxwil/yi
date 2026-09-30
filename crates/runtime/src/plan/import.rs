//! Import of format 1 (plan section 5.5): explicit, lossless, never a side effect of a read.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::Value;
use yi_types::plan::canonical::ArtifactRef;
use yi_types::plan::doc::{
    DocError, LEGACY_PLAN_FORMAT, NOTE_MAX_BYTES, Note, Plan, PlanId, PlanParseError, PlanState,
    Resolution, Todo, TodoState,
};
use yi_types::url::{Scheme, Url};

use super::artifact::{ArtifactError, Artifacts};
use super::ops::{Actor, Delta, Op, Outcome, PlanEngine, PlanOpError};
use super::state::{KIND_IMPORT, root_of};
use super::store::{PLAN_CAP_BYTES, PlanStore, draft};
use yi_types::plan::ledger::RequestId;

/// Invariant: one cap over the whole document before any parse, so an oversized body is refused whole and never silently shortened.
pub const IMPORT_CAP_BYTES: usize = 1024 * 1024;

pub const MARKDOWN_MEDIA_TYPE: &str = "text/markdown";

pub const NOTE_REF_KEY: &str = "note_ref";

/// Invariant: the frontmatter is JSON between two `---` lines, so the one parser behind a user-editable file is serde_json and nothing hand-rolled.
#[derive(Debug, thiserror::Error)]
pub enum DocumentError {
    #[error("no opening --- delimiter: {head}")]
    NoFrontmatter { head: String },
    #[error("frontmatter opened at line 1 is unclosed after {lines} lines")]
    OpenFrontmatter { lines: usize },
    #[error("frontmatter is {bytes} bytes, over the {cap} cap; nothing was imported")]
    FrontmatterOverCap { bytes: usize, cap: usize },
    #[error("frontmatter: {0}")]
    Plan(#[from] PlanParseError),
}

pub fn split_frontmatter(document: &str) -> Result<(&str, &str), DocumentError> {
    let mut offset = 0usize;
    let mut opened = false;
    let mut start = 0usize;
    let mut count = 0usize;
    for raw in document.split_inclusive('\n') {
        count = count.saturating_add(1);
        let closing = raw.trim_end() == "---";
        if !opened {
            if !closing {
                return Err(DocumentError::NoFrontmatter {
                    head: raw.trim_end().to_string(),
                });
            }
            opened = true;
            start = offset.saturating_add(raw.len());
        } else if closing {
            let body = offset.saturating_add(raw.len());
            return Ok((
                document.get(start..offset).unwrap_or(""),
                document.get(body..).unwrap_or(""),
            ));
        }
        offset = offset.saturating_add(raw.len());
    }
    if opened {
        Err(DocumentError::OpenFrontmatter { lines: count })
    } else {
        Err(DocumentError::NoFrontmatter {
            head: String::new(),
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LegacyDocument {
    pub plan: Plan,
    pub body: String,
}

pub fn parse_document(text: &str) -> Result<LegacyDocument, DocumentError> {
    let (front, body) = split_frontmatter(text)?;
    if front.len() > PLAN_CAP_BYTES {
        return Err(DocumentError::FrontmatterOverCap {
            bytes: front.len(),
            cap: PLAN_CAP_BYTES,
        });
    }
    Ok(LegacyDocument {
        plan: Plan::parse_legacy(front)?,
        body: body.to_owned(),
    })
}

pub fn section_of(body: &str, heading: &str) -> Option<String> {
    let header = format!("## {heading}");
    let mut collected: Vec<&str> = Vec::new();
    let mut inside = false;
    for line in body.lines() {
        if line.trim_end() == header {
            inside = true;
            collected.push(line);
        } else if inside && line.starts_with("## ") {
            break;
        } else if inside {
            collected.push(line);
        }
    }
    inside.then(|| collected.join("\n").trim_end().to_owned())
}

pub fn legacy_path(store: &PlanStore, id: &PlanId) -> PathBuf {
    store.dir().join(format!("{id}.md"))
}

pub fn read_legacy(store: &PlanStore, id: &PlanId) -> Result<LegacyDocument, ImportError> {
    Ok(load(&legacy_path(store, id))?.1)
}

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path}: {source}")]
    Document {
        path: PathBuf,
        source: DocumentError,
    },
    #[error("import source {url} is not a plain local:// file")]
    NotLocal { url: Url },
    #[error(
        "import source {url} is a protected path (a key store, the workspace .git or a device)"
    )]
    Protected { url: Url },
    #[error("{path} is over the {cap} byte import cap; nothing was imported")]
    OverCap { path: PathBuf, cap: usize },
    #[error("{path} is not UTF-8")]
    NotUtf8 { path: PathBuf },
    #[error("plan {id} is already format 2 at {path}")]
    AlreadyImported { id: PlanId, path: PathBuf },
    #[error("checkpoint did not read as a format-2 plan: {detail}")]
    Checkpoint { detail: String },
    #[error("{0}")]
    Artifact(#[from] ArtifactError),
    #[error("note of todo {label:?}: {source}")]
    Note { label: String, source: DocError },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Imported {
    pub plan: Plan,
    pub artifact: ArtifactRef,
    pub format: u32,
    pub source: Url,
}

fn io_at(path: &Path) -> impl FnOnce(std::io::Error) -> ImportError + '_ {
    move |source| ImportError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// The bounded reader in front of every decode: one cap over the whole file.
fn read_capped(path: &Path) -> Result<Vec<u8>, ImportError> {
    let file = std::fs::File::open(path).map_err(io_at(path))?;
    let cap = u64::try_from(IMPORT_CAP_BYTES).unwrap_or(u64::MAX);
    let mut bytes = Vec::new();
    file.take(cap.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(io_at(path))?;
    if bytes.len() > IMPORT_CAP_BYTES {
        return Err(ImportError::OverCap {
            path: path.to_path_buf(),
            cap: IMPORT_CAP_BYTES,
        });
    }
    Ok(bytes)
}

fn parse_legacy(path: &Path, bytes: &[u8]) -> Result<LegacyDocument, ImportError> {
    let text = std::str::from_utf8(bytes).map_err(|_not_utf8| ImportError::NotUtf8 {
        path: path.to_path_buf(),
    })?;
    parse_document(text).map_err(|source| ImportError::Document {
        path: path.to_path_buf(),
        source,
    })
}

fn load(path: &Path) -> Result<(Vec<u8>, LegacyDocument), ImportError> {
    let bytes = read_capped(path)?;
    let document = parse_legacy(path, &bytes)?;
    Ok((bytes, document))
}

fn source_path(cwd: &Path, source: &Url) -> Result<PathBuf, ImportError> {
    if source.scheme() != &Scheme::Local || source.fragment().is_some() {
        return Err(ImportError::NotLocal {
            url: source.clone(),
        });
    }
    let raw = Path::new(source.path());
    let path = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        cwd.join(raw)
    };
    // Judged before the open, so no error can quote a key's first line (D323).
    let context = yi_permission::CatastrophicContext::detect(cwd);
    if yi_permission::read_is_catastrophic(&path, &context) {
        return Err(ImportError::Protected {
            url: source.clone(),
        });
    }
    Ok(path)
}

fn map_sections(
    todos: &mut [Todo],
    body: &str,
    artifacts: &Artifacts,
    store: &PlanStore,
) -> Result<(), ImportError> {
    for todo in todos {
        if let Some(section) = section_of(body, todo.label.as_str()) {
            if section.len() <= NOTE_MAX_BYTES {
                let note = Note::new(section).map_err(|source| ImportError::Note {
                    label: todo.label.as_str().to_owned(),
                    source,
                })?;
                todo.note = Some(note);
            } else {
                let blob =
                    artifacts.put(section.as_bytes(), MARKDOWN_MEDIA_TYPE, &store.nonce())?;
                todo.extra
                    .insert(NOTE_REF_KEY.to_owned(), Value::String(blob.to_string()));
            }
        }
        map_sections(&mut todo.children, body, artifacts, store)?;
    }
    Ok(())
}

/// An imported `done` is history, never evidence (plan section 3.6): whatever the file said, no
/// verdict in this journal stands behind it.
fn mark_legacy(todos: &mut [Todo]) {
    for todo in todos {
        if let TodoState::Done { resolution, .. } = &mut todo.state {
            *resolution = Some(Resolution::LegacyUnverified);
        }
        mark_legacy(&mut todo.children);
    }
}

/// A document read and parsed, nothing written yet: the engine consults the journal between
/// this and [`store`], so a retry replays and a plan the journal already holds is refused.
#[must_use]
#[derive(Debug)]
pub struct Source {
    pub plan: Plan,
    bytes: Vec<u8>,
    body: String,
    format: u32,
    media_type: &'static str,
    url: Url,
}

/// A format-2 checkpoint (a clone's `plan.json`) reads as a plan with no body; a format-1
/// document parses as before.
pub fn read(cwd: &Path, source: &Url) -> Result<Source, ImportError> {
    let path = source_path(cwd, source)?;
    let bytes = read_capped(&path)?;
    if bytes.trim_ascii_start().starts_with(b"{") {
        let id = path
            .parent()
            .and_then(Path::file_name)
            .and_then(std::ffi::OsStr::to_str)
            .and_then(|name| PlanId::new(name).ok())
            .ok_or_else(|| ImportError::NotLocal {
                url: source.clone(),
            })?;
        let plan = PlanStore::decode_checkpoint(path, &bytes, &id)
            .map_err(|error| ImportError::Checkpoint {
                detail: error.to_string(),
            })?
            .unmarked();
        return Ok(Source {
            plan,
            bytes,
            body: String::new(),
            format: yi_types::plan::doc::PLAN_FORMAT,
            media_type: "application/json",
            url: source.clone(),
        });
    }
    let document = parse_legacy(&path, &bytes)?;
    Ok(Source {
        plan: document.plan,
        bytes,
        body: document.body,
        format: LEGACY_PLAN_FORMAT,
        media_type: MARKDOWN_MEDIA_TYPE,
        url: source.clone(),
    })
}

/// The blobs: the original bytes whole, and every body section over the note cap.
pub fn store(store: &PlanStore, source: Source) -> Result<Imported, ImportError> {
    let Source {
        mut plan,
        bytes,
        body,
        format,
        media_type,
        url,
    } = source;
    let artifacts = store.artifacts(&plan.id);
    let artifact = artifacts.put(&bytes, media_type, &store.nonce())?;
    map_sections(&mut plan.todos, &body, &artifacts, store)?;
    mark_legacy(&mut plan.todos);
    Ok(Imported {
        plan,
        artifact,
        format,
        source: url,
    })
}

impl PlanEngine {
    pub(super) fn import(
        &self,
        source: &Url,
        actor: &Actor,
        request: RequestId,
    ) -> Result<Outcome, PlanOpError> {
        let read = read(&self.cwd, source)?;
        super::table::validate_plan(&read.plan)?;
        let id = read.plan.id.clone();
        let root = root_of(&id)?;
        if read.plan.state == PlanState::Active
            && let Some(open) = self.roots()?.into_iter().find(|other| {
                *other != root
                    && self
                        .store
                        .read(other)
                        .is_ok_and(|plan| plan.state == PlanState::Active)
            })
        {
            return Err(PlanOpError::PlanExists { id: open });
        }
        let op = Op::Import {
            source: source.clone(),
        };
        let mut txn = self.begin(&root, actor, request, None, true)?;
        if let Some(replayed) = self.replay(&txn, &op)? {
            return Ok(replayed);
        }
        // Invariant: the journal, not the checkpoint file, says whether the plan exists, so a
        // checkpoint lost behind a live journal cannot be imported over.
        if txn.state.plans.contains_key(&id) {
            return Err(ImportError::AlreadyImported {
                path: self.store.path(&id),
                id,
            }
            .into());
        }
        let imported = store(&self.store, read)?;
        let mut record = draft(
            &id,
            KIND_IMPORT,
            txn.actor.clone(),
            self.store.now_ms(),
            u32::try_from(imported.plan.todos.len()).unwrap_or(u32::MAX),
            serde_json::json!({
                "source": imported.source,
                "artifact": imported.artifact.to_string(),
                "format": imported.format,
                "plan": imported.plan.clone().unmarked(),
            }),
            txn.request.clone(),
            0,
            None,
        );
        record.record.extra.insert(
            "media_type".to_owned(),
            Value::String(imported.artifact.media_type.clone()),
        );
        record
            .record
            .extra
            .insert("length".to_owned(), Value::from(imported.artifact.length));
        PlanStore::render(&imported.plan)?;
        let committed = self.commit(&mut txn, record)?;
        self.store.checkpoint_family(&txn.state)?;
        self.emit(&committed);
        let mut outcome = self.conclude(&id, &txn.state, &[], Delta::default())?;
        outcome.notices.push(format!(
            "imported {} as {} (original kept as {})",
            imported.source, id, imported.artifact
        ));
        Ok(outcome)
    }
}
