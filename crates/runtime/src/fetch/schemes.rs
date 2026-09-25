use std::path::{Path, PathBuf};

use yi_permission::lexical_normalize;
use yi_session::{EntryOrder, EntryQuery};
use yi_tools::hashline::format::compute_file_hash;
use yi_tools::hashline::normalize::{normalize_to_lf, strip_bom};
use yi_types::message::UserContent;
use yi_types::plan::ids::TodoAddr;
use yi_types::url::Url;

use super::{
    CHECKPOINT_MISSING, FetchError, KERNEL_MISSING, MCP_MISSING, Page, Resolver, SESSION_MISSING,
    Transcript, unsupported,
};
use crate::kernel::{VariableName, VariableReadError};
use crate::plan::import::{parse_document, section_of};
use crate::plan::store::PlanStore;

type Served = (String, String);
type Paged = Result<(String, String, Option<usize>), FetchError>;

fn render_history(
    url: &Url,
    session: &yi_session::SharedSession,
    entry_id: Option<&str>,
    page: Option<Page>,
) -> Paged {
    // `custom/<type>` closes any listing and keeps that custom type alone: an inbox (D214).
    let (entry_id, custom) = match entry_id.and_then(|selector| selector.rsplit_once("custom/")) {
        Some((head, custom)) if head.is_empty() || head.ends_with('/') => {
            let head = head.trim_end_matches('/');
            ((!head.is_empty()).then_some(head), Some(custom))
        }
        _ => (entry_id, None),
    };
    // A page counts entries that stay put: `tail/N` is anchored at the end, so an append slides it.
    if page.is_some() && entry_id.is_some_and(|id| !id.starts_with("since/")) {
        return Err(FetchError::BadAddress {
            url: url.to_string(),
            detail:
                "offset and limit page the whole listing or since/<seq>, whose entries stay put"
                    .to_owned(),
        });
    }
    let paged = |lines: Vec<String>, served_by: String| {
        let (lines, next) = match page {
            Some(page) => page.window(lines),
            None => (lines, None),
        };
        Ok((lines.join("\n"), served_by, next))
    };
    let backend = |message: String| FetchError::Backend {
        url: url.to_string(),
        message,
    };
    let store = yi_session::lock_session(session);
    let all = |custom_type: Option<&str>| {
        store
            .find_entries(&EntryQuery {
                custom_type: custom_type.map(str::to_owned),
                order: EntryOrder::OldestFirst,
                ..EntryQuery::default()
            })
            .map_err(|error| backend(error.to_string()))
    };
    // `tail/N` is the last N entries compact, `since/S` everything after sequence S (D165).
    let window = entry_id
        .and_then(|selector| selector.split_once('/'))
        .filter(|(kind, _)| matches!(*kind, "tail" | "since"))
        .and_then(|(kind, raw)| raw.parse::<u64>().ok().map(|number| (kind, number)));
    match (entry_id, window) {
        (_, Some((kind, number))) => {
            let entries = all(custom)?;
            // A filtered listing is read for its data, which the compact line drops.
            let line = |entry: &yi_types::entry::Entry| match custom {
                Some(_) => serde_json::to_string(entry).unwrap_or_default(),
                None => crate::family::compact_entry(entry),
            };
            let kept: Vec<String> = match kind {
                "tail" => entries
                    .iter()
                    .rev()
                    .take(usize::try_from(number).unwrap_or(usize::MAX))
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .map(line)
                    .collect(),
                _ => entries
                    .iter()
                    .filter(|entry| entry.seq() > number)
                    .map(line)
                    .collect(),
            };
            paged(kept, format!("session-{kind}"))
        }
        (Some(_), None) if custom.is_some() => Err(FetchError::BadAddress {
            url: url.to_string(),
            detail: "custom/<type> filters a listing: the whole one, tail/<n> or since/<seq>"
                .to_owned(),
        }),
        (Some(id), None) => {
            let entry = store.entry(id).ok_or_else(|| FetchError::NotFound {
                url: url.to_string(),
                what: format!("entry {id}"),
            })?;
            let text =
                serde_json::to_string_pretty(&entry).map_err(|error| backend(error.to_string()))?;
            Ok((text, "session-entry".to_owned(), None))
        }
        (None, None) => {
            let entries = all(custom)?;
            let lines = entries
                .iter()
                .map(serde_json::to_string)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| backend(error.to_string()))?;
            paged(lines, "session-transcript".to_owned())
        }
    }
}

impl Resolver {
    pub(super) fn resolve_local(&self, url: &Url) -> Result<Served, FetchError> {
        let raw = Path::new(url.path());
        let outside = |path: PathBuf| FetchError::OutsideWorkspace {
            url: url.to_string(),
            path,
        };
        let (path, served_by, root) = if raw.is_absolute() {
            let normalized = lexical_normalize(raw);
            let inside = self
                .spill_dir()
                .filter(|spill| normalized.starts_with(lexical_normalize(spill)));
            let Some(spill) = inside else {
                return Err(outside(normalized));
            };
            (normalized, "spill-file", spill.to_path_buf())
        } else {
            let root = lexical_normalize(self.workspace());
            let normalized = lexical_normalize(&self.workspace().join(raw));
            if !normalized.starts_with(&root) {
                return Err(outside(normalized));
            }
            (normalized, "workspace-file", root)
        };
        let text = read_text(url, &self.resolved(url, path, &root)?)?;
        Ok((apply_fragment(url, text)?, served_by.to_owned()))
    }

    /// Incident: containment was lexical, so a link at `notes -> /etc/passwd` was served.
    /// What the read lands on takes the same wall a path naming it directly takes.
    fn resolved(&self, url: &Url, path: PathBuf, root: &Path) -> Result<PathBuf, FetchError> {
        let Ok(real) = std::fs::canonicalize(&path) else {
            return Ok(path);
        };
        let real_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        if !real.starts_with(&real_root) {
            return Err(FetchError::OutsideWorkspace {
                url: url.to_string(),
                path: real,
            });
        }
        match self.wall().check_read_path(&real) {
            Some(refusal) => Err(FetchError::Denied {
                url: url.to_string(),
                refusal,
            }),
            None => Ok(real),
        }
    }

