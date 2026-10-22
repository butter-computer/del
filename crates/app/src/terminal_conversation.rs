//! Bind terminal entries to provider conversation IDs, never to “the latest” chat.
use crate::terminal_store::TerminalRecord;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

/// Provider progress is not a conversation name. Ignore only the standalone
/// status, allowing real titles such as “Fix renaming bugs”.
pub fn title_is_pending(title: &str) -> bool {
    let words: String = title.chars().filter(|c| c.is_alphanumeric()).collect();
    words.eq_ignore_ascii_case("renaming")
}

/// Claude decorates OSC window titles with a status/logo glyph. The sidebar
/// already supplies the provider icon, so retain only the title itself.
pub fn claude_osc_title(title: &str) -> &str {
    let title = title.trim();
    if let Some(rest) = title.strip_prefix(['✳', '✻', '✽', '✶', '✢', '✴', '⏺']) {
        let rest = rest.trim_start_matches('\u{fe0f}');
        if rest.starts_with(char::is_whitespace) && !rest.trim().is_empty() {
            return rest.trim();
        }
    }
    title
}

pub fn codex_root() -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".codex"))
}

/// Codex's OSC title truncates its session-id item to 29 characters plus `...`.
/// Keep that identity separate from the user-visible provider title.
pub fn codex_title(title: &str) -> Option<(String, Option<String>)> {
    let end = title
        .find(|c: char| !(c.is_ascii_hexdigit() || c == '-'))
        .unwrap_or(title.len());
    let prefix = &title[..end];
    if !matches!(prefix.len(), 29 | 36)
        || !prefix.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
    {
        return None;
    }
    let name = title[end..].trim_start_matches(|c: char| {
        c.is_whitespace() || matches!(c, '.' | '…' | '·' | '|' | '—' | '–' | '-')
    });
    let name = name
        .chars()
        .any(char::is_alphanumeric)
        .then(|| name.to_owned());
    Some((prefix.to_owned(), name))
}

fn rollouts(root: &Path) -> Vec<PathBuf> {
    // Only Codex's year/month/day hierarchy; never recursively traverse a workspace.
    let mut dirs = vec![root.join("sessions")];
    for _ in 0..3 {
        dirs = dirs
            .into_iter()
            .flat_map(|p| std::fs::read_dir(p).into_iter().flatten())
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.path())
            .collect();
    }
    dirs.into_iter()
        .flat_map(|p| std::fs::read_dir(p).into_iter().flatten())
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.path())
        .collect()
}

fn rollout_id(path: &Path) -> Option<&str> {
    let stem = path.file_stem()?.to_str()?;
    let id = stem.get(stem.len().checked_sub(36)?..)?;
    uuid::Uuid::parse_str(id).ok()?;
    Some(id)
}

pub fn resolve_codex_prefix(root: &Path, prefix: &str) -> Option<String> {
    if prefix.len() == 36 {
        return uuid::Uuid::parse_str(prefix).ok().map(|id| id.to_string());
    }
    codex_title(prefix)?;
    let mut matches = rollouts(root)
        .iter()
        .filter_map(|path| rollout_id(path))
        .filter(|id| id.starts_with(prefix))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    matches.sort();
    matches.dedup();
    (matches.len() == 1).then(|| matches.remove(0))
}

fn started_ms(record: &TerminalRecord) -> Option<u64> {
    record
        .id
        .0
        .rsplit('-')
        .next()?
        .parse::<u64>()
        .ok()
        .map(|ns| ns / 1_000_000)
}

/// Older builds stored no provider ID. Recover only a unique CLI session created
/// in this launch's short time window, in this exact workspace. Ambiguity is an error.
pub fn recover_legacy_codex(
    root: &Path,
    record: &TerminalRecord,
    records: &[TerminalRecord],
) -> Option<String> {
    let start = started_ms(record)?;
    let end = records
        .iter()
        .filter(|r| r.launch.cwd == record.launch.cwd)
        .filter_map(started_ms)
        .filter(|time| *time > start)
        .min()
        .unwrap_or(start + 30_000)
        .min(start + 30_000);
    let mut matches = Vec::new();
    for path in rollouts(root) {
        let Some(id) = rollout_id(&path) else {
            continue;
        };
        let Ok(uuid) = uuid::Uuid::parse_str(id) else {
            continue;
        };
        let Some(timestamp) = uuid.get_timestamp() else {
            continue;
        };
        let (sec, nanos) = timestamp.to_unix();
        let time = sec * 1000 + u64::from(nanos) / 1_000_000;
        if time < start || time >= end {
            continue;
        }
        let Ok(file) = std::fs::File::open(&path) else {
            continue;
        };
        let Some(Ok(line)) = BufReader::new(file).lines().next() else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let meta = &value["payload"];
        if meta["source"] == "cli"
            && meta["cwd"]
                .as_str()
                .is_some_and(|cwd| Path::new(cwd) == record.launch.cwd)
            && (meta["id"] == id || meta["session_id"] == id)
            && !records
                .iter()
                .any(|r| r.id != record.id && r.conversation_id.as_deref() == Some(id))
        {
            matches.push(id.to_owned());
        }
    }
    matches.sort();
    matches.dedup();
    (matches.len() == 1).then(|| matches.remove(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_window_decoration_is_not_part_of_the_conversation_name() {
        assert_eq!(claude_osc_title("✳ Claude Code"), "Claude Code");
        assert_eq!(claude_osc_title("✳️ Fix sidebar"), "Fix sidebar");
        assert_eq!(claude_osc_title("Fix ✳ icons"), "Fix ✳ icons");
        assert_eq!(claude_osc_title("✳"), "✳");
    }
    #[test]
    fn title_identity_is_separate_from_its_display_name() {
        let id = "01a16585-21de-7e41-9a31-9857f8268084";
        assert_eq!(
            codex_title(&format!("{}... · Fix sidebar", &id[..29])),
            Some((id[..29].into(), Some("Fix sidebar".into())))
        );
        assert_eq!(codex_title(id), Some((id.into(), None)));
        assert!(codex_title("Fix sidebar").is_none());
    }

    #[test]
    fn legacy_recovery_refuses_to_guess_between_conversations() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("sessions/2026/10/23");
        std::fs::create_dir_all(&folder).unwrap();
        let record = TerminalRecord {
            id: del_agent::SessionId::new("terminal-65275-1792704584548540000"),
            title: "Codex".into(),
            launch: crate::terminal_host::TerminalLaunchSpec::shell("/work"),
            pinned: false,
            conversation_id: None,
        };
        let first = "01a16585-21de-7e41-9a31-9857f8268084";
        let second = "01a16585-7da2-71c1-af17-6c8de9e50723";
        let write = |id: &str| {
            std::fs::write(
                folder.join(format!("rollout-{id}.jsonl")),
                serde_json::json!({"payload":{"id":id,"source":"cli","cwd":"/work"}}).to_string(),
            )
            .unwrap()
        };
        write(first);
        assert_eq!(
            recover_legacy_codex(dir.path(), &record, std::slice::from_ref(&record)),
            Some(first.into())
        );
        write(second);
        assert_eq!(
            recover_legacy_codex(dir.path(), &record, std::slice::from_ref(&record)),
            None
        );
        assert_eq!(
            resolve_codex_prefix(dir.path(), &first[..29]),
            Some(first.into())
        );
    }
}
