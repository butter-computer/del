//! Exclusive ownership while a native provider TUI edits a conversation.
use super::*;

impl SessionManager {
    pub fn is_terminal_owned(&self, id: &SessionId) -> bool {
        self.shared.terminal_owned.lock().unwrap().contains(id)
    }

    pub async fn handoff_to_terminal(&self, id: &SessionId) -> Result<SessionRecord> {
        let lock = self
            .shared
            .resume_locks
            .lock()
            .unwrap()
            .entry(id.clone())
            .or_default()
            .clone();
        let _guard = lock.lock().await;
        if self.is_terminal_owned(id)
            || matches!(
                self.status(id),
                Some(SessionStatus::Running | SessionStatus::WaitingApproval)
            )
        {
            return Err(AgentError::Busy(
                "Finish or stop the current response before switching to Terminal.".into(),
            ));
        }
        if self.is_temporary(id) {
            return Err(AgentError::Backend(
                "Temporary side chats cannot switch to Terminal.".into(),
            ));
        }
        if self.pending_fork_context(id).is_some() {
            return Err(AgentError::Backend(
                "Send the next message in Chat before switching to Terminal so the saved fork context reaches the provider.".into(),
            ));
        }
        let record = self
            .shared
            .index
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| AgentError::UnknownSession(id.clone()))?;
        self.close_locked(id).await?;
        self.shared
            .terminal_owned
            .lock()
            .unwrap()
            .insert(id.clone());
        Ok(record)
    }

    /// Only release after the TUI has exited and its PTY has been drained.
    pub fn release_terminal(&self, id: &SessionId) {
        self.shared.terminal_owned.lock().unwrap().remove(id);
    }

    /// Register an exact native terminal identity without rewriting an existing chat.
    pub fn import_terminal(&self, record: SessionRecord) -> Result<()> {
        let mut index = self.shared.index.lock().unwrap();
        if let Some(existing) = index.get(&record.id) {
            if existing.provider != record.provider
                || existing.account_id != record.account_id
                || existing.cwd != record.cwd
            {
                return Err(AgentError::Backend("This conversation is already saved with different provider, account, or folder settings.".into()));
            }
            return Ok(());
        }
        let id = record.id.clone();
        let cwd = record.cwd.clone();
        index.upsert(record)?;
        drop(index);
        self.set_peer_project(&id, &cwd)?;
        let _ = self.shared.all_tx.send(ManagerEvent::StatusChanged {
            session: id,
            status: SessionStatus::Idle,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::MockBackend;

    #[tokio::test]
    async fn all_providers_keep_identity_config_and_exclusive_ownership() {
        for provider in [Provider::Codex, Provider::Claude, Provider::OpenCode] {
            let dir = tempfile::tempdir().unwrap();
            let backend = Arc::new(MockBackend::new(provider));
            let manager = SessionManager::new(
                dir.path().join("index"),
                [backend.clone() as Arc<dyn AgentBackend>],
            )
            .unwrap();
            manager.register_account("test-account".into(), backend);
            let config = SessionConfig {
                provider,
                cwd: dir.path().into(),
                model: Some("test-model".into()),
                reasoning_effort: Some("high".into()),
                service_tier: None,
                permission_mode: PermissionMode::Plan,
            };
            let handle = manager
                .start_account_session(config.clone(), Some("test-account".into()))
                .await
                .unwrap();
            let record = manager.handoff_to_terminal(&handle.id).await.unwrap();
            assert_eq!(record.id, handle.id);
            assert_eq!(record.config(), config);
            assert_eq!(record.account_id.as_deref(), Some("test-account"));
            assert!(!manager.is_live(&handle.id));
            assert!(manager.is_terminal_owned(&handle.id));
            assert!(matches!(
                manager.resume(&handle.id).await,
                Err(AgentError::Busy(_))
            ));
            assert!(manager.handoff_to_terminal(&handle.id).await.is_err());
            assert!(matches!(
                manager
                    .send_prompt(&handle.id, UserInput::text("must not reach provider"))
                    .await,
                Err(AgentError::Busy(_))
            ));
            // Scheduler ticks during the handoff must leave terminal mail queued.
            let peer = manager
                .start_account_session(config.clone(), Some("test-account".into()))
                .await
                .unwrap();
            let message = manager
                .send_to_peer(
                    &peer.id,
                    &handle.id,
                    "handoff-mail",
                    "Keep this request queued",
                )
                .unwrap();
            assert!(!manager.dispatch_peer(&handle.id).await.unwrap());
            let pending = manager.pending_peer(&handle.id).unwrap();
            assert_eq!(pending.id, message.id);
            assert_eq!(pending.state, crate::peers::DeliveryState::Queued);
            assert!(!manager.is_live(&handle.id));
            manager.release_terminal(&handle.id);
            assert_eq!(manager.resume(&handle.id).await.unwrap().id, handle.id);
            assert_eq!(manager.session_record(&handle.id).unwrap().config(), config);
            // Importing a terminal must preserve existing chat settings and title.
            let mut imported = record.clone();
            imported.model = Some("do not overwrite".into());
            manager.import_terminal(imported).unwrap();
            assert_eq!(manager.session_record(&handle.id).unwrap().config(), config);
            let mut conflicting = record;
            conflicting.account_id = None;
            assert!(manager.import_terminal(conflicting).is_err());
            let fork = manager
                .fork(ForkSource::Session(handle.id.clone()), ForkLifetime::Saved)
                .await
                .unwrap();
            assert!(manager.handoff_to_terminal(&fork.handle.id).await.is_err());
            assert!(manager.is_live(&fork.handle.id));
            assert!(!manager.is_terminal_owned(&fork.handle.id));
            manager.shutdown().await;
        }
    }

    #[tokio::test]
    async fn handoff_rejects_an_active_response_without_closing_it() {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(
            MockBackend::new(Provider::Claude).with_delay(std::time::Duration::from_secs(1)),
        );
        let manager =
            SessionManager::new(dir.path().join("index"), [backend as Arc<dyn AgentBackend>])
                .unwrap();
        let handle = manager
            .start_session(SessionConfig {
                provider: Provider::Claude,
                cwd: dir.path().into(),
                model: None,
                reasoning_effort: None,
                service_tier: None,
                permission_mode: PermissionMode::Ask,
            })
            .await
            .unwrap();
        manager
            .send_prompt(&handle.id, UserInput::text("hello"))
            .await
            .unwrap();
        assert!(matches!(
            manager.handoff_to_terminal(&handle.id).await,
            Err(AgentError::Busy(_))
        ));
        assert!(manager.is_live(&handle.id));
        assert!(!manager.is_terminal_owned(&handle.id));
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn concurrent_send_and_handoff_never_share_native_ownership() {
        for provider in [Provider::Codex, Provider::Claude, Provider::OpenCode] {
            let dir = tempfile::tempdir().unwrap();
            let backend = Arc::new(
                MockBackend::new(provider).with_delay(std::time::Duration::from_millis(50)),
            );
            let manager =
                SessionManager::new(dir.path().join("index"), [backend as Arc<dyn AgentBackend>])
                    .unwrap();
            for send_first in [true, false] {
                let handle = manager
                    .start_session(SessionConfig {
                        provider,
                        cwd: dir.path().into(),
                        model: None,
                        reasoning_effort: None,
                        service_tier: None,
                        permission_mode: PermissionMode::Ask,
                    })
                    .await
                    .unwrap();
                let (sent, handed) = if send_first {
                    tokio::join!(
                        manager.send_prompt(&handle.id, UserInput::text("test response")),
                        manager.handoff_to_terminal(&handle.id),
                    )
                } else {
                    let (handed, sent) = tokio::join!(
                        manager.handoff_to_terminal(&handle.id),
                        manager.send_prompt(&handle.id, UserInput::text("test response")),
                    );
                    (sent, handed)
                };
                assert_ne!(sent.is_ok(), handed.is_ok());
                assert_eq!(manager.is_terminal_owned(&handle.id), handed.is_ok());
                assert_eq!(manager.is_live(&handle.id), sent.is_ok());
                manager.release_terminal(&handle.id);
                manager.close(&handle.id).await.unwrap();
            }
            manager.shutdown().await;
        }
    }
}