    pub(super) fn resolve_plan(&self, url: &Url) -> Result<Served, FetchError> {
        let (plan_id, slug) = match url.path().split_once('/') {
            Some((plan_id, slug)) => (plan_id, Some(slug)),
            None => (url.path(), None),
        };
        let backend = |message: String| FetchError::Backend {
            url: url.to_string(),
            message,
        };
        let legacy = self.plans_dir().join(format!("{plan_id}.md"));
        let (plan, body) = if legacy.is_file() {
            let text = read_text(url, &legacy)?;
            if slug.is_none() {
                return Ok((text, "plan-file".to_owned()));
            }
            let document = parse_document(&text).map_err(|error| backend(error.to_string()))?;
            (document.plan, Some(document.body))
        } else {
            let id = yi_types::plan::doc::PlanId::new(plan_id).map_err(|error| {
                FetchError::BadAddress {
                    url: url.to_string(),
                    detail: error.to_string(),
                }
            })?;
            let store = PlanStore::open(self.plans_dir().to_path_buf())
                .map_err(|error| backend(error.to_string()))?;
            // Invariant: the journal decides, for the bare address too; a lagging or lost
            // checkpoint is regenerated by the read, never served raw.
            let plan = store.read(&id).map_err(|error| match error {
                crate::plan::store::StoreError::Missing { .. } => FetchError::NotFound {
                    url: url.to_string(),
                    what: format!("plan {plan_id}"),
                },
                other => backend(other.to_string()),
            })?;
            if slug.is_none() {
                let text = PlanStore::render(&plan).map_err(|error| backend(error.to_string()))?;
                return Ok((text, "plan-file".to_owned()));
            }
            // A blob by digest (a recorded product or source); no todo slug carries a slash.
            if let Some(hex) = slug.and_then(|slug| slug.strip_prefix("artifacts/")) {
                let bytes = format!("{}{hex}", yi_types::plan::canonical::DIGEST_PREFIX)
                    .try_into()
                    .map_err(|error: yi_types::plan::canonical::DigestError| error.to_string())
                    .and_then(|digest| {
                        let blobs = store.artifacts(&id);
                        blobs.get(&digest).map_err(|error| error.to_string())
                    })
                    .map_err(|what| FetchError::NotFound {
                        url: url.to_string(),
                        what,
                    })?;
                let text = String::from_utf8(bytes)
                    .map_err(|_| backend("the artifact is not UTF-8 text".to_owned()))?;
                return Ok((text, "plan-artifact".to_owned()));
            }
            (plan, None)
        };
        let Some(slug) = slug else {
            return Err(backend(
                "unreachable: a bare plan address returns above".to_owned(),
            ));
        };
        let todo = plan
            .todos
            .iter()
            .find(|todo| {
                TodoAddr {
                    plan: plan.id.clone(),
                    todo: todo.label.clone(),
                }
                .to_url()
                .is_ok_and(|addr| addr.path() == url.path())
            })
            .ok_or_else(|| FetchError::NotFound {
                url: url.to_string(),
                what: format!("todo slug {slug} in plan {plan_id}"),
            })?;
        let mut rendered =
            serde_json::to_string_pretty(todo).map_err(|error| backend(error.to_string()))?;
        let prose = match &body {
            Some(body) => section_of(body, todo.label.as_str()),
            None => todo.note.as_ref().map(|note| note.as_str().to_owned()),
        };
        if let Some(prose) = prose {
            rendered.push_str("\n\n");
            rendered.push_str(&prose);
        }
        Ok((rendered, "plan-file".to_owned()))
    }

    /// Invariant: an agent name carries its own slash, so the whole path is tried as an agent
    /// before an entry id is split off; splitting first read a reap pin as a missing entry.
    pub(super) fn resolve_history(&self, url: &Url, page: Option<Page>) -> Paged {
        let path = url.path();
        if let Some(session) = self.transcript_of(path) {
            return render_history(url, &session, None, page);
        }
        for (at, _) in path.rmatch_indices('/') {
            let (agent, entry) = path.split_at(at);
            if let Some(session) = self.transcript_of(agent) {
                return render_history(url, &session, entry.get(1..), page);
            }
        }
        if self.session_store().is_none() && self.transcripts().is_none() {
            return Err(unsupported(url, SESSION_MISSING));
        }
        Err(FetchError::NotFound {
            url: url.to_string(),
            what: format!(
                "agent {path}: not the attached session, not a live child, and no session on disk carries that id"
            ),
        })
    }

    /// One address space: this session (by its name or as `self`), a live child, the corpus.
    pub(super) fn transcript_of(&self, agent: &str) -> Option<yi_session::SharedSession> {
        if agent == super::SELF || self.session_agent() == Some(agent) {
            return self.session_store();
        }
        self.transcripts()
            .and_then(|desk| desk.open(agent))
            .map(Transcript::session)
    }

    /// The one error shape both kernel paths return (D164).
    fn kernel_error(
        url: &Url,
        agent: &str,
        variable: &VariableName,
        error: VariableReadError,
    ) -> FetchError {
        match error {
            VariableReadError::NotRunning => FetchError::NotFound {
                url: url.to_string(),
                what: format!("a running kernel for agent {agent}"),
            },
            error @ VariableReadError::NotAnIdentifier { .. } => FetchError::BadAddress {
                url: url.to_string(),
                detail: error.to_string(),
            },
            error @ (VariableReadError::Cell { .. } | VariableReadError::Unreadable { .. }) => {
                FetchError::Backend {
                    url: url.to_string(),
                    message: format!("{error} ({variable})"),
                }
            }
        }
    }

