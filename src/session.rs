//! A Review session: the Findings under Adjudication and the Reviewer's Verdicts.

use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::model::{Finding, Location, Severity};

pub const REVIEWER_ID_PREFIX: &str = "reviewer-";

/// What is being reviewed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Target {
    /// A local git range with no Code host.
    Local { base_ref: String, head_ref: String },
}

impl Target {
    pub fn label(&self) -> String {
        match self {
            Target::Local { base_ref, head_ref } => format!("{base_ref}...{head_ref}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Open,
    Submitted,
    Abandoned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Agent,
    Reviewer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewState {
    RequestChanges,
    Comment,
    Approve,
}

impl ReviewState {
    pub fn label(self) -> &'static str {
        match self {
            ReviewState::RequestChanges => "Request changes",
            ReviewState::Comment => "Comment",
            ReviewState::Approve => "Approve",
        }
    }
}

// A session holds a handful of entries, so the size gap between variants is irrelevant.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Proposal {
    Agent(Finding),
    Reviewer { location: Location, comment: String },
}

/// The Reviewer's ruling on a Finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum Verdict {
    Block,
    Comment,
    Nit,
    Dismiss { reason: String },
    Escalate { to: String },
}

impl Verdict {
    pub fn from_form(verdict: &str, reason: &str, escalate_to: &str) -> Result<Verdict, String> {
        let (reason, to) = (reason.trim(), escalate_to.trim());
        match verdict {
            "block" => Ok(Verdict::Block),
            "comment" => Ok(Verdict::Comment),
            "nit" => Ok(Verdict::Nit),
            "dismiss" if reason.is_empty() => Err("Dismiss needs a reason.".into()),
            "dismiss" => Ok(Verdict::Dismiss { reason: reason.into() }),
            "escalate" if !is_valid_handle(to) => {
                Err("Escalate needs an @user or @org/team handle.".into())
            }
            "escalate" => Ok(Verdict::Escalate { to: to.into() }),
            "" => Err("Choose a verdict.".into()),
            other => Err(format!("Unknown verdict `{other}`.")),
        }
    }

    pub fn key(&self) -> &'static str {
        match self {
            Verdict::Block => "block",
            Verdict::Comment => "comment",
            Verdict::Nit => "nit",
            Verdict::Dismiss { .. } => "dismiss",
            Verdict::Escalate { .. } => "escalate",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Verdict::Block => "Block",
            Verdict::Comment => "Comment",
            Verdict::Nit => "Nit",
            Verdict::Dismiss { .. } => "Dismiss",
            Verdict::Escalate { .. } => "Escalate",
        }
    }
}

