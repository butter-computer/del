//! Background preparation and maintenance, independent of sending a message.

use del_agent::{SessionConfig, SessionManager};
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::JoinHandle;

pub struct Readiness {
    config: watch::Sender<Option<SessionConfig>>,
    preparation: JoinHandle<()>,
    updates: JoinHandle<()>,
}

impl Readiness {
    pub fn new(runtime: &tokio::runtime::Handle, manager: SessionManager) -> Self {
        let (config, mut rx) = watch::channel::<Option<SessionConfig>>(None);
        let manager_for_updates = manager.clone();
        let mut update_rx = rx.clone();
        let preparation = runtime.spawn(async move {
            loop {
                let selected = rx.borrow_and_update().clone();
                let mut retry = Duration::from_secs(1);
                if let Some(config) = selected {
                    if let Err(error) = manager.prepare_session(config).await {
                        eprintln!("[del startup] background preparation failed: {error}");
                        retry = Duration::from_secs(30);
                    }
                } else {
                    manager.clear_prepared_session().await;
                }
                tokio::select! {
                    result = rx.changed() => if result.is_err() { break; },
                    _ = tokio::time::sleep(retry) => {},
                }
            }
        });
        let updates = runtime.spawn(async move {
            loop {
                let selected = update_rx.borrow_and_update().clone();
                let mut wait = Duration::from_secs(30 * 60);
                if let Some(config) = selected {
                    // Replacing a harness under a running chat or terminal can
                    // break it mid-turn. Try again later.
                    if crate::provider_updates::provider_in_use(
                        &manager_for_updates,
                        config.provider,
                    ) {
                        wait = Duration::from_secs(5 * 60);
                    } else if let Err(error) = crate::provider_updates::ensure_provider(
                        config.provider,
                        config.model,
                        |_| {},
                    )
                    .await
                    {
                        // Update failures must not prevent a working CLI from chatting.
                        eprintln!("[del update] background check failed: {error}");
                    }
                }
                tokio::select! {
                    result = update_rx.changed() => if result.is_err() { break; },
                    _ = tokio::time::sleep(wait) => {},
                }
            }
        });
        Self {
            config,
            preparation,
            updates,
        }
    }

    pub fn set_config(&self, config: Option<SessionConfig>) {
        self.config.send_if_modified(|previous| {
            if *previous == config {
                return false;
            }
            *previous = config;
            true
        });
    }

    pub fn stop(&self) {
        self.preparation.abort();
        self.updates.abort();
    }
}

impl Drop for Readiness {
    fn drop(&mut self) {
        self.stop();
    }
}
