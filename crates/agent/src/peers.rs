//! Durable peer mailboxes. Identity and project membership come from Del, never
//! from model-supplied sender fields. A work lease is distinct from a model turn.
use crate::{AgentError, Result, SessionId};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    Queued,
    Delivering,
    Delivered,
    Failed,
    Uncertain,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerMessage {
    pub id: String,
    pub idempotency_key: String,
    pub from: SessionId,
    pub to: SessionId,
    pub reply_to: Option<String>,
    pub text: String,
    pub created_at_ms: u64,
    pub state: DeliveryState,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkStatus {
    Active,
    Complete,
    Paused,
    Waiting,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerWork {
    pub id: String,
    pub status: WorkStatus,
    /// The peer exchange must not replace this session's existing assignment.
    pub retains_original_work: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct State {
    #[serde(default)]
    aliases: BTreeMap<SessionId, SessionId>,
    projects: BTreeMap<SessionId, PathBuf>,
    messages: Vec<PeerMessage>,
    work: BTreeMap<SessionId, PeerWork>,
}

pub struct PeerStore {
    path: PathBuf,
    state: State,
}

fn failure(text: impl Into<String>) -> AgentError {
    AgentError::Backend(text.into())
}

impl PeerStore {
    pub fn open(path: PathBuf) -> Result<Self> {
        let state = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
                failure(format!("Cannot read peer mailbox {}: {e}", path.display()))
            })?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => State::default(),
            Err(e) => return Err(e.into()),
        };
        let mut store = Self { path, state };
        // A send may have reached the provider just before the process stopped.
        // Do not replay it automatically, especially if it requested file edits.
        if store
            .state
            .messages
            .iter()
            .any(|m| m.state == DeliveryState::Delivering)
        {
            store.update(|state| {
                for message in &mut state.messages {
                    if message.state == DeliveryState::Delivering {
                        message.state = DeliveryState::Uncertain;
                        if let Some(work) = state.work.get_mut(&message.to) {
                            work.status = WorkStatus::Paused;
                        }
                        message.error = Some(
                            "Del stopped during delivery. Check the recipient before retrying."
                                .into(),
                        );
                    }
                }
                Ok(())
            })?;
        }
        Ok(store)
    }

    fn update<T>(&mut self, change: impl FnOnce(&mut State) -> Result<T>) -> Result<T> {
        let mut next = self.state.clone();
        let result = change(&mut next)?;
        let parent = self
            .path
            .parent()
            .ok_or_else(|| failure("Peer mailbox needs a directory"))?;
        std::fs::create_dir_all(parent)?;
        let temp = parent.join(format!(".peer-mailbox-{}.tmp", uuid::Uuid::new_v4()));
        let write = (|| -> Result<()> {
            use std::io::Write;
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temp)?;
            file.write_all(&serde_json::to_vec(&next).map_err(|e| failure(e.to_string()))?)?;
            file.sync_all()?;
            std::fs::rename(&temp, &self.path)?;
            std::fs::File::open(parent)?.sync_all()?;
            Ok(())
        })();
        if write.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        write?;
        self.state = next;
        Ok(result)
    }

    /// Only host code calls this, using the same project identity as the sidebar.
    pub fn register(&mut self, session: &SessionId, project: &Path) -> Result<()> {
        if !project.is_absolute() {
            return Err(failure("Project path must be absolute"));
        }
        if self
            .state
            .projects
            .get(session)
            .is_some_and(|p| p == project)
        {
            return Ok(());
        }
        self.update(|state| {
            state
                .projects
                .insert(session.clone(), project.to_path_buf());
            Ok(())
        })
    }

    pub fn resolve(&self, id: &SessionId) -> SessionId {
        self.state
            .aliases
            .get(id)
            .cloned()
            .unwrap_or_else(|| id.clone())
    }

    /// Side chats retain mailbox identity when their provider-neutral snapshot
    /// is restored into a new native conversation (including account changes).
    pub fn rebind(&mut self, from: &SessionId, to: &SessionId) -> Result<()> {
        let from = self.resolve(from);
        if &from == to {
            return Ok(());
        }
        if self.state.projects.contains_key(to) {
            return Err(failure("Cannot replace another peer's mailbox"));
        }
        self.update(|state| {
            if let Some(project) = state.projects.remove(&from) {
                state.projects.insert(to.clone(), project);
            }
            if let Some(work) = state.work.remove(&from) {
                state.work.insert(to.clone(), work);
            }
            for message in &mut state.messages {
                if message.from == from {
                    message.from = to.clone();
                }
                if message.to == from {
                    message.to = to.clone();
                }
            }
            for target in state.aliases.values_mut() {
                if *target == from {
                    *target = to.clone();
                }
            }
            state.aliases.insert(from, to.clone());
            Ok(())
        })
    }

    pub fn project(&self, id: &SessionId) -> Option<&Path> {
        self.state.projects.get(id).map(PathBuf::as_path)
    }

    pub fn authorize(&self, caller: &SessionId, target: &SessionId) -> Result<()> {
        match (
            self.project(&self.resolve(caller)),
            self.project(&self.resolve(target)),
        ) {
            (Some(a), Some(b)) if a == b => Ok(()),
            _ => Err(failure("Peer sessions must belong to the same Del project")),
        }
    }

    pub fn registered_sessions(&self) -> Vec<SessionId> {
        self.state.projects.keys().cloned().collect()
    }

    pub fn peers(&self, caller: &SessionId) -> Vec<SessionId> {
        self.state
            .projects
            .keys()
            .filter(|id| self.authorize(caller, id).is_ok())
            .cloned()
            .collect()
    }

    /// Atomic durable receipt. Retries return the original identity, never
    /// create another request. Conflicting reuse of a key is rejected.
    pub fn send(
        &mut self,
        from: &SessionId,
        to: &SessionId,
        key: &str,
        text: &str,
        reply_to: Option<&str>,
        original_work: bool,
    ) -> Result<PeerMessage> {
        self.authorize(from, to)?;
        if from == to {
            return Err(failure("Message another session, not yourself"));
        }
        if text.trim().is_empty() || text.chars().count() > 20_000 {
            return Err(failure("Peer messages must contain 1–20000 characters"));
        }
        if key.trim().is_empty() || key.len() > 128 {
            return Err(failure("Use a stable idempotency key of 1–128 bytes"));
        }
        if let Some(old) = self
            .state
            .messages
            .iter()
            .find(|m| &m.from == from && m.idempotency_key == key)
        {
            if &old.to != to || old.text != text || old.reply_to.as_deref() != reply_to {
                return Err(failure(
                    "That idempotency key already identifies a different message",
                ));
            }
            return Ok(old.clone());
        }
        if let Some(request) = reply_to {
            let request = self
                .state
                .messages
                .iter()
                .find(|m| m.id == request)
                .ok_or_else(|| failure("Unknown peer request"))?;
            if &request.to != from || &request.from != to || request.reply_to.is_some() {
                return Err(failure(
                    "Only the recipient can reply to the original request",
                ));
            }
            if self
                .state
                .messages
                .iter()
                .any(|m| m.reply_to.as_deref() == Some(request.id.as_str()))
            {
                return Err(failure(
                    "This request already has a reply; use its original idempotency key to retry",
                ));
            }
        }
        let message = PeerMessage {
            id: uuid::Uuid::new_v4().to_string(),
            idempotency_key: key.into(),
            from: from.clone(),
            to: to.clone(),
            reply_to: reply_to.map(str::to_owned),
            text: text.into(),
            created_at_ms: crate::index::now_ms(),
            state: DeliveryState::Queued,
            error: None,
        };
        self.update(|state| {
            state.messages.push(message.clone());
            if reply_to.is_none() {
                activate(state, from, true);
            }
            activate(state, to, original_work);
            Ok(message)
        })
    }

    pub fn message(&self, id: &str) -> Option<PeerMessage> {
        self.state.messages.iter().find(|m| m.id == id).cloned()
    }

    pub fn messages(&self, id: &SessionId) -> Vec<PeerMessage> {
        self.state
            .messages
            .iter()
            .filter(|m| &m.from == id || &m.to == id)
            .cloned()
            .collect()
    }

    pub fn pending(&self, id: &SessionId) -> Option<PeerMessage> {
        // Never leapfrog an ambiguous or failed earlier message silently.
        self.state
            .messages
            .iter()
            .find(|m| {
                &m.to == id
                    && matches!(
                        m.state,
                        DeliveryState::Queued
                            | DeliveryState::Delivering
                            | DeliveryState::Failed
                            | DeliveryState::Uncertain
                    )
            })
            .cloned()
    }

    pub fn claim(&mut self, id: &str) -> Result<PeerMessage> {
        let message = self
            .message(id)
            .ok_or_else(|| failure("Unknown peer message"))?;
        self.authorize(&message.from, &message.to)?;
        if self
            .pending(&message.to)
            .is_none_or(|m| m.id != id || m.state != DeliveryState::Queued)
        {
            return Err(AgentError::Busy(
                "An earlier peer message is pending".into(),
            ));
        }
        self.set_delivery(id, DeliveryState::Delivering, None)
    }

    pub fn set_delivery(
        &mut self,
        id: &str,
        status: DeliveryState,
        error: Option<String>,
    ) -> Result<PeerMessage> {
        self.update(|state| {
            let message = state
                .messages
                .iter_mut()
                .find(|m| m.id == id)
                .ok_or_else(|| failure("Unknown peer message"))?;
            message.state = status;
            message.error = error;
            let result = message.clone();
            if matches!(status, DeliveryState::Cancelled | DeliveryState::Failed) {
                for peer in [&result.from, &result.to] {
                    if let Some(work) = state.work.get_mut(peer)
                        && work.status == WorkStatus::Waiting
                    {
                        work.status = WorkStatus::Active;
                    }
                }
            }
            Ok(result)
        })
    }

    pub fn work(&self, id: &SessionId) -> Option<PeerWork> {
        self.state.work.get(id).cloned()
    }

    pub fn user_queue_blocked(&self, id: &SessionId) -> bool {
        self.pending(id).is_some()
            || self
                .work(id)
                .is_some_and(|w| w.status != WorkStatus::Complete)
    }

    pub fn finish_work(&mut self, id: &SessionId) -> Result<PeerWork> {
        if self.pending(id).is_some() {
            return Err(failure(
                "Handle pending peer messages before completing work",
            ));
        }
        let unanswered = self.state.messages.iter().any(|m| {
            m.reply_to.is_none()
                && (&m.from == id || &m.to == id)
                && !matches!(m.state, DeliveryState::Failed | DeliveryState::Cancelled)
                && !self
                    .state
                    .messages
                    .iter()
                    .any(|r| r.reply_to.as_deref() == Some(m.id.as_str()))
        });
        if unanswered {
            return Err(failure(
                "Reply to incoming requests and receive outstanding replies before completing work",
            ));
        }
        self.update(|state| {
            let work = state
                .work
                .get_mut(id)
                .ok_or_else(|| failure("No peer work is active"))?;
            work.status = WorkStatus::Complete;
            Ok(work.clone())
        })
    }

    pub fn pause(&mut self, id: &SessionId) -> Result<()> {
        if !self.state.work.contains_key(id) {
            return Ok(());
        }
        self.update(|state| {
            state.work.get_mut(id).unwrap().status = WorkStatus::Paused;
            Ok(())
        })
    }

    pub fn wait(&mut self, id: &SessionId) -> Result<()> {
        if self.pending(id).is_some() {
            return Err(failure("Handle queued peer messages before waiting"));
        }
        if !self.state.messages.iter().any(|m| {
            &m.from == id
                && !matches!(m.state, DeliveryState::Cancelled | DeliveryState::Failed)
                && m.reply_to.is_none()
                && !self
                    .state
                    .messages
                    .iter()
                    .any(|r| r.reply_to.as_deref() == Some(&m.id))
        }) {
            return Err(failure("No peer reply is outstanding"));
        }
        self.update(|state| {
            state
                .work
                .get_mut(id)
                .ok_or_else(|| failure("No active work"))?
                .status = WorkStatus::Waiting;
            Ok(())
        })
    }

    pub fn resume(&mut self, id: &SessionId) -> Result<()> {
        if !self.state.work.contains_key(id) {
            return Ok(());
        }
        self.update(|state| {
            state.work.get_mut(id).unwrap().status = WorkStatus::Active;
            Ok(())
        })
    }

    pub fn remove_session(&mut self, id: &SessionId) -> Result<()> {
        self.update(|state| {
            state.projects.remove(id);
            state.work.remove(id);
            for message in &mut state.messages {
                if &message.to == id || &message.from == id {
                    message.state = DeliveryState::Cancelled;
                    message.error = Some("A session in this exchange was deleted".into());
                    let other = if &message.to == id {
                        &message.from
                    } else {
                        &message.to
                    };
                    if let Some(work) = state.work.get_mut(other)
                        && work.status == WorkStatus::Waiting
                    {
                        work.status = WorkStatus::Active;
                    }
                }
            }
            Ok(())
        })
    }
}

