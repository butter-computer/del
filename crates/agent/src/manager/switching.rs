use super::*;
use crate::conversation::{self, Journal, Segment};

pub(super) fn namespace_event(event: &mut AgentEvent, native: &SessionId) {
    match event {
        AgentEvent::TurnStarted { turn_id }
        | AgentEvent::AssistantTextDelta { turn_id, .. }
        | AgentEvent::AssistantMessage { turn_id, .. }
        | AgentEvent::ReasoningDelta { turn_id, .. }
        | AgentEvent::TurnCompleted { turn_id, .. } => *turn_id = format!("{native}/{turn_id}"),
        AgentEvent::ToolCallStarted { id, .. }
        | AgentEvent::ToolCallUpdated { id, .. }
        | AgentEvent::ToolCallFinished { id, .. } => *id = format!("{native}/{id}"),
        _ => {}
    }
}

impl SessionManager {
    /// Every successful handoff creates a fresh native session. The stable Del
    /// identity and its fanout survive; publishing the index is the commit point.
    pub async fn switch_provider(
        &self,
        id: &SessionId,
        provider: Provider,
        account: Option<String>,
    ) -> Result<()> {
        self.resume(id).await?;
        let lock = self
            .shared
            .resume_locks
            .lock()
            .unwrap()
            .entry(id.clone())
            .or_default()
            .clone();
        let _switching = lock.lock().await;
        let mut record = self.session_record(id)?;
        if provider == record.provider {
            return Err(AgentError::Backend(
                "Use account switching within the current provider".into(),
            ));
        }
        if !matches!(
            provider,
            Provider::Codex | Provider::Claude | Provider::OpenCode
        ) || self.is_temporary(id)
        {
            return Err(AgentError::Backend(
                "Provider switching is available in main Codex, Claude and OpenCode chats".into(),
            ));
        }
        if self.status(id) != Some(SessionStatus::Idle) {
            return Err(AgentError::Busy(
                "Finish or stop the response before switching providers".into(),
            ));
        }
        let old = self.session_backend(id)?;
        if record.provider != Provider::OpenCode
            && let Some(goal) = old.agent_goal(id, None).await?
            && goal.status != crate::GoalStatus::Complete
        {
            return Err(AgentError::Busy("Clear the native goal before switching providers; goals cannot move between providers".into()));
        }
        let backend = self.account_backend(provider, account.as_deref())?;
        let original_config = record.config();
        let config = SessionConfig {
            provider,
            model: None,
            reasoning_effort: None,
            service_tier: None,
            ..original_config.clone()
        };
        if record.segments.is_empty() {
            record.segments.push(Segment {
                session_id: id.clone(),
                provider: record.provider,
                account_id: record.account_id.clone(),
                started_at_ms: record.created_at_ms,
                history: None,
                context: None,
                journal: None,
            });
        }
        let root = self
            .shared
            .index
            .lock()
            .unwrap()
            .path()
            .with_extension("conversations")
            .join(uuid::Uuid::new_v4().to_string());
        let context_path = root.join("context.txt");
        let history = self.subscribe_with_history(id)?.0;
        let context = conversation::context(id, &record.segments, &history, 100_000)?;
        conversation::write_private(&context_path, context.as_bytes())?;
        let mut handle = backend
            .start_session_with_context(config.clone(), &context_path)
            .await?;
        let native = handle.id.clone();
        if record.segments.iter().any(|s| s.session_id == native) {
            let _ = backend.close(&native).await;
            return Err(AgentError::Backend(
                "The provider did not create a fresh session".into(),
            ));
        }
        let marker = AgentEvent::ProviderSwitched {
            from: record.provider,
            to: provider,
            session_id: native.clone(),
            at_ms: now_ms(),
        };
        if let Err(error) = old.close(id).await {
            let _ = backend.close(&native).await;
            match old.resume(original_config.clone(), id).await {
                Ok(handle) => {
                    self.attach(handle, original_config.clone())?;
                }
                Err(restore) => {
                    self.shared.set_status(id, SessionStatus::Error);
                    return Err(AgentError::Backend(format!(
                        "{error}; could not reconnect the original provider: {restore}"
                    )));
                }
            }
            return Err(error);
        }
        let prepare = (|| -> Result<()> {
            // Capture again after target preparation to retain peer receipts
            // that arrived during setup. The context tool reads this full log.
            let mut sessions = self.shared.live.lock().unwrap();
            let live = sessions
                .get_mut(id)
                .ok_or_else(|| AgentError::UnknownSession(id.clone()))?;
            let mut history = live.history.clone();
            let history_path = root.join("previous.json");
            conversation::write_private(
                &history_path,
                &serde_json::to_vec(&history).map_err(|e| AgentError::Backend(e.to_string()))?,
            )?;
            history.push(marker.clone());
            let journal = root.join("events.jsonl");
            let log = Journal::create(&journal, &history)?;
            record.segments.push(Segment {
                session_id: native.clone(),
                provider,
                account_id: account.clone(),
                started_at_ms: now_ms(),
                history: Some(history_path),
                context: Some(context_path),
                journal: Some(journal),
            });
            record.provider = provider;
            record.account_id = account;
            record.model = None;
            record.reasoning_effort = None;
            record.service_tier = None;
            record.updated_at_ms = now_ms();
            let mut index = self.shared.index.lock().unwrap();
            let current = index
                .get(id)
                .ok_or_else(|| AgentError::UnknownSession(id.clone()))?;
            // A manual rename or background title result may arrive while the
            // target starts. Those belong to the chat, not the old execution.
            record.title = current.title.clone();
            record.title_turns = current.title_turns;
            index.replace_durable(record.clone())?;
            live.pump.abort();
            live.generation = uuid::Uuid::new_v4();
            live.journal = Some(log);
            live.history.push(marker.clone());
            let _ = live.fanout.send(marker.clone());
            Ok(())
        })();
        if let Err(error) = prepare {
            let _ = backend.close(&native).await;
            let restored = match old.resume(original_config.clone(), id).await {
                Ok(handle) => handle,
                Err(restore) => {
                    self.shared.set_status(id, SessionStatus::Error);
                    return Err(AgentError::Backend(format!(
                        "{error}; could not reconnect the original provider: {restore}"
                    )));
                }
            };
            self.attach(restored, original_config)?;
            return Err(error);
        }
        handle.id = id.clone();
        self.attach(handle, config)?;
        let _ = self.shared.all_tx.send(ManagerEvent::Event {
            session: id.clone(),
            event: marker,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(provider: Provider) -> SessionConfig {
        SessionConfig {
            provider,
            cwd: std::env::temp_dir(),
            model: None,
            reasoning_effort: None,
            service_tier: None,
            permission_mode: PermissionMode::Ask,
        }
    }
    fn manager(path: &Path) -> (SessionManager, Vec<crate::MockBackend>) {
        let backends: Vec<_> = [Provider::Codex, Provider::Claude, Provider::OpenCode]
            .into_iter()
            .map(crate::MockBackend::new)
            .collect();
        (
            SessionManager::new(
                path,
                backends
                    .iter()
                    .cloned()
                    .map(|b| Arc::new(b) as Arc<dyn AgentBackend>),
            )
            .unwrap(),
            backends,
        )
    }
    async fn send(manager: &SessionManager, id: &SessionId, text: &str) {
        let mut events = manager.subscribe_all();
        manager
            .send_prompt(id, UserInput::text(text))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if matches!(events.recv().await.unwrap(), ManagerEvent::Event { session, event: AgentEvent::TurnCompleted { .. } } if session == *id) { break; }
            }
        }).await.unwrap();
    }