    /// `family://<name>`: the blackboard entry's sidecar (D164), written by `rlm.put`.
    pub(super) fn resolve_family(&self, url: &Url) -> Result<Served, FetchError> {
        let name = url.path();
        let clean = !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
        if !clean {
            return Err(FetchError::BadAddress {
                url: url.to_string(),
                detail: "a family address is family://<name> with a plain file-safe name"
                    .to_owned(),
            });
        }
        let Some(dir) = self.family_dir() else {
            return Err(unsupported(url, "this session has no family directory"));
        };
        crate::wiring::read_board(&dir.join(format!("{name}.json")))
            .map(|text| (text, "family-blackboard".to_owned()))
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::NotFound => FetchError::NotFound {
                    url: url.to_string(),
                    what: format!("blackboard entry {name} (rlm.put writes one)"),
                },
                _ => FetchError::Denied {
                    url: url.to_string(),
                    refusal: error.to_string(),
                },
            })
    }

    /// `tree://<agent>/<path>`: a file in a family member's own checkout, read-only and walled
    /// by the reader's `deny_read` (D164).
    pub(super) fn resolve_tree(&self, url: &Url) -> Result<Served, FetchError> {
        let Some((agent, rel)) = url.path().split_once('/') else {
            return Err(FetchError::BadAddress {
                url: url.to_string(),
                detail: "a tree address is tree://<agent>/<path>".to_owned(),
            });
        };
        let Some(trees) = self.member_trees() else {
            return Err(unsupported(
                url,
                "this session has no family to read trees of",
            ));
        };
        let Some(root) = trees.cwd_of(agent) else {
            return Err(FetchError::NotFound {
                url: url.to_string(),
                what: format!("agent {agent} with a checkout"),
            });
        };
        let root = lexical_normalize(&root);
        let path = lexical_normalize(&root.join(rel));
        if !path.starts_with(&root) {
            return Err(FetchError::OutsideWorkspace {
                url: url.to_string(),
                path,
            });
        }
        if let Some(refusal) = self.wall().check_read_path(&path) {
            return Err(FetchError::Denied {
                url: url.to_string(),
                refusal,
            });
        }
        std::fs::read_to_string(&path)
            .map(|text| (text, format!("member-tree {agent}")))
            .map_err(|_| FetchError::NotFound {
                url: url.to_string(),
                what: format!("{rel} in the checkout of {agent}"),
            })
    }

    /// The object behind `kernel://<agent>/<var>`, dilled by its own kernel into the family
    /// dir (D164): the path and its size.
    pub fn dump_kernel(&self, url: &Url) -> Result<(PathBuf, u64), FetchError> {
        let bad = |detail: String| FetchError::BadAddress {
            url: url.to_string(),
            detail,
        };
        let Some((agent, raw)) = url.path().split_once('/') else {
            return Err(bad("an object address is kernel://<agent>/<var>".to_owned()));
        };
        let variable = VariableName::parse(raw).map_err(|error| bad(error.to_string()))?;
        let Some(kernels) = self.kernel_variables() else {
            return Err(unsupported(url, KERNEL_MISSING));
        };
        let Some(dir) = self.family_dir() else {
            return Err(unsupported(url, "this session has no family directory"));
        };
        let path = dir.join(format!("{agent}.{variable}.dill"));
        match kernels.dump(agent, &variable, &path) {
            Ok(Some(_)) if !crate::wiring::is_board_file(&path) => Err(FetchError::Denied {
                url: url.to_string(),
                refusal: format!("{} is not a regular file on the board", path.display()),
            }),
            Ok(Some(bytes)) => Ok((path, bytes)),
            Ok(None) => Err(FetchError::NotFound {
                url: url.to_string(),
                what: format!("variable {variable} in the {agent} namespace"),
            }),
            Err(error) => Err(Self::kernel_error(url, agent, &variable, error)),
        }
    }

    pub(super) fn resolve_kernel(&self, url: &Url, page: Option<Page>) -> Paged {
        let bad = |detail: String| FetchError::BadAddress {
            url: url.to_string(),
            detail,
        };
        let (agent, raw) = match url.path().split_once('/') {
            Some((agent, raw)) => (agent, raw),
            None => {
                let owner = self.session_agent().ok_or_else(|| {
                    bad(
                        "the owner is elided and no session is attached to stand in for it"
                            .to_owned(),
                    )
                })?;
                (owner, url.path())
            }
        };
        let variable = VariableName::parse(raw).map_err(|error| bad(error.to_string()))?;
        let Some(kernels) = self.kernel_variables() else {
            return Err(unsupported(url, KERNEL_MISSING));
        };
        match kernels.read(agent, &variable, page) {
            Ok(Some((text, next))) => Ok((text, format!("kernel-namespace {agent}"), next)),
            Ok(None) => Err(FetchError::NotFound {
                url: url.to_string(),
                what: format!("variable {variable} in the {agent} namespace"),
            }),
            Err(error) => Err(Self::kernel_error(url, agent, &variable, error)),
        }
    }

    /// Invariant: [`super::FetchLog::register_pin`] admits only durable pins,
    /// so the re-resolution below terminates — a pin never names `agent://`.
    pub(super) fn resolve_agent(&self, url: &Url) -> Result<Served, FetchError> {
        // §5: a live child is inspectable at will and a dead one through what
        // the reap promoted, so the running transcript answers before the pin.
        if let Some(Transcript::Live(session)) =
            self.transcripts().and_then(|desk| desk.open(url.path()))
        {
            let (text, _reaped, _next) = render_history(url, &session, None, None)?;
            return Ok((text, format!("live-child {}", url.path())));
        }
        let Some(pinned) = self.log().pin_of(url) else {
            return Err(FetchError::NotFound {
                url: url.to_string(),
                what: "a reap pin for this delegation: the child is still live, or was never reaped through this log".to_owned(),
            });
        };
        let (text, _served_by, _next) = self.resolve(&pinned, None)?;
        Ok((text, format!("reap-pin {pinned}")))
    }

    /// Invariant: `<n>` is 1-based over [`yi_types::message::Attribution::User`] messages
    /// alone, so a host-minted user-role entry never shifts what a citation names.
    pub(super) fn resolve_user(&self, url: &Url) -> Result<Served, FetchError> {
        let ordinal = url
            .path()
            .parse::<usize>()
            .ok()
            .filter(|n| *n >= 1)
            .ok_or_else(|| FetchError::BadAddress {
                url: url.to_string(),
                detail: "a user address is user://<n> with n a 1-based ordinal over the user's own messages".to_owned(),
            })?;
        let Some(session) = self.session_store() else {
            return Err(unsupported(url, SESSION_MISSING));
        };
        let inputs = super::user_inputs(&session).map_err(|message| FetchError::Backend {
            url: url.to_string(),
            message,
        })?;
        match inputs.get(ordinal.saturating_sub(1)) {
            Some(content) => Ok((user_text(url, content)?, "user-input".to_owned())),
            None => Err(FetchError::NotFound {
                url: url.to_string(),
                what: format!(
                    "attributed user message {ordinal}: the transcript holds {}",
                    inputs.len()
                ),
            }),
        }
    }

    pub(super) fn resolve_mcp(&self, url: &Url) -> Result<Served, FetchError> {
        let bad = || FetchError::BadAddress {
            url: url.to_string(),
            detail: "an MCP address is <server>/<resource uri>".to_owned(),
        };
        let Some((server, resource)) = url.path().split_once('/') else {
            return Err(bad());
        };
        if server.is_empty() || resource.is_empty() {
            return Err(bad());
        }
        let Some(reader) = self.mcp_read() else {
            return Err(unsupported(url, MCP_MISSING));
        };
        let contents = reader
            .read(server, resource)
            .map_err(|message| FetchError::Backend {
                url: url.to_string(),
                message,
            })?;
        Ok((contents, format!("mcp-server {server}")))
    }

    pub(super) fn resolve_checkpoint(&self, url: &Url) -> Result<Served, FetchError> {
        let Some((tree, path)) = url.path().split_once('/') else {
            return Err(FetchError::BadAddress {
                url: url.to_string(),
                detail: "a checkpoint address is <tree>/<path>".to_owned(),
            });
        };
        if !is_git_object_id(tree) {
            return Err(FetchError::BadAddress {
                url: url.to_string(),
                detail: format!("tree {tree} is not a 40- or 64-hex git object id"),
            });
        }
        let Some(show) = self.checkpoint_show() else {
            return Err(unsupported(url, CHECKPOINT_MISSING));
        };
        let raw = show
            .show(tree, path)
            .map_err(|message| FetchError::Backend {
                url: url.to_string(),
                message,
            })?;
        let text = normalize_to_lf(strip_bom(&raw).text);
        Ok((
            apply_fragment(url, text)?,
            format!("checkpoint-tree {tree}"),
        ))
    }
}