/// `@user` or `@org/team`.
pub fn is_valid_handle(s: &str) -> bool {
    let Some(rest) = s.strip_prefix('@') else { return false };
    let parts: Vec<&str> = rest.split('/').collect();
    parts.len() <= 2
        && parts.iter().all(|p| {
            !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ruling {
    pub verdict: Verdict,
    /// The text to post, as edited by the Reviewer.
    pub comment: String,
}

/// A Finding under Adjudication, with the text prr resolved for it and the Reviewer's ruling.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub proposal: Proposal,
    /// Text of the location's lines on the head commit.
    #[serde(default)]
    pub location_text: Option<String>,
    /// Resolved text of each code excerpt, keyed by evidence index.
    #[serde(default)]
    pub excerpts: BTreeMap<usize, String>,
    #[serde(default)]
    pub ruling: Option<Ruling>,
}

impl Entry {
    pub fn finding(&self) -> Option<&Finding> {
        match &self.proposal {
            Proposal::Agent(f) => Some(f),
            Proposal::Reviewer { .. } => None,
        }
    }

    pub fn origin(&self) -> Origin {
        match &self.proposal {
            Proposal::Agent(_) => Origin::Agent,
            Proposal::Reviewer { .. } => Origin::Reviewer,
        }
    }

    pub fn location(&self) -> &Location {
        match &self.proposal {
            Proposal::Agent(f) => &f.location,
            Proposal::Reviewer { location, .. } => location,
        }
    }

    pub fn title(&self) -> Option<&str> {
        self.finding().map(|f| f.title.as_str())
    }

    pub fn display_title(&self) -> String {
        match &self.proposal {
            Proposal::Agent(f) => f.title.clone(),
            Proposal::Reviewer { comment, .. } => {
                let first: String = comment.lines().next().unwrap_or("").chars().take(60).collect();
                format!("Reviewer: {first}")
            }
        }
    }

    pub fn draft_comment(&self) -> &str {
        match &self.proposal {
            Proposal::Agent(f) => &f.draft_comment,
            Proposal::Reviewer { comment, .. } => comment,
        }
    }

    pub fn severity(&self) -> Option<Severity> {
        self.finding().map(|f| f.severity)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Outcome {
    pub state: Option<ReviewState>,
    pub summary: String,
    pub export_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SessionError {
    #[error("This session is no longer open.")]
    NotOpen,
    #[error("There is no finding #{0}.")]
    NoSuchFinding(usize),
    #[error("{0}")]
    Invalid(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub token: String,
    pub repo_root: PathBuf,
    pub target: Target,
    pub base_sha: String,
    pub head_sha: String,
    pub entries: Vec<Entry>,
    pub status: SessionStatus,
    #[serde(default)]
    pub export_md: Option<PathBuf>,
    #[serde(default)]
    pub outcome: Option<Outcome>,
}

impl Session {
    pub fn new(
        repo_root: PathBuf,
        target: Target,
        base_sha: String,
        head_sha: String,
        mut entries: Vec<Entry>,
        export_md: Option<PathBuf>,
    ) -> Session {
        // Stable sort: most severe first, then most confident; Reviewer findings (no severity) last.
        entries.sort_by_key(|e| {
            let f = e.finding();
            (Reverse(f.map(|f| f.severity)), Reverse(f.map(|f| f.confidence.level)))
        });
        Session {
            id: uuid::Uuid::new_v4().simple().to_string(),
            token: uuid::Uuid::new_v4().simple().to_string(),
            repo_root,
            target,
            base_sha,
            head_sha,
            entries,
            status: SessionStatus::Open,
            export_md,
            outcome: None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.status == SessionStatus::Open
    }

    /// Whether `other` reviews the same change (repo, target and head commit).
    pub fn same_change(&self, other: &Session) -> bool {
        self.repo_root == other.repo_root && self.target == other.target && self.head_sha == other.head_sha
    }

    pub fn unruled(&self) -> Vec<usize> {
        (0..self.entries.len()).filter(|&i| self.entries[i].ruling.is_none()).collect()
    }

    pub fn next_unruled_after(&self, idx: usize) -> Option<usize> {
        let n = self.entries.len();
        (idx + 1..n).chain(0..idx.min(n)).find(|&i| self.entries[i].ruling.is_none())
    }

    pub fn rule(&mut self, idx: usize, ruling: Ruling) -> Result<(), SessionError> {
        if !self.is_open() {
            return Err(SessionError::NotOpen);
        }
        let entry = self.entries.get_mut(idx).ok_or(SessionError::NoSuchFinding(idx))?;
        if ruling.comment.trim().is_empty() && !matches!(ruling.verdict, Verdict::Dismiss { .. }) {
            return Err(SessionError::Invalid("The comment to post must not be empty.".into()));
        }
        entry.ruling = Some(ruling);
        Ok(())
    }

    pub fn add_reviewer_finding(
        &mut self,
        location: Location,
        comment: String,
        location_text: Option<String>,
    ) -> Result<usize, SessionError> {
        if !self.is_open() {
            return Err(SessionError::NotOpen);
        }
        if comment.trim().is_empty() {
            return Err(SessionError::Invalid("Write the comment for your finding.".into()));
        }
        let n = self.entries.iter().filter(|e| e.origin() == Origin::Reviewer).count() + 1;
        self.entries.push(Entry {
            id: format!("{REVIEWER_ID_PREFIX}{n}"),
            proposal: Proposal::Reviewer { location, comment: comment.trim().into() },
            location_text,
            excerpts: BTreeMap::new(),
            ruling: None,
        });
        Ok(self.entries.len() - 1)
    }
}

/// Sessions on disk, one JSON file each.
#[derive(Debug, Clone)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: PathBuf) -> Store {
        Store { dir }
    }

    pub fn from_env() -> Result<Store> {
        if let Some(dir) = std::env::var_os("PRR_STATE_DIR") {
            return Ok(Store::new(dir.into()));
        }
        let base = dirs::state_dir()
            .or_else(dirs::data_local_dir)
            .context("no state directory found; set PRR_STATE_DIR")?;
        Ok(Store::new(base.join("prr")))
    }

    fn sessions_dir(&self) -> PathBuf {
        self.dir.join("sessions")
    }

    fn file(&self, id: &str, ext: &str) -> Result<PathBuf> {
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            bail!("no session `{id}`");
        }
        Ok(self.sessions_dir().join(format!("{id}.{ext}")))
    }

    /// Writes atomically: a crash never leaves a half-written session.
    pub fn save(&self, session: &Session) -> Result<()> {
        fs::create_dir_all(self.sessions_dir())?;
        let path = self.file(&session.id, "json")?;
        let tmp = self.file(&session.id, "json.tmp")?;
        fs::write(&tmp, serde_json::to_vec_pretty(session)?)?;
        fs::rename(&tmp, &path).with_context(|| format!("saving {}", path.display()))?;
        Ok(())
    }

    pub fn load(&self, id: &str) -> Result<Session> {
        let path = self.file(id, "json")?;
        let bytes = fs::read(&path).with_context(|| format!("no session `{id}`"))?;
        serde_json::from_slice(&bytes).with_context(|| format!("{} is corrupt", path.display()))
    }

    pub fn list(&self) -> Result<Vec<Session>> {
        let Ok(dir) = fs::read_dir(self.sessions_dir()) else { return Ok(vec![]) };
        let mut out = Vec::new();
        for entry in dir {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == "json") {
                let parsed = fs::read(&path)
                    .map_err(anyhow::Error::from)
                    .and_then(|b| serde_json::from_slice::<Session>(&b).map_err(Into::into));
                match parsed {
                    Ok(s) => out.push(s),
                    Err(e) => eprintln!("prr: skipping {}: {e:#}", path.display()),
                }
            }
        }
        Ok(out)
    }

    pub fn find_open(&self, like: &Session) -> Result<Option<Session>> {
        Ok(self.list()?.into_iter().find(|s| s.is_open() && s.same_change(like)))
    }

    /// An exclusive lock marking the session as served by this process; `None` if another process holds it.
    /// The lock is released when the returned file is dropped or the process exits.
    pub fn try_lock(&self, id: &str) -> Result<Option<File>> {
        fs::create_dir_all(self.sessions_dir())?;
        let file = OpenOptions::new().create(true).truncate(false).write(true).open(self.file(id, "lock")?)?;
        match file.try_lock() {
            Ok(()) => Ok(Some(file)),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ConfidenceLevel as C, Severity as S};
    use crate::testutil::finding;

    fn entry(id: &str, s: S, c: C) -> Entry {
        Entry {
            id: id.into(),
            proposal: Proposal::Agent(finding(id, s, c)),
            location_text: None,
            excerpts: BTreeMap::new(),
            ruling: None,
        }
    }

    fn session(entries: Vec<Entry>) -> Session {
        Session::new(
            PathBuf::from("/repo"),
            Target::Local { base_ref: "main".into(), head_ref: "feature".into() },
            "b".repeat(40),
            "h".repeat(40),
            entries,
            None,
        )
    }

    fn ruling(verdict: Verdict) -> Ruling {
        Ruling { verdict, comment: "text".into() }
    }

    #[test]
    fn new_orders_by_severity_then_confidence_keeping_input_order() {
        let s = session(vec![
            entry("minor", S::Minor, C::High),
            entry("major-low", S::Major, C::Low),
            entry("major-high", S::Major, C::High),
            entry("major-high-2", S::Major, C::High),
            entry("critical", S::Critical, C::Low),
        ]);
        let ids: Vec<&str> = s.entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["critical", "major-high", "major-high-2", "major-low", "minor"]);
        assert_eq!(s.id.len(), 32);
        assert_ne!(s.id, s.token);
    }

    #[test]
    fn next_unruled_wraps_around() {
        let mut s = session(vec![entry("a", S::Major, C::High), entry("b", S::Minor, C::High), entry("c", S::Info, C::High)]);
        s.rule(1, ruling(Verdict::Nit)).unwrap();
        assert_eq!(s.next_unruled_after(0), Some(2));
        assert_eq!(s.next_unruled_after(2), Some(0));
        assert_eq!(s.unruled(), vec![0, 2]);
        s.rule(0, ruling(Verdict::Block)).unwrap();
        s.rule(2, ruling(Verdict::Comment)).unwrap();
        assert_eq!(s.next_unruled_after(0), None);
    }

    #[test]
    fn rule_validates() {
        let mut s = session(vec![entry("a", S::Major, C::High)]);
        assert_eq!(s.rule(5, ruling(Verdict::Block)), Err(SessionError::NoSuchFinding(5)));
        let empty = Ruling { verdict: Verdict::Comment, comment: "  ".into() };
        assert!(matches!(s.rule(0, empty), Err(SessionError::Invalid(_))));
        let dismissed = Ruling { verdict: Verdict::Dismiss { reason: "false positive".into() }, comment: String::new() };
        assert_eq!(s.rule(0, dismissed), Ok(()));
        s.status = SessionStatus::Submitted;
        assert_eq!(s.rule(0, ruling(Verdict::Block)), Err(SessionError::NotOpen));
    }

    #[test]
    fn verdict_from_form() {
        assert_eq!(Verdict::from_form("block", "", ""), Ok(Verdict::Block));
        assert_eq!(Verdict::from_form("nit", "", ""), Ok(Verdict::Nit));
        assert!(Verdict::from_form("dismiss", " ", "").unwrap_err().contains("reason"));
        assert_eq!(Verdict::from_form("dismiss", " noise ", ""), Ok(Verdict::Dismiss { reason: "noise".into() }));
        assert!(Verdict::from_form("escalate", "", "bob").is_err());
        assert_eq!(Verdict::from_form("escalate", "", "@org/team"), Ok(Verdict::Escalate { to: "@org/team".into() }));
        assert!(Verdict::from_form("", "", "").unwrap_err().contains("Choose"));
        assert!(Verdict::from_form("approve", "", "").is_err());
    }

    #[test]
    fn handles() {
        assert!(is_valid_handle("@alice"));
        assert!(is_valid_handle("@my-org/core_team.v2"));
        assert!(!is_valid_handle("alice"));
        assert!(!is_valid_handle("@"));
        assert!(!is_valid_handle("@a/b/c"));
        assert!(!is_valid_handle("@a b"));
    }

    #[test]
    fn reviewer_findings_get_reserved_ids_and_need_a_comment() {
        let mut s = session(vec![entry("a", S::Major, C::High)]);
        assert!(matches!(s.add_reviewer_finding(Location::PrWide, " ".into(), None), Err(SessionError::Invalid(_))));
        let i = s.add_reviewer_finding(Location::PrWide, "Rename this\nplease".into(), None).unwrap();
        let j = s.add_reviewer_finding(Location::PrWide, "Another".into(), None).unwrap();
        assert_eq!((i, j), (1, 2));
        assert_eq!(s.entries[1].id, "reviewer-1");
        assert_eq!(s.entries[2].id, "reviewer-2");
        assert_eq!(s.entries[1].origin(), Origin::Reviewer);
        assert_eq!(s.entries[1].display_title(), "Reviewer: Rename this");
        assert_eq!(s.entries[1].draft_comment(), "Rename this\nplease");
    }

    #[test]
    fn store_round_trips_and_finds_open_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf());
        let mut s = session(vec![entry("a", S::Major, C::High)]);
        s.rule(0, ruling(Verdict::Escalate { to: "@bob".into() })).unwrap();
        store.save(&s).unwrap();
        assert_eq!(store.load(&s.id).unwrap(), s);
        assert_eq!(store.list().unwrap().len(), 1);

        let same = session(vec![]);
        assert_eq!(store.find_open(&same).unwrap().map(|f| f.id), Some(s.id.clone()));
        s.status = SessionStatus::Submitted;
        store.save(&s).unwrap();
        assert_eq!(store.find_open(&same).unwrap(), None);
    }

    #[test]
    fn store_rejects_path_like_ids() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf());
        assert!(store.load("../etc/passwd").is_err());
        assert!(store.load("").is_err());
    }

    #[test]
    fn lock_is_exclusive() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().to_path_buf());
        let first = store.try_lock("abc").unwrap();
        assert!(first.is_some());
        assert!(store.try_lock("abc").unwrap().is_none());
        drop(first);
        assert!(store.try_lock("abc").unwrap().is_some());
    }
}