    #[tokio::test]
    async fn all_six_directions_keep_chat_history_subscribers_and_unmodified_prompts() {
        for from in [Provider::Codex, Provider::Claude, Provider::OpenCode] {
            for to in [Provider::Codex, Provider::Claude, Provider::OpenCode] {
                if from == to {
                    continue;
                }
                let dir = tempfile::tempdir().unwrap();
                let (manager, backends) = manager(&dir.path().join("index.jsonl"));
                let chat = manager.start_session(config(from)).await.unwrap().id;
                send(&manager, &chat, "remember the original tool decision").await;
                let (before, mut subscription) = manager.subscribe_with_history(&chat).unwrap();
                manager.switch_provider(&chat, to, None).await.unwrap();
                let record = manager.session_record(&chat).unwrap();
                assert_eq!(record.provider, to);
                assert_ne!(record.native_id(), &chat);
                assert_eq!(manager.list_sessions().len(), 1);
                assert_eq!(record.cwd, config(from).cwd);
                let after = manager.subscribe_with_history(&chat).unwrap().0;
                assert_eq!(&after[..before.len()], &before);
                assert!(
                    matches!(after.last(), Some(AgentEvent::ProviderSwitched { from: a, to: b, .. }) if *a == from && *b == to)
                );
                assert!(matches!(
                    subscription.recv().await.unwrap(),
                    AgentEvent::ProviderSwitched { .. }
                ));
                let target = backends.iter().find(|b| b.provider() == to).unwrap();
                assert!(
                    target.submitted.lock().unwrap().is_empty(),
                    "switch must not infer"
                );
                let context = target.attached_contexts.lock().unwrap()[record.native_id()].clone();
                assert!(context.contains("original tool decision"));
                assert!(
                    !context.contains("Thinking it through"),
                    "private reasoning must not transfer"
                );
                send(&manager, &chat, "next message exactly").await;
                let submitted = target.submitted.lock().unwrap().clone();
                assert_eq!(
                    submitted,
                    vec![(
                        record.native_id().clone(),
                        UserInput::text("next message exactly")
                    )]
                );
                manager.shutdown().await;
            }
        }
    }

