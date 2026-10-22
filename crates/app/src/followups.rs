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
        let name: String = session.0.bytes().map(|b| format!("{b:02x}")).collect();
        Some(
            del_agent::data_dir()?
                .join("followups")
                .join(format!("{name}.json")),
        )
    }
    pub fn load(session: &SessionId) -> Self {
        Self::path(session).map(Self::open).unwrap_or_default()
    }
    fn open(path: std::path::PathBuf) -> Self {
        let mut queue: Self = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
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
        std::fs::create_dir_all(path.parent().unwrap())?;
        let temporary = path.with_extension("tmp");
        std::fs::write(&temporary, serde_json::to_vec(self)?)?;
        std::fs::rename(temporary, path)
    }
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
            text: "picture".into(),
            attachments: vec![del_agent::Attachment {
                path: "/tmp/picture.png".into(),
                mime: Some("image/png".into()),
            }],
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
}
