//! Durable follow-up queue. Delivery is acknowledged before removing an item.
use del_agent::{SessionId, UserInput};
use serde::{Deserialize, Serialize};
#[derive(Clone, Serialize, Deserialize)]
pub struct Followup {
    pub id: String,
    pub input: UserInput,
    #[serde(default)]
    pub sending: bool,
    #[serde(default)]
    pub error: Option<String>,
}
#[derive(Default, Serialize, Deserialize)]
pub struct Queue {
    pub items: Vec<Followup>,
    #[serde(skip)]
    storage: Option<std::path::PathBuf>,
}
impl Queue {
    fn path(session: &SessionId) -> Option<std::path::PathBuf> {
        Some(Self::path_in(&del_agent::data_dir()?, session))
    }
    fn path_in(data_dir: &std::path::Path, session: &SessionId) -> std::path::PathBuf {
        let name: String = session.0.bytes().map(|b| format!("{b:02x}")).collect();
        data_dir.join("followups").join(format!("{name}.json"))
    }
    pub fn load(session: &SessionId) -> Self {
        Self::path(session).map(Self::open).unwrap_or_default()
    }
    fn open(path: std::path::PathBuf) -> Self {
        // An unreadable queue is preserved aside rather than overwritten by the next save.
        let (mut queue, _): (Self, _) =
            crate::sessions::load_or_set_aside(&path, "queued follow-ups");
        queue.storage = Some(path);
        for item in &mut queue.items {
            if item.sending {
                item.sending = false;
                item.error = Some("Delivery was interrupted. Review before retrying.".into());
            } else if item
                .error
                .as_deref()
                .is_some_and(|error| error.starts_with("session is busy:"))
            {
                // Older builds treated a rejected, undelivered follow-up as a
                // permanent failure. It is safe to put it back in the queue.
                item.error = None;
            }
        }
        queue
    }
    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = &self.storage else {
            return Ok(());
        };
        crate::sessions::write_atomic(path, &serde_json::to_vec(self)?, crate::sessions::PRIVATE)
    }
    /// Delete a session's queue file and the imported attachment files it still
    /// references. Returns the attachment paths that were removed.
    pub fn remove_session(session: &SessionId) -> Vec<std::path::PathBuf> {
        del_agent::data_dir()
            .map(|d| Self::remove_session_in(&d, session))
            .unwrap_or_default()
    }
    fn remove_session_in(
        data_dir: &std::path::Path,
        session: &SessionId,
    ) -> Vec<std::path::PathBuf> {
        let path = Self::path_in(data_dir, session);
        let attachments = data_dir.join("attachments");
        let queue: Option<Self> = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        let mut removed = Vec::new();
        for file in queue
            .iter()
            .flat_map(|q| q.items.iter())
            .flat_map(|item| item.input.attachments.iter())
        {
            if let Some(entry) = attachment_entry(&attachments, &file.path) {
                let _ = if entry.is_dir() {
                    std::fs::remove_dir_all(&entry)
                } else {
                    std::fs::remove_file(&entry)
                };
                removed.push(entry);
            }
        }
        let _ = std::fs::remove_file(path);
        removed
    }
}

/// The top-level entry under the attachments directory that holds this file:
/// `attachments/<uuid>/<name>` imports map to their directory, clipboard images to the file.
fn attachment_entry(
    attachments: &std::path::Path,
    file: &std::path::Path,
) -> Option<std::path::PathBuf> {
    let relative = file.strip_prefix(attachments).ok()?;
    // Only a plain name directly inside the store: never `..` or a root.
    let std::path::Component::Normal(first) = relative.components().next()? else {
        return None;
    };
    Some(attachments.join(first))
}

