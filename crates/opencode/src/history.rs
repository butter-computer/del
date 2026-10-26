//! Read-only native exports for search, without starting an agent turn.
use crate::mapping;
use del_agent::AgentEvent;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};
type Stamp = Vec<(Option<SystemTime>, u64)>;
#[derive(Default)]
pub struct Cache {
    entries: HashMap<(PathBuf, String, Vec<(String, String)>), CachedHistory>,
}
struct CachedHistory {
    stamp: Stamp,
    events: Arc<Vec<AgentEvent>>,
}
fn stamp(environment: &[(String, String)]) -> Stamp {
    let directory = environment.iter().find(|(key, _)| key == "XDG_DATA_HOME").map(|(_, value)| PathBuf::from(value))
        .or_else(|| std::env::var_os("XDG_DATA_HOME").map(PathBuf::from))
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default()
                .join(".local/share")
        })
        .join("opencode");
    ["opencode.db", "opencode.db-wal"]
        .iter()
        .map(|name| {
            std::fs::metadata(directory.join(name))
                .map(|m| (m.modified().ok(), m.len()))
                .unwrap_or((None, 0))
        })
        .collect()
}
impl Cache {
    /// Called only by the search worker, never the UI thread.
    pub fn read(&mut self, cwd: &Path, id: &str) -> std::io::Result<Arc<Vec<AgentEvent>>> {
        self.read_with_environment(cwd, id, &[])
    }
    pub fn read_with_environment(&mut self, cwd: &Path, id: &str, environment: &[(String, String)]) -> std::io::Result<Arc<Vec<AgentEvent>>> {
        let stamp = stamp(environment);
        let key = (cwd.to_owned(), id.to_owned(), environment.to_vec());
        if let Some(cached) = self.entries.get(&key)
            && stamp.iter().any(|(modified, _)| modified.is_some())
            && cached.stamp == stamp
        {
            return Ok(cached.events.clone());
        }
        let binary = del_agent::detect::find_binary("opencode")
            .ok_or_else(|| std::io::Error::other("OpenCode not installed"))?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let output = runtime.block_on(async {
            let mut command = tokio::process::Command::new(binary);
            command.envs(environment.iter().cloned())
                .args(["session", "export", "--standalone", id])
                .current_dir(cwd)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true);
            del_agent::environment::prepare_provider_command(command.as_std_mut());
            #[cfg(unix)]
            command.process_group(0);
            let child = command.spawn()?;
            let pid = child.id();
            let result =
                tokio::time::timeout(std::time::Duration::from_secs(15), child.wait_with_output())
                    .await;
            if result.is_err()
                && let Some(pid) = pid
            {
                // A standalone export owns its private server and must never
                // leave it behind, including when an export times out.
                del_agent::procgroup::signal_group(pid, true);
            }
            result.map_err(|_| std::io::Error::other("OpenCode history export timed out"))?
        })?;
        if !output.status.success() {
            return Err(std::io::Error::other(
                "OpenCode could not export this session",
            ));
        }
        let export: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        let mut mapping = mapping::Stream::default();
        let events: Arc<Vec<_>> = Arc::new(
            export["data"]["messages"]
                .as_array()
                .or(export["messages"].as_array())
                .ok_or_else(|| std::io::Error::other("OpenCode export is missing messages"))?
                .iter()
                .flat_map(|m| mapping.message(m))
                .collect(),
        );
        self.entries.insert(
            key,
            CachedHistory {
                stamp,
                events: events.clone(),
            },
        );
        Ok(events)
    }
}