fn activate(state: &mut State, id: &SessionId, original: bool) {
    match state.work.get_mut(id) {
        Some(work) if work.status != WorkStatus::Complete => {
            work.retains_original_work |= original;
            if work.status == WorkStatus::Waiting {
                work.status = WorkStatus::Active;
            }
            // A user stop stays stopped, even when another peer sends a message.
        }
        _ => {
            state.work.insert(
                id.clone(),
                PeerWork {
                    id: uuid::Uuid::new_v4().to_string(),
                    status: WorkStatus::Active,
                    retains_original_work: original,
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restored_side_chat_keeps_mailbox_and_cancelled_peer_unblocks_waiter() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("peers.json");
        let (a, b, c) = (
            SessionId::new("a"),
            SessionId::new("b"),
            SessionId::new("c"),
        );
        let restored = SessionId::new("restored-b");
        let mut store = setup(path.clone());
        let request = store
            .send(&a, &b, "request", "change it", None, true)
            .unwrap();
        store.wait(&a).unwrap();
        store.rebind(&b, &restored).unwrap();
        assert_eq!(store.resolve(&b), restored);
        assert_eq!(store.pending(&restored).unwrap().id, request.id);
        store.claim(&request.id).unwrap();
        drop(store);
        let mut store = PeerStore::open(path).unwrap();
        assert_eq!(
            store.pending(&restored).unwrap().state,
            DeliveryState::Uncertain
        );
        assert!(store.user_queue_blocked(&a));
        assert!(store.rebind(&restored, &c).is_err());
        store.remove_session(&restored).unwrap();
        assert_eq!(store.work(&a).unwrap().status, WorkStatus::Active);
        assert!(!store.authorize(&a, &b).is_ok());
        assert!(store.wait(&a).is_err());
        store.finish_work(&a).unwrap();
        assert!(!store.user_queue_blocked(&a));
    }

    fn setup(path: PathBuf) -> PeerStore {
        let mut store = PeerStore::open(path).unwrap();
        for id in ["a", "b", "c"] {
            store
                .register(&SessionId::new(id), Path::new("/project"))
                .unwrap();
        }
        store
            .register(&SessionId::new("outsider"), Path::new("/other"))
            .unwrap();
        store
    }
    fn id(s: &str) -> SessionId {
        SessionId::new(s)
    }
    #[test]
    fn project_scope_and_idempotency_are_enforced_before_delivery() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = setup(dir.path().join("peers.json"));
        assert!(
            store
                .send(&id("a"), &id("outsider"), "k", "hi", None, false)
                .is_err()
        );
        assert!(
            store
                .send(&id("unknown"), &id("b"), "k", "hi", None, false)
                .is_err()
        );
        let message = store
            .send(&id("a"), &id("b"), "k", "hi", None, true)
            .unwrap();
        assert_eq!(
            store
                .send(&id("a"), &id("b"), "k", "hi", None, true)
                .unwrap(),
            message
        );
        assert!(
            store
                .send(&id("a"), &id("b"), "k", "different", None, true)
                .is_err()
        );
        assert_eq!(store.messages(&id("b")).len(), 1);
        assert_eq!(store.peers(&id("a")).len(), 3);
    }
    #[test]
    fn replying_does_not_complete_original_work_or_release_user_queue() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = setup(dir.path().join("peers.json"));
        let request = store
            .send(&id("b"), &id("a"), "q", "check API", None, true)
            .unwrap();
        store.claim(&request.id).unwrap();
        store
            .set_delivery(&request.id, DeliveryState::Delivered, None)
            .unwrap();
        assert!(store.finish_work(&id("a")).is_err());
        assert!(
            store
                .send(&id("c"), &id("b"), "r", "forged", Some(&request.id), false)
                .is_err()
        );
        let reply = store
            .send(&id("a"), &id("b"), "r", "here", Some(&request.id), true)
            .unwrap();
        assert!(store.user_queue_blocked(&id("a")));
        assert!(store.work(&id("a")).unwrap().retains_original_work);
        store.finish_work(&id("a")).unwrap();
        assert!(!store.user_queue_blocked(&id("a")));
        assert!(store.user_queue_blocked(&id("b")));
        store.claim(&reply.id).unwrap();
        store
            .set_delivery(&reply.id, DeliveryState::Delivered, None)
            .unwrap();
        store.finish_work(&id("b")).unwrap();
        assert!(!store.user_queue_blocked(&id("b")));
    }
    #[test]
    fn fifo_and_restart_preserve_queued_work_without_replaying_uncertain_delivery() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("peers.json");
        let mut store = setup(path.clone());
        let first = store
            .send(&id("b"), &id("a"), "first", "one", None, true)
            .unwrap();
        let second = store
            .send(&id("c"), &id("a"), "second", "two", None, true)
            .unwrap();
        assert!(store.claim(&second.id).is_err());
        store.claim(&first.id).unwrap();
        drop(store);
        let mut store = PeerStore::open(path).unwrap();
        assert_eq!(
            store.pending(&id("a")).unwrap().state,
            DeliveryState::Uncertain
        );
        assert!(store.claim(&second.id).is_err());
        assert_eq!(
            store.message(&second.id).unwrap().state,
            DeliveryState::Queued
        );
        assert!(store.work(&id("a")).unwrap().retains_original_work);
        store.pause(&id("a")).unwrap();
        store
            .send(&id("b"), &id("a"), "third", "three", None, false)
            .unwrap();
        assert_eq!(store.work(&id("a")).unwrap().status, WorkStatus::Paused);
    }
    #[test]
    fn unreadable_mailbox_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("peers.json");
        std::fs::write(&path, b"broken").unwrap();
        assert!(PeerStore::open(path.clone()).is_err());
        assert_eq!(std::fs::read(path).unwrap(), b"broken");
    }
}