impl Queue {
    pub fn push(&mut self, input: UserInput) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        self.items.push(Followup {
            id: id.clone(),
            input,
            sending: false,
            error: None,
        });
        id
    }
    pub fn take(&mut self, id: &str) -> Option<Followup> {
        let index = self
            .items
            .iter()
            .position(|item| item.id == id && !item.sending)?;
        Some(self.items.remove(index))
    }
    pub fn reorder(&mut self, from: &str, to: &str) {
        if from == to {
            return;
        }
        let Some(index) = self.items.iter().position(|item| item.id == to) else {
            return;
        };
        if let Some(item) = self.take(from) {
            self.items.insert(index.min(self.items.len()), item);
        }
    }
    pub fn begin(&mut self, id: &str) -> Option<UserInput> {
        if self.items.iter().any(|item| item.sending) {
            return None;
        }
        let item = self.items.iter_mut().find(|item| item.id == id)?;
        item.sending = true;
        item.error = None;
        Some(item.input.clone())
    }
    pub fn finish(&mut self, id: &str, error: Option<String>) {
        if let Some(error) = error {
            if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
                item.sending = false;
                item.error = Some(error);
            }
        } else {
            self.items.retain(|item| item.id != id);
        }
    }
    pub fn wait(&mut self, id: &str) {
        if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
            item.sending = false;
            item.error = None;
        }
    }
    pub fn next(&self) -> Option<String> {
        self.items
            .first()
            .filter(|item| !item.sending && item.error.is_none())
            .map(|item| item.id.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reorder_edit_failure_and_acknowledgement_preserve_attachments() {
        let mut queue = Queue::default();
        let first = queue.push(UserInput::text("first"));
        let input = UserInput {
            command: None,
            skills: Vec::new(),
            text: "picture".into(),
            attachments: vec![
                del_agent::Attachment {
                    path: "/tmp/picture.png".into(),
                    mime: Some("image/png".into()),
                },
                del_agent::Attachment {
                    path: "/tmp/report.pdf".into(),
                    mime: Some("application/pdf".into()),
                },
                del_agent::Attachment {
                    path: "/tmp/book.xlsx".into(),
                    mime: Some(
                        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".into(),
                    ),
                },
            ],
        };
        let second = queue.push(input.clone());
        queue.reorder(&second, &first);
        assert_eq!(queue.next(), Some(second.clone()));
        assert_eq!(queue.begin(&second), Some(input.clone()));
        assert!(queue.begin(&first).is_none());
        assert!(queue.take(&second).is_none());
        queue.finish(&second, Some("offline".into()));
        assert!(queue.next().is_none());
        assert_eq!(queue.take(&second).unwrap().input, input);
        assert_eq!(queue.next(), Some(first.clone()));
        queue.begin(&first);
        queue.finish(&first, None);
        assert!(queue.items.is_empty());
    }
    #[test]
    fn restart_preserves_order_and_never_replays_unacknowledged_delivery() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("queue.json");
        let mut queue = Queue::open(path.clone());
        let first = queue.push(UserInput::text("first"));
        queue.push(UserInput::text("second"));
        queue.begin(&first);
        queue.save().unwrap();
        let reopened = Queue::open(path);
        assert_eq!(reopened.items.len(), 2);
        assert_eq!(reopened.items[0].id, first);
        assert!(reopened.items[0].error.is_some());
        assert!(reopened.next().is_none());
    }

    #[test]
    fn deleting_a_session_removes_its_queue_and_its_attachments_only() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path();
        let attachments = data.join("attachments");
        let referenced = attachments.join("ref-dir");
        let clipboard = attachments.join("clipboard-1.png");
        let other = attachments.join("other-dir");
        for d in [&referenced, &other] {
            std::fs::create_dir_all(d).unwrap();
            std::fs::write(d.join("file.txt"), "x").unwrap();
        }
        std::fs::write(&clipboard, "png").unwrap();
        let outside = data.join("outside.txt");
        std::fs::write(&outside, "keep").unwrap();
        let escaping = attachments.join("../outside.txt");
        let session = SessionId::new("s1");
        let path = Queue::path_in(data, &session);
        let mut queue = Queue::open(path.clone());
        queue.push(UserInput {
            command: None,
            skills: Vec::new(),
            text: "with files".into(),
            attachments: vec![
                del_agent::Attachment {
                    path: referenced.join("file.txt"),
                    mime: None,
                },
                del_agent::Attachment {
                    path: clipboard.clone(),
                    mime: None,
                },
                del_agent::Attachment {
                    path: outside.clone(),
                    mime: None,
                },
                del_agent::Attachment {
                    path: escaping,
                    mime: None,
                },
            ],
        });
        queue.save().unwrap();
        let removed = Queue::remove_session_in(data, &session);
        assert_eq!(removed.len(), 2);
        assert!(!path.exists() && !referenced.exists() && !clipboard.exists());
        // Files outside Del's attachment store and other imports are untouched.
        assert!(outside.exists() && other.exists());
    }
    #[test]
    fn reopening_retries_known_busy_rejections_but_retains_other_failures() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("queue.json");
        let mut queue = Queue::open(path.clone());
        let busy = queue.push(UserInput::text("waiting"));
        let failed = queue.push(UserInput::text("offline"));
        queue.finish(
            &busy,
            Some("session is busy: a turn is already running".into()),
        );
        queue.finish(&failed, Some("connection lost".into()));
        queue.save().unwrap();
        let reopened = Queue::open(path);
        assert_eq!(reopened.next(), Some(busy));
        assert_eq!(reopened.items[1].error.as_deref(), Some("connection lost"));
    }

    #[test]
    fn unreadable_queue_is_preserved_instead_of_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("queue.json");
        std::fs::write(&path, "{not json").unwrap();
        let mut queue = Queue::open(path.clone());
        assert!(queue.items.is_empty());
        queue.push(UserInput::text("new"));
        queue.save().unwrap();
        let aside: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.to_string_lossy().contains(".corrupt-"))
            .collect();
        assert_eq!(aside.len(), 1);
        assert_eq!(std::fs::read(&aside[0]).unwrap(), b"{not json");
    }
}