    #[tokio::test]
    async fn repeated_switches_and_restart_before_first_target_turn_preserve_all_segments() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.jsonl");
        let (manager, _) = manager(&path);
        let chat = manager
            .start_session(config(Provider::Codex))
            .await
            .unwrap()
            .id;
        let mut ids = HashSet::from([chat.clone()]);
        send(&manager, &chat, "original fact").await;
        for index in 0..24 {
            let provider = [Provider::Claude, Provider::OpenCode, Provider::Codex][index % 3];
            manager
                .switch_provider(&chat, provider, None)
                .await
                .unwrap();
            assert!(ids.insert(manager.session_record(&chat).unwrap().native_id().clone()));
            if index < 23 {
                send(&manager, &chat, &format!("tool step {index}")).await;
            }
        }
        let before = manager.subscribe_with_history(&chat).unwrap().0;
        let active = manager.session_record(&chat).unwrap();
        manager.shutdown().await;
        drop(manager);
        let (restored, _) = self::manager(&path);
        assert!(!restored.is_live(&chat));
        let page = restored
            .read_peer_context(&chat, &chat, None, 64000)
            .await
            .unwrap();
        assert!(page.chunks.iter().any(|c| c.text.contains("original fact")));
        assert!(
            !restored.is_live(&chat),
            "history reads must not wake a provider"
        );
        restored.resume(&chat).await.unwrap();
        assert_eq!(
            restored.session_record(&chat).unwrap().native_id(),
            active.native_id()
        );
        assert_eq!(restored.subscribe_with_history(&chat).unwrap().0, before);
        send(&restored, &chat, "after restart").await;
        assert_eq!(restored.list_sessions().len(), 1);
        assert_eq!(restored.session_record(&chat).unwrap().segments.len(), 25);
        restored.shutdown().await;
    }

    #[tokio::test]
    async fn long_history_is_saved_in_full_and_omitted_facts_remain_readable() {
        let dir = tempfile::tempdir().unwrap();
        let (manager, backends) = manager(&dir.path().join("index.jsonl"));
        let chat = manager
            .start_session(config(Provider::Codex))
            .await
            .unwrap()
            .id;
        {
            let mut live = manager.shared.live.lock().unwrap();
            let history = &mut live.get_mut(&chat).unwrap().history;
            history.push(AgentEvent::UserMessage {
                text: "EARLIEST_FACT_雪_123".into(),
            });
            history.extend((0..60_000).map(|i| AgentEvent::AssistantMessage {
                turn_id: i.to_string(),
                text: format!("Recorded public detail number {i} 🦀"),
            }));
            history.push(AgentEvent::ReasoningDelta {
                turn_id: "private".into(),
                text: "PRIVATE_REASONING".into(),
            });
        }
        let before = manager.subscribe_with_history(&chat).unwrap().0;
        manager
            .switch_provider(&chat, Provider::Claude, None)
            .await
            .unwrap();
        let record = manager.session_record(&chat).unwrap();
        let context = backends[1].attached_contexts.lock().unwrap()[record.native_id()].clone();
        assert!(!context.contains("EARLIEST_FACT"));
        assert!(!context.contains("PRIVATE_REASONING"));
        assert!(context.contains("omitted"));
        assert_eq!(
            &manager.subscribe_with_history(&chat).unwrap().0[..before.len()],
            &before
        );
        let page = manager
            .read_peer_context(&chat, &chat, None, 1000)
            .await
            .unwrap();
        assert!(page.total_events > 60_000);
        assert!(page.chunks.iter().any(|c| c.text.contains("EARLIEST_FACT")));
        assert!(page.next_cursor.is_some());
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn failed_context_attachment_and_busy_switch_leave_source_usable() {
        let dir = tempfile::tempdir().unwrap();
        let source = crate::MockBackend::new(Provider::Codex);
        let mut target = crate::MockBackend::new(Provider::Claude);
        target.fail_context = true;
        let manager = SessionManager::new(
            dir.path().join("i.jsonl"),
            [Arc::new(source) as Arc<dyn AgentBackend>, Arc::new(target)],
        )
        .unwrap();
        let chat = manager
            .start_session(config(Provider::Codex))
            .await
            .unwrap()
            .id;
        let before = manager.subscribe_with_history(&chat).unwrap().0;
        assert!(
            manager
                .switch_provider(&chat, Provider::Claude, None)
                .await
                .is_err()
        );
        assert_eq!(
            manager.session_record(&chat).unwrap().provider,
            Provider::Codex
        );
        assert_eq!(manager.subscribe_with_history(&chat).unwrap().0, before);
        manager
            .shared
            .set_status(&chat, SessionStatus::WaitingApproval);
        assert!(matches!(
            manager.switch_provider(&chat, Provider::Claude, None).await,
            Err(AgentError::Busy(_))
        ));
        manager.shared.set_status(&chat, SessionStatus::Idle);
        send(&manager, &chat, "source still works").await;
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn account_transfer_after_provider_switch_uses_native_identity_and_preserves_context() {
        let dir = tempfile::tempdir().unwrap();
        let (manager, _) = manager(&dir.path().join("index.jsonl"));
        let chat = manager
            .start_session(config(Provider::Codex))
            .await
            .unwrap()
            .id;
        send(&manager, &chat, "before provider switch").await;
        manager
            .switch_provider(&chat, Provider::Claude, None)
            .await
            .unwrap();
        let native = manager.session_record(&chat).unwrap().native_id().clone();
        let target = crate::MockBackend::new(Provider::Claude);
        let submitted = target.submitted.clone();
        manager.register_account("second".into(), Arc::new(target));
        let expected = native.clone();
        manager
            .switch_account(&chat, Some("second".into()), move |record| async move {
                assert_eq!(record.id, expected);
                Ok(None)
            })
            .await
            .unwrap();
        assert_eq!(manager.session_record(&chat).unwrap().native_id(), &native);
        assert_eq!(
            manager
                .session_record(&chat)
                .unwrap()
                .segments
                .last()
                .unwrap()
                .account_id
                .as_deref(),
            Some("second")
        );
        send(&manager, &chat, "after account transfer").await;
        assert_eq!(
            submitted.lock().unwrap()[0],
            (native, UserInput::text("after account transfer"))
        );
        assert_eq!(manager.list_sessions().len(), 1);
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn disk_commit_failure_restores_the_original_execution() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.jsonl");
        let backup = dir.path().join("backup.jsonl");
        let (manager, _) = manager(&path);
        let chat = manager
            .start_session(config(Provider::Codex))
            .await
            .unwrap()
            .id;
        send(&manager, &chat, "source fact").await;
        manager.flush_index().await;
        std::fs::rename(&path, &backup).unwrap();
        std::fs::create_dir(&path).unwrap();
        let before = manager.subscribe_with_history(&chat).unwrap().0;
        assert!(
            manager
                .switch_provider(&chat, Provider::Claude, None)
                .await
                .is_err()
        );
        assert_eq!(
            manager.session_record(&chat).unwrap().provider,
            Provider::Codex
        );
        assert_eq!(manager.subscribe_with_history(&chat).unwrap().0, before);
        std::fs::remove_dir(&path).unwrap();
        std::fs::rename(&backup, &path).unwrap();
        send(&manager, &chat, "source recovered").await;
        manager.shutdown().await;
    }

    #[tokio::test]
    async fn followup_waiting_during_context_setup_runs_once_on_the_target() {
        let dir = tempfile::tempdir().unwrap();
        let source = crate::MockBackend::new(Provider::Codex);
        let source_submitted = source.submitted.clone();
        let mut target = crate::MockBackend::new(Provider::Claude);
        let entered = target.context_started.clone();
        let release = Arc::new(tokio::sync::Notify::new());
        target.context_release = Some(release.clone());
        let target_submitted = target.submitted.clone();
        let manager = SessionManager::new(
            dir.path().join("i.jsonl"),
            [Arc::new(source) as Arc<dyn AgentBackend>, Arc::new(target)],
        )
        .unwrap();
        let chat = manager
            .start_session(config(Provider::Codex))
            .await
            .unwrap()
            .id;
        let switching = {
            let manager = manager.clone();
            let chat = chat.clone();
            tokio::spawn(
                async move { manager.switch_provider(&chat, Provider::Claude, None).await },
            )
        };
        entered.notified().await;
        manager
            .shared
            .index
            .lock()
            .unwrap()
            .update(&chat, |record| {
                record.title = Some("Renamed during switch".into())
            })
            .unwrap();
        let followup = {
            let manager = manager.clone();
            let chat = chat.clone();
            tokio::spawn(async move { send(&manager, &chat, "queued unchanged").await })
        };
        release.notify_one();
        switching.await.unwrap().unwrap();
        followup.await.unwrap();
        assert_eq!(
            manager.session_record(&chat).unwrap().title.as_deref(),
            Some("Renamed during switch")
        );
        assert!(source_submitted.lock().unwrap().is_empty());
        assert_eq!(
            target_submitted.lock().unwrap().as_slice(),
            &[(
                manager.session_record(&chat).unwrap().native_id().clone(),
                UserInput::text("queued unchanged")
            )]
        );
        manager.shutdown().await;
    }
}