fn user_text(url: &Url, content: &UserContent) -> Result<String, FetchError> {
    match content {
        UserContent::Text(text) => Ok(text.clone()),
        UserContent::Blocks(_) => {
            serde_json::to_string_pretty(content).map_err(|error| FetchError::Backend {
                url: url.to_string(),
                message: error.to_string(),
            })
        }
    }
}

fn read_text(url: &Url, path: &Path) -> Result<String, FetchError> {
    match std::fs::read_to_string(path) {
        Ok(raw) => Ok(normalize_to_lf(strip_bom(&raw).text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(FetchError::NotFound {
            url: url.to_string(),
            what: path.display().to_string(),
        }),
        Err(error) => Err(FetchError::Backend {
            url: url.to_string(),
            message: error.to_string(),
        }),
    }
}

/// Invariant: the fragment tag pins the whole file, never the span — the line
/// range only locates within the pinned version, so any edit anywhere is stale.
fn apply_fragment(url: &Url, text: String) -> Result<String, FetchError> {
    let Some(fragment) = url.fragment() else {
        return Ok(text);
    };
    let live = compute_file_hash(&text);
    if live.0 != fragment.tag() {
        return Err(FetchError::Stale {
            url: url.to_string(),
            expected: format!("{:04X}", fragment.tag()),
            found: live.to_string(),
        });
    }
    let lines: Vec<&str> = text.lines().collect();
    let start = (fragment.start().get() as usize).saturating_sub(1);
    let end = fragment.end().get() as usize;
    lines
        .get(start..end)
        .map(|span| span.join("\n"))
        .ok_or_else(|| FetchError::BadAddress {
            url: url.to_string(),
            detail: format!(
                "line range {}-{} is beyond the {} lines of the pinned file",
                fragment.start(),
                fragment.end(),
                lines.len()
            ),
        })
}

fn is_git_object_id(tree: &str) -> bool {
    (tree.len() == 40 || tree.len() == 64) && tree.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::fetch::{FetchLog, KernelVariables, McpResourceRead, open_checkpoint_show};
    use crate::scratch::Scratch;
    use crate::wall::Wall;
    use yi_types::message::AgentMessage;

    struct StubKernel;

    impl KernelVariables for StubKernel {
        fn read(
            &self,
            agent: &str,
            variable: &VariableName,
            _page: Option<Page>,
        ) -> Result<Option<(String, Option<usize>)>, VariableReadError> {
            match (agent, variable.as_str()) {
                ("ghost", _) => Err(VariableReadError::NotRunning),
                (_, "answer") => Ok(Some(("42".to_owned(), None))),
                _ => Ok(None),
            }
        }

        fn dump(
            &self,
            agent: &str,
            variable: &VariableName,
            path: &std::path::Path,
        ) -> Result<Option<u64>, VariableReadError> {
            match (agent, variable.as_str()) {
                ("ghost", _) => Err(VariableReadError::NotRunning),
                (_, "answer") => {
                    std::fs::create_dir_all(path.parent().unwrap_or(path)).ok();
                    std::fs::write(path, b"pickled").ok();
                    Ok(Some(7))
                }
                _ => Ok(None),
            }
        }
    }

    struct StubTrees(PathBuf);

    impl crate::fetch::MemberTrees for StubTrees {
        fn cwd_of(&self, agent: &str) -> Option<PathBuf> {
            (agent == "worker").then(|| self.0.clone())
        }
    }

    #[test]
    fn a_family_entry_a_member_tree_and_a_kernel_object_resolve() -> TestResult {
        let workspace = Scratch::new("yi-schemes-family-ws")?;
        let family = Scratch::new("yi-schemes-family-dir")?;
        let tree = Scratch::new("yi-schemes-family-tree")?;
        std::fs::write(
            family.join("shard.json"),
            r#"{"owner":"read-auth","bytes":12}"#,
        )?;
        std::fs::create_dir_all(tree.join("src"))?;
        std::fs::write(tree.join("src/x.rs"), "fn x() {}\n")?;
        let mut wall = Wall::default();
        wall.deny_read.push(tree.join("src/secret.rs"));
        std::fs::write(tree.join("src/secret.rs"), "hush\n")?;
        let resolver = Resolver::new(workspace.to_path_buf(), wall)
            .with_family_dir(family.to_path_buf())
            .with_member_trees(Arc::new(StubTrees(tree.to_path_buf())))
            .with_kernel_variables(Arc::new(StubKernel));
        let entry: Url = "family://shard".parse()?;
        assert_eq!(resolver.fetch(&entry)?.served_by, "family-blackboard");
        let missing: Url = "family://nothing".parse()?;
        assert!(matches!(
            resolver.fetch(&missing),
            Err(FetchError::NotFound { .. })
        ));
        let file: Url = "tree://worker/src/x.rs".parse()?;
        let served = resolver.fetch(&file)?;
        assert_eq!(served.text, "fn x() {}\n");
        assert_eq!(served.served_by, "member-tree worker");
        let walled: Url = "tree://worker/src/secret.rs".parse()?;
        assert!(matches!(
            resolver.fetch(&walled),
            Err(FetchError::Denied { .. })
        ));
        let escape: Url = "tree://worker/../elsewhere".parse()?;
        assert!(matches!(
            resolver.fetch(&escape),
            Err(FetchError::OutsideWorkspace { .. })
        ));
        let object: Url = "kernel://main/answer".parse()?;
        let (path, bytes) = resolver.dump_kernel(&object)?;
        assert_eq!((path, bytes), (family.join("main.answer.dill"), 7));
        Ok(())
    }

    struct StubMcp(Result<String, String>);

    impl McpResourceRead for StubMcp {
        fn read(&self, _server: &str, _resource: &str) -> Result<String, String> {
            self.0.clone()
        }
    }

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn a_stale_fragment_fails_loud_naming_both_tags() -> TestResult {
        let workspace = Scratch::new("yi-schemes-stale")?;
        std::fs::write(workspace.join("auth.rs"), "alpha\nbeta\n")?;
        let live = compute_file_hash("alpha\nbeta\n");
        let wrong = yi_tools::hashline::format::FileTag(live.0 ^ 1);
        let resolver = Resolver::new(workspace.to_path_buf(), Wall::default());
        let url: Url = format!("local://auth.rs#L1-2@{wrong}").parse()?;
        let error = resolver.fetch(&url).err().ok_or("stale tag must refuse")?;
        let FetchError::Stale {
            expected, found, ..
        } = error
        else {
            return Err(format!("wrong arm: {error}").into());
        };
        assert_eq!(expected, wrong.to_string());
        assert_eq!(found, live.to_string());
        Ok(())
    }

    #[test]
    fn a_matching_fragment_serves_only_its_lines() -> TestResult {
        let workspace = Scratch::new("yi-schemes-span")?;
        std::fs::write(workspace.join("auth.rs"), "alpha\nbeta\ngamma\n")?;
        let tag = compute_file_hash("alpha\nbeta\ngamma\n");
        let resolver = Resolver::new(workspace.to_path_buf(), Wall::default());
        let url: Url = format!("local://auth.rs#L2-3@{tag}").parse()?;
        let fetched = resolver.fetch(&url)?;
        assert_eq!(fetched.text, "beta\ngamma");
        Ok(())
    }

    #[test]
    fn a_fragment_running_past_eof_refuses_instead_of_truncating() -> TestResult {
        let workspace = Scratch::new("yi-schemes-past-eof")?;
        std::fs::write(workspace.join("short.rs"), "one\ntwo\nthree\n")?;
        let tag = compute_file_hash("one\ntwo\nthree\n");
        let resolver = Resolver::new(workspace.to_path_buf(), Wall::default());
        for range in ["L3-9", "L4-4", "L1-4"] {
            let url: Url = format!("local://short.rs#{range}@{tag}").parse()?;
            let error = resolver
                .fetch(&url)
                .err()
                .ok_or_else(|| format!("{range} was served past EOF instead of refused"))?;
            let FetchError::BadAddress { .. } = error else {
                return Err(format!("{range} was served, not refused: {error}").into());
            };
            assert!(!resolver.log().backs(&url), "{range} was logged as backed");
        }
        let whole: Url = format!("local://short.rs#L1-3@{tag}").parse()?;
        assert_eq!(resolver.fetch(&whole)?.text, "one\ntwo\nthree");
        Ok(())
    }

    #[test]
    fn an_absolute_local_path_reaches_only_the_spill_dir() -> TestResult {
        let workspace = Scratch::new("yi-schemes-spill-ws")?;
        let spill = Scratch::new("yi-schemes-spill-out")?;
        std::fs::write(spill.join("a1b2c3d4.txt"), "spilled\n")?;
        let resolver = Resolver::new(workspace.to_path_buf(), Wall::default())
            .with_spill_dir(spill.to_path_buf());
        let inside: Url = format!("local://{}", spill.join("a1b2c3d4.txt").display()).parse()?;
        assert_eq!(resolver.fetch(&inside)?.served_by, "spill-file");
        let outside: Url = "local:///etc/hosts".parse()?;
        assert!(matches!(
            resolver.fetch(&outside),
            Err(FetchError::OutsideWorkspace { .. })
        ));
        let escape: Url = "local://../outside.txt".parse()?;
        assert!(matches!(
            resolver.fetch(&escape),
            Err(FetchError::OutsideWorkspace { .. })
        ));
        Ok(())
    }

    #[test]
    fn a_plan_todo_is_addressed_by_its_label_slug() -> TestResult {
        let workspace = Scratch::new("yi-schemes-plan")?;
        let plans = workspace.join(".yi/plans");
        std::fs::create_dir_all(&plans)?;
        let document = concat!(
            "---\n",
            "{\"format\": 1, \"plan\": \"auth-refactor\", \"goal\": \"Ship OAuth\", \"version\": 1,\n",
            " \"tier\": \"root\", \"state\": \"active\",\n",
            " \"todos\": [{\"label\": \"Freeze the token API seam\", \"state\": \"pending\"}]}\n",
            "---\n",
            "## Freeze the token API seam\nHold the seam steady.\n"
        );
        std::fs::write(plans.join("auth-refactor.md"), document)?;
        let resolver = Resolver::new(workspace.to_path_buf(), Wall::default());
        let whole: Url = "plan://auth-refactor".parse()?;
        assert_eq!(resolver.fetch(&whole)?.served_by, "plan-file");
        let todo: Url = "plan://auth-refactor/freeze-the-token-api-seam".parse()?;
        let fetched = resolver.fetch(&todo)?;
        assert!(fetched.text.contains("\"state\": \"pending\""));
        assert!(fetched.text.contains("Hold the seam steady."));
        let missing: Url = "plan://auth-refactor/no-such-todo".parse()?;
        assert!(matches!(
            resolver.fetch(&missing),
            Err(FetchError::NotFound { .. })
        ));
        Ok(())
    }

    /// A format-2 plan is served through the journal on both addresses: with the checkpoint
    /// lost in the crash window after a commit, the bare address still answers and the read
    /// regenerates the checkpoint.
    #[test]
    fn a_format_two_plan_is_served_from_its_journal_not_its_checkpoint() -> TestResult {
        use crate::plan::authority::Unhosted;
        use crate::plan::ops::{Actor, Op, OpRequest, PlanEngine, TodoSpec};
        use yi_types::plan::doc::{GoalText, TodoLabel};
        let workspace = Scratch::new("yi-schemes-plan-journal")?;
        let store = PlanStore::open(workspace.join(".yi/plans"))?;
        let engine = PlanEngine::new(store.clone(), Arc::new(Unhosted));
        let out = engine.apply(OpRequest {
            plan: None,
            actor: Actor::Owner,
            op: Op::Init {
                goal: GoalText::new("Ship OAuth")?,
                todos: vec![TodoSpec {
                    label: TodoLabel::new("Freeze the token API seam")?,
                    after: Vec::new(),
                    delegation: None,
                    contract: None,
                    children: Vec::new(),
                }],
            },
            request_id: None,
            expected_revision: None,
        })?;
        let id = out.plan.id;
        std::fs::remove_file(store.path(&id))?;
        let resolver = Resolver::new(workspace.to_path_buf(), Wall::default());
        let whole: Url = format!("plan://{id}").parse()?;
        let fetched = resolver.fetch(&whole)?;
        assert_eq!(fetched.served_by, "plan-file");
        assert!(
            fetched
                .text
                .contains("\"label\": \"Freeze the token API seam\""),
            "{}",
            fetched.text
        );
        assert!(
            store.path(&id).is_file(),
            "the read regenerated the checkpoint"
        );
        let todo: Url = format!("plan://{id}/freeze-the-token-api-seam").parse()?;
        assert!(
            resolver
                .fetch(&todo)?
                .text
                .contains("\"state\": \"pending\"")
        );
        Ok(())
    }

    #[test]
    fn a_checkpoint_address_validates_its_tree_id() -> TestResult {
        let workspace = Scratch::new("yi-schemes-tree")?;
        let resolver = Resolver::new(workspace.to_path_buf(), Wall::default());
        for bad in ["checkpoint://-oops/x", "checkpoint://abc123/x"] {
            let url: Url = bad.parse()?;
            assert!(
                matches!(resolver.fetch(&url), Err(FetchError::BadAddress { .. })),
                "{bad}"
            );
        }
        Ok(())
    }

    #[test]
    fn a_kernel_variable_reads_through_the_attached_seam() -> TestResult {
        let workspace = Scratch::new("yi-schemes-kernel")?;
        let resolver = Resolver::new(workspace.to_path_buf(), Wall::default())
            .with_kernel_variables(Arc::new(StubKernel));
        let bound: Url = "kernel://main/answer".parse()?;
        let fetched = resolver.fetch(&bound)?;
        assert_eq!(fetched.text, "42");
        assert_eq!(fetched.served_by, "kernel-namespace main");
        for url in ["kernel://main/missing", "kernel://ghost/answer"] {
            let parsed: Url = url.parse()?;
            assert!(
                matches!(resolver.fetch(&parsed), Err(FetchError::NotFound { .. })),
                "{url}"
            );
        }
        let dotted: Url = "kernel://main/config.token".parse()?;
        assert!(matches!(
            resolver.fetch(&dotted),
            Err(FetchError::BadAddress { .. })
        ));
        Ok(())
    }

    #[test]
    fn an_mcp_resource_is_served_exactly_as_the_server_returned_it() -> TestResult {
        let workspace = Scratch::new("yi-schemes-mcp")?;
        let contents = r#"{"contents":[{"uri":"issue-42","text":"open"}]}"#;
        let resolver = Resolver::new(workspace.to_path_buf(), Wall::default())
            .with_mcp_read(Arc::new(StubMcp(Ok(contents.to_owned()))));
        let url: Url = "mcp://github/issue-42".parse()?;
        let fetched = resolver.fetch(&url)?;
        assert_eq!(fetched.text, contents);
        assert_eq!(fetched.served_by, "mcp-server github");
        let serverless: Url = "mcp://github".parse()?;
        assert!(matches!(
            resolver.fetch(&serverless),
            Err(FetchError::BadAddress { .. })
        ));
        let failing = Resolver::new(workspace.to_path_buf(), Wall::default()).with_mcp_read(
            Arc::new(StubMcp(Err("@github: resources/read failed".to_owned()))),
        );
        assert!(matches!(
            failing.fetch(&url),
            Err(FetchError::Backend { .. })
        ));
        Ok(())
    }

    #[test]
    fn a_checkpoint_serves_the_file_as_of_its_tree() -> TestResult {
        let home = Scratch::new("yi-schemes-checkpoint-home")?;
        let workspace = Scratch::new("yi-schemes-checkpoint-ws")?;
        std::fs::write(workspace.join("auth.rs"), "alpha\nbeta\ngamma\n")?;
        let checkpoints =
            yi_tools::Checkpoints::open(&crate::checkpoint::checkpoint_root(&home), &workspace)?;
        let tree = checkpoints.capture()?;
        std::fs::write(workspace.join("auth.rs"), "rewritten\n")?;
        let resolver = Resolver::new(workspace.to_path_buf(), Wall::default())
            .with_checkpoint_show(open_checkpoint_show(&home, &workspace)?);
        let url: Url = format!("checkpoint://{}/auth.rs", tree.as_str()).parse()?;
        let fetched = resolver.fetch(&url)?;
        assert_eq!(fetched.text, "alpha\nbeta\ngamma\n");
        assert_eq!(
            fetched.served_by,
            format!("checkpoint-tree {}", tree.as_str())
        );
        Ok(())
    }

    #[test]
    fn history_resolves_entries_through_the_attached_session() -> TestResult {
        let workspace = Scratch::new("yi-schemes-history")?;
        let metadata = yi_session::SessionMetadata {
            id: "test".to_owned(),
            created_at: 0,
            parent_session_id: None,
            name: None,
        };
        let mut store = yi_session::SessionStore::in_memory(metadata);
        let id = store.append_custom("main", "note", None)?;
        let shared: yi_session::SharedSession = std::sync::Arc::new(std::sync::Mutex::new(store));
        let resolver =
            Resolver::new(workspace.to_path_buf(), Wall::default()).with_session("main", shared);
        let entry: Url = format!("history://main/{id}").parse()?;
        assert_eq!(resolver.fetch(&entry)?.served_by, "session-entry");
        let tail: Url = "history://main/tail/1".parse()?;
        let served = resolver.fetch(&tail)?;
        assert_eq!(served.served_by, "session-tail");
        assert_eq!(served.text.lines().count(), 1, "{}", served.text);
        assert!(served.text.starts_with('#'), "{}", served.text);
        let since: Url = "history://main/since/0".parse()?;
        assert_eq!(resolver.fetch(&since)?.served_by, "session-since");
        let transcript: Url = "history://main".parse()?;
        assert_eq!(resolver.fetch(&transcript)?.served_by, "session-transcript");
        let other: Url = "history://sibling".parse()?;
        assert!(matches!(
            resolver.fetch(&other),
            Err(FetchError::NotFound { .. })
        ));
        Ok(())
    }

    /// Incident: `history://<plan>/<todo>/tail/1` read `<plan>` as the agent and missed.
    #[test]
    fn a_window_follows_an_agent_whose_name_has_a_slash() -> TestResult {
        let workspace = Scratch::new("yi-schemes-history-slash")?;
        let mut store = in_memory_session();
        store.append_custom("main", "note", None)?;
        let shared = std::sync::Arc::new(std::sync::Mutex::new(store));
        let resolver = Resolver::new(workspace.to_path_buf(), Wall::default())
            .with_session("plan-a/confmerge", shared);
        let tail: Url = "history://plan-a/confmerge/tail/1".parse()?;
        assert_eq!(resolver.fetch(&tail)?.served_by, "session-tail");
        Ok(())
    }

    fn in_memory_session() -> yi_session::SessionStore {
        yi_session::SessionStore::in_memory(yi_session::SessionMetadata {
            id: "test".to_owned(),
            created_at: 0,
            parent_session_id: None,
            name: None,
        })
    }

    #[test]
    fn user_n_counts_only_attributed_user_messages() -> TestResult {
        let workspace = Scratch::new("yi-schemes-user")?;
        let mut store = in_memory_session();
        store.append_message(
            "main",
            AgentMessage::host_user(UserContent::Text("host-minted preamble".to_owned()), 0),
        )?;
        store.append_message(
            "main",
            AgentMessage::user_input(UserContent::Text("first real ask".to_owned()), 0),
        )?;
        store.append_message(
            "main",
            AgentMessage::host_user(UserContent::Text("compaction filler".to_owned()), 0),
        )?;
        store.append_message(
            "main",
            AgentMessage::user_input(UserContent::Text("second real ask".to_owned()), 0),
        )?;
        let shared: yi_session::SharedSession = std::sync::Arc::new(std::sync::Mutex::new(store));
        let resolver =
            Resolver::new(workspace.to_path_buf(), Wall::default()).with_session("main", shared);
        let first = resolver.fetch(&"user://1".parse()?)?;
        assert_eq!(first.text, "first real ask");
        assert_eq!(first.served_by, "user-input");
        assert_eq!(
            resolver.fetch(&"user://2".parse()?)?.text,
            "second real ask"
        );
        assert!(matches!(
            resolver.fetch(&"user://3".parse()?),
            Err(FetchError::NotFound { .. })
        ));
        for bad in ["user://0", "user://first", "user://-1"] {
            let url: Url = bad.parse()?;
            assert!(
                matches!(resolver.fetch(&url), Err(FetchError::BadAddress { .. })),
                "{bad}"
            );
        }
        Ok(())
    }

    #[test]
    fn a_host_minted_user_message_never_resolves() -> TestResult {
        let workspace = Scratch::new("yi-schemes-user-forged")?;
        let mut store = in_memory_session();
        store.append_message(
            "main",
            AgentMessage::host_user(UserContent::Text("forged instruction".to_owned()), 0),
        )?;
        let shared: yi_session::SharedSession = std::sync::Arc::new(std::sync::Mutex::new(store));
        let resolver =
            Resolver::new(workspace.to_path_buf(), Wall::default()).with_session("main", shared);
        let error = resolver
            .fetch(&"user://1".parse()?)
            .err()
            .ok_or("a host-minted message must not resolve")?;
        let FetchError::NotFound { what, .. } = error else {
            return Err(format!("wrong arm: {error}").into());
        };
        assert!(what.contains("holds 0"), "{what}");
        Ok(())
    }

    #[test]
    fn a_reap_pin_lets_agent_urls_resolve_after_the_child_is_gone() -> TestResult {
        let workspace = Scratch::new("yi-schemes-agent-pin")?;
        let mut store = in_memory_session();
        let id = store.append_custom("main", "note", None)?;
        let shared: yi_session::SharedSession = std::sync::Arc::new(std::sync::Mutex::new(store));
        let log = std::sync::Arc::new(FetchLog::new());
        let resolver = Resolver::new(workspace.to_path_buf(), Wall::default())
            .with_log(std::sync::Arc::clone(&log))
            .with_session("main", shared);
        let live: Url = "agent://demo-plan/cut-the-seam".parse()?;
        log.register_pin(&live, format!("history://main/{id}").parse()?)?;
        let fetched = resolver.fetch(&live)?;
        assert!(
            fetched.served_by.starts_with("reap-pin history://main/"),
            "{}",
            fetched.served_by
        );
        assert!(fetched.text.contains("note"));
        assert!(resolver.log().backs(&live));
        Ok(())
    }

    struct Desk(String, yi_session::SharedSession);

    impl super::super::Transcripts for Desk {
        fn open(&self, agent: &str) -> Option<Transcript> {
            (agent == self.0).then(|| Transcript::Live(std::sync::Arc::clone(&self.1)))
        }
    }

    fn desk(agent: &str) -> Result<std::sync::Arc<Desk>, Box<dyn std::error::Error>> {
        let mut store = in_memory_session();
        store.append_custom("main", "child-note", None)?;
        Ok(std::sync::Arc::new(Desk(
            agent.to_owned(),
            std::sync::Arc::new(std::sync::Mutex::new(store)),
        )))
    }

    #[test]
    fn a_reap_pin_named_like_a_todo_address_still_resolves() -> TestResult {
        let workspace = Scratch::new("yi-schemes-agent-pin-slash")?;
        let log = std::sync::Arc::new(FetchLog::new());
        let resolver = Resolver::new(workspace.to_path_buf(), Wall::default())
            .with_log(std::sync::Arc::clone(&log))
            .with_session(
                "main",
                std::sync::Arc::new(std::sync::Mutex::new(in_memory_session())),
            )
            .with_transcripts(desk("demo-plan/cut-the-seam")?);
        let live: Url = "agent://demo-plan/cut-the-seam".parse()?;
        log.register_pin(&live, "history://demo-plan/cut-the-seam".parse()?)?;
        let pinned: Url = "history://demo-plan/cut-the-seam".parse()?;
        let fetched = resolver.fetch(&pinned)?;
        assert_eq!(fetched.served_by, "session-transcript");
        assert!(fetched.text.contains("child-note"));
        Ok(())
    }

    #[test]
    fn a_live_child_answers_its_own_agent_url_before_any_pin_exists() -> TestResult {
        let workspace = Scratch::new("yi-schemes-agent-live")?;
        let resolver = Resolver::new(workspace.to_path_buf(), Wall::default())
            .with_session(
                "main",
                std::sync::Arc::new(std::sync::Mutex::new(in_memory_session())),
            )
            .with_transcripts(desk("demo-plan/cut-the-seam")?);
        let live: Url = "agent://demo-plan/cut-the-seam".parse()?;
        let fetched = resolver.fetch(&live)?;
        assert_eq!(fetched.served_by, "live-child demo-plan/cut-the-seam");
        assert!(fetched.text.contains("child-note"));
        let unknown: Url = "agent://demo-plan/never-started".parse()?;
        assert!(matches!(
            resolver.fetch(&unknown),
            Err(FetchError::NotFound { .. })
        ));
        Ok(())
    }

    #[test]
    fn history_reaches_a_run_that_is_not_the_attached_session() -> TestResult {
        let workspace = Scratch::new("yi-schemes-history-corpus")?;
        let resolver = Resolver::new(workspace.to_path_buf(), Wall::default())
            .with_session(
                "main",
                std::sync::Arc::new(std::sync::Mutex::new(in_memory_session())),
            )
            .with_transcripts(desk("some-older-run")?);
        let elsewhere: Url = "history://some-older-run".parse()?;
        assert_eq!(resolver.fetch(&elsewhere)?.served_by, "session-transcript");
        let nowhere: Url = "history://a-run-that-never-happened".parse()?;
        assert!(matches!(
            resolver.fetch(&nowhere),
            Err(FetchError::NotFound { .. })
        ));
        Ok(())
    }
}
