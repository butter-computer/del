//! Local file editing and read-only Git review used by the workspace sidebar.
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// Largest diff or new-file preview kept per file.
const MAX_PREVIEW_BYTES: usize = 512 * 1024;
/// Largest `git status` listing read; huge untracked trees are cut off here.
const MAX_STATUS_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FileDraft {
    pub original: String,
    pub text: String,
}

pub fn save_file(path: &Path, original: &str, text: &str) -> Result<(), String> {
    // Resolve symlinks so saving edits their target rather than replacing the link.
    let target = path.canonicalize().map_err(|e| e.to_string())?;
    let current = std::fs::read_to_string(&target).map_err(|e| e.to_string())?;
    if current != original {
        return Err("File changed on disk. Your edits are kept. Copy them before using Revert, then Reload to read the new version.".into());
    }
    let permissions = std::fs::metadata(&target)
        .map_err(|e| e.to_string())?
        .permissions();
    let mut file = tempfile::NamedTempFile::new_in(target.parent().ok_or("File has no parent")?)
        .map_err(|e| e.to_string())?;
    file.as_file()
        .set_permissions(permissions)
        .map_err(|e| e.to_string())?;
    file.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
    file.as_file().sync_all().map_err(|e| e.to_string())?;
    // Check again after preparing the replacement, before touching the original.
    if std::fs::read_to_string(&target).map_err(|e| e.to_string())? != original {
        return Err("File changed on disk while saving. Your edits are kept.".into());
    }
    file.persist(&target).map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChangedFile {
    pub path: PathBuf,
    pub previous: Option<PathBuf>,
    pub status: String,
}

/// Read-only Git for the review pane, which polls while visible. It must not
/// take `index.lock` (an agent's concurrent commit would fail) or run commands
/// named by repository config such as `core.fsmonitor`.
fn git_command(root: &Path) -> Result<Command, String> {
    let program = crate::projects::git_program().ok_or("Git is not installed.")?;
    let mut command = Command::new(program);
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args([
            "--no-optional-locks",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.untrackedCache=false",
        ])
        .arg("-C")
        .arg(root);
    Ok(command)
}

/// Longest a review-pane Git command may run, as for project metadata queries.
const GIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// Run Git, keeping at most `limit` bytes of stdout. Returns the output and
/// whether it was cut off (the process is then stopped).
fn git_bounded(
    root: &Path,
    args: &[&std::ffi::OsStr],
    limit: usize,
) -> Result<(Vec<u8>, bool), String> {
    let mut child = git_command(root)?
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    // Drain stderr concurrently so a chatty Git can't block on a full pipe.
    let mut errors = child.stderr.take().expect("piped stderr");
    let stderr = std::thread::spawn(move || {
        let mut kept = Vec::new();
        let _ = (&mut errors).take(64 * 1024).read_to_end(&mut kept);
        let _ = std::io::copy(&mut errors, &mut std::io::sink());
        kept
    });
    let output = child.stdout.take().expect("piped stdout");
    // A hung Git (e.g. a network filesystem) is stopped at the deadline,
    // which also ends the read below.
    let child = std::sync::Arc::new(std::sync::Mutex::new(child));
    let (finished, deadline) = std::sync::mpsc::channel::<()>();
    let watchdog = {
        let child = child.clone();
        std::thread::spawn(move || {
            let expired = deadline.recv_timeout(GIT_TIMEOUT)
                == Err(std::sync::mpsc::RecvTimeoutError::Timeout);
            if expired {
                let _ = child.lock().unwrap().kill();
            }
            expired
        })
    };
    let mut stdout = Vec::new();
    let read = output.take(limit as u64 + 1).read_to_end(&mut stdout);
    let truncated = stdout.len() > limit;
    if truncated {
        let _ = child.lock().unwrap().kill();
    }
    // Poll rather than block in wait() so the watchdog can still take the lock.
    let status = loop {
        match child.lock().unwrap().try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {}
            Err(error) => break Err(error.to_string()),
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    let _ = finished.send(());
    let timed_out = watchdog.join().unwrap_or(false);
    let stderr = stderr.join().unwrap_or_default();
    if timed_out {
        return Err("Git timed out".into());
    }
    read.map_err(|e| e.to_string())?;
    if truncated {
        stdout.truncate(limit);
        return Ok((stdout, true));
    }
    let status = status?;
    if !status.success() {
        return Err(String::from_utf8_lossy(&stderr).trim().to_owned());
    }
    Ok((stdout, false))
}

fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let args: Vec<_> = args.iter().map(std::ffi::OsStr::new).collect();
    git_bounded(root, &args, MAX_STATUS_BYTES).map(|(out, _)| out)
}

/// Whether the repository has a commit to diff against. Resolve once per refresh.
pub fn has_head(root: &Path) -> bool {
    head_id(root).is_some()
}

pub fn head_id(root: &Path) -> Option<String> {
    git(root, &["rev-parse", "--verify", "--quiet", "HEAD"])
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_owned())
}

pub fn changes(root: &Path) -> Result<(PathBuf, Vec<ChangedFile>), String> {
    let base = git(root, &["rev-parse", "--show-toplevel"])
        .map_err(|_| "No Git repository".to_string())?;
    let base = PathBuf::from(
        String::from_utf8(base)
            .map_err(|e| e.to_string())?
            .trim_end_matches('\n'),
    );
    let status = git(
        &base,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?;
    // Drop the partial record before decoding: the cut may split a UTF-8 path.
    let status = String::from_utf8(complete_records(status))
        .map_err(|_| "Git paths contain unsupported text encoding")?;
    let mut records = status.split('\0').filter(|s| !s.is_empty());
    let mut files = Vec::new();
    while let Some(record) = records.next() {
        let code = record.get(..2).ok_or("Invalid Git status")?;
        let path = record.get(3..).ok_or("Invalid Git path")?;
        let previous = if code.contains('R') || code.contains('C') {
            Some(PathBuf::from(records.next().ok_or("Missing renamed path")?))
        } else {
            None
        };
        files.push(ChangedFile {
            path: path.into(),
            previous,
            status: code.into(),
        });
    }
    Ok((base, files))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Commit {
    pub id: String,
    pub short_id: String,
    pub subject: String,
    pub author: String,
    pub date: String,
    pub added: u64,
    pub removed: u64,
}

/// Recent history of the current branch, used only when there are no changes.
pub fn recent_commits(root: &Path) -> Result<Vec<Commit>, String> {
    if !has_head(root) {
        return Ok(Vec::new());
    }
    let args: Vec<_> = [
        "log",
        "-50",
        "-z",
        "--numstat",
        "--find-renames",
        "--no-ext-diff",
        "--no-textconv",
        "--diff-merges=first-parent",
        "--format=%x00%H%x00%h%x00%s%x00%an%x00%cI",
        "HEAD",
        "--",
    ]
    .into_iter()
    .map(std::ffi::OsStr::new)
    .collect();
    let (output, truncated) = git_bounded(root, &args, MAX_STATUS_BYTES)?;
    if truncated {
        return Err("Commit history is too large to load line totals.".into());
    }
    parse_commits(&String::from_utf8_lossy(&output))
}

/// Commit objects are immutable; never compare historical reviews with the worktree.
fn validate_commit(id: &str) -> Result<(), String> {
    if matches!(id.len(), 40 | 64) && id.bytes().all(|c| c.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err("Invalid commit ID".into())
    }
}

pub fn commit_files(root: &Path, id: &str) -> Result<Vec<ChangedFile>, String> {
    validate_commit(id)?;
    let args: Vec<_> = [
        "diff-tree",
        "--root",
        "--no-commit-id",
        "-r",
        "-z",
        "--name-status",
        "--find-renames",
        "--no-ext-diff",
        "--no-textconv",
        "--diff-merges=first-parent",
        id,
        "--",
    ]
    .into_iter()
    .map(std::ffi::OsStr::new)
    .collect();
    let (bytes, truncated) = git_bounded(root, &args, MAX_STATUS_BYTES)?;
    if truncated {
        return Err("Commit file list is too large to display.".into());
    }
    let text =
        String::from_utf8(bytes).map_err(|_| "Git paths contain unsupported text encoding")?;
    let mut fields = text.split_terminator('\0');
    let mut files = Vec::new();
    while let Some(status) = fields.next() {
        let first = fields.next().ok_or("Missing commit file path")?;
        let (previous, path) = if status.starts_with(['R', 'C']) {
            (
                Some(PathBuf::from(first)),
                fields.next().ok_or("Missing renamed path")?,
            )
        } else {
            (None, first)
        };
        files.push(ChangedFile {
            path: path.into(),
            previous,
            status: status.into(),
        });
    }
    Ok(files)
}

pub fn commit_diff(root: &Path, id: &str, file: &ChangedFile) -> Result<String, String> {
    validate_commit(id)?;
    let paths: Vec<_> = std::iter::once(&file.path)
        .chain(file.previous.iter())
        .map(|path| format!(":(literal){}", path.to_string_lossy()))
        .collect();
    let mut args: Vec<_> = [
        "diff-tree",
        "--root",
        "--no-commit-id",
        "-r",
        "-p",
        "--find-renames",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--diff-merges=first-parent",
        id,
        "--",
    ]
    .into_iter()
    .map(std::ffi::OsStr::new)
    .collect();
    args.extend(paths.iter().map(std::ffi::OsStr::new));
    let (bytes, truncated) = git_bounded(root, &args, MAX_PREVIEW_BYTES)?;
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if text.is_empty() {
        text = "No textual changes (file metadata only).".into();
    }
    if truncated {
        if let Some(end) = text.rfind('\n') {
            text.truncate(end + 1);
        }
        text.push_str("… Diff preview truncated.\n");
    }
    Ok(text)
}

pub fn commit_date_label<T: chrono::TimeZone>(date: &str, now: chrono::DateTime<T>) -> String {
    use chrono::Datelike;
    let Ok(at) = chrono::DateTime::parse_from_rfc3339(date) else {
        return date.into();
    };
    let at = at.with_timezone(&now.timezone());
    let day = at.date_naive();
    let today = now.date_naive();
    if day == today {
        let hours = (now - at).num_hours();
        return if hours < 1 {
            "Today".into()
        } else if hours == 1 {
            "1 hr ago".into()
        } else {
            format!("{hours} hrs ago")
        };
    }
    if today.pred_opt() == Some(day) {
        return "Yesterday".into();
    }
    let d = day.day();
    let suffix = match (d % 100, d % 10) {
        (11..=13, _) => "th",
        (_, 1) => "st",
        (_, 2) => "nd",
        (_, 3) => "rd",
        _ => "th",
    };
    let year = if day.year() == today.year() {
        String::new()
    } else {
        format!(" {}", day.year())
    };
    format!("{d}{suffix} {}{year}", day.format("%b"))
}

fn parse_commits(output: &str) -> Result<Vec<Commit>, String> {
    let mut fields = output.split_terminator('\0');
    let mut commits: Vec<Commit> = Vec::new();
    while let Some(field) = fields.next() {
        if field.is_empty() {
            let mut next = || {
                fields
                    .next()
                    .map(str::to_owned)
                    .ok_or_else(|| "Incomplete Git commit metadata".to_string())
            };
            commits.push(Commit {
                id: next()?,
                short_id: next()?,
                subject: next()?,
                author: next()?,
                date: next()?,
                added: 0,
                removed: 0,
            });
            continue;
        }
        let mut stat = field.trim_start_matches('\n').splitn(3, '\t');
        let added = stat.next().ok_or("Invalid Git line totals")?;
        let removed = stat.next().ok_or("Invalid Git line totals")?;
        let path = stat.next().ok_or("Missing Git diff path")?;
        if path.is_empty() {
            // With -z, renames carry two separate NUL-terminated paths.
            fields.next().ok_or("Missing original Git path")?;
            fields.next().ok_or("Missing renamed Git path")?;
        }
        let commit = commits.last_mut().ok_or("Missing Git commit metadata")?;
        if added != "-" && removed != "-" {
            commit.added += added
                .parse::<u64>()
                .map_err(|_| "Invalid Git added lines")?;
            commit.removed += removed
                .parse::<u64>()
                .map_err(|_| "Invalid Git removed lines")?;
        }
    }
    Ok(commits)
}

/// A listing cut off at the size cap ends mid-record; keep only whole
/// NUL-terminated records.
fn complete_records(mut status: Vec<u8>) -> Vec<u8> {
    if status.last().is_some_and(|&b| b != 0) {
        status.truncate(status.iter().rposition(|&b| b == 0).map_or(0, |i| i + 1));
    }
    status
}

/// `head` comes from [`has_head`], resolved once for all files of a refresh.
pub fn diff(root: &Path, file: &ChangedFile, head: bool) -> Result<String, String> {
    if file.status == "??" {
        let path = root.join(&file.path);
        let meta = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        let text = if meta.file_type().is_symlink() {
            std::fs::read_link(&path)
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .into_owned()
        } else if !meta.is_file() {
            return Ok("Not a regular file".into());
        } else {
            if meta.len() > MAX_PREVIEW_BYTES as u64 {
                return Ok("New file is too large to preview.".into());
            }
            let mut bytes = Vec::new();
            std::fs::File::open(&path)
                .and_then(|f| f.take(MAX_PREVIEW_BYTES as u64 + 1).read_to_end(&mut bytes))
                .map_err(|e| e.to_string())?;
            if bytes.len() > MAX_PREVIEW_BYTES {
                return Ok("New file is too large to preview.".into());
            }
            match String::from_utf8(bytes) {
                Ok(text) => text,
                Err(_) => return Ok("New binary file".into()),
            }
        };
        return Ok(format!(
            "New file: {}\n{}",
            file.path.display(),
            text.lines()
                .map(|line| format!("+{line}\n"))
                .collect::<String>()
        ));
    }
    fn diff_args<'a>(file: &'a ChangedFile, base: Option<&'a str>) -> Vec<&'a std::ffi::OsStr> {
        let mut args: Vec<&std::ffi::OsStr> =
            ["diff", "--no-ext-diff", "--no-textconv", "--no-color"]
                .into_iter()
                .chain(base)
                .chain(["--"])
                .map(std::ffi::OsStr::new)
                .collect();
        args.push(file.path.as_os_str());
        if base.is_some()
            && let Some(previous) = &file.previous
        {
            args.push(previous.as_os_str());
        }
        args
    }
    let (mut bytes, mut truncated) = git_bounded(
        root,
        &diff_args(file, Some(if head { "HEAD" } else { "--cached" })),
        MAX_PREVIEW_BYTES,
    )?;
    if !head && !truncated {
        let (extra, cut) = git_bounded(
            root,
            &diff_args(file, None),
            MAX_PREVIEW_BYTES - bytes.len(),
        )?;
        bytes.extend(extra);
        truncated = cut;
    }
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if text.is_empty() {
        text = "No textual diff (the index or file metadata may have changed).".into();
    }
    if truncated {
        // Lossy decoding may have replaced a split final character; drop the partial line.
        if let Some(end) = text.rfind('\n') {
            text.truncate(end + 1);
        }
        text.push_str("… Diff preview truncated.\n");
    }
    Ok(text)
}

/// Display rows omit Git's file headers, which must not count as edits.
pub struct ReviewLine {
    pub kind: char,
    pub text: String,
    pub old: Option<usize>,
    pub new: Option<usize>,
}

pub fn review_lines(text: &str) -> Vec<ReviewLine> {
    let mut old = 0;
    let mut new = 0;
    let mut in_hunk = text.starts_with("New file: ");
    let mut rows = Vec::new();
    for line in text.lines() {
        if line.starts_with("@@ ") {
            let mut parts = line.split_whitespace();
            parts.next();
            old = parts
                .next()
                .and_then(|s| s.trim_start_matches('-').split(',').next()?.parse().ok())
                .unwrap_or(0);
            new = parts
                .next()
                .and_then(|s| s.trim_start_matches('+').split(',').next()?.parse().ok())
                .unwrap_or(0);
            in_hunk = true;
            rows.push(ReviewLine {
                kind: ' ',
                text: line.into(),
                old: None,
                new: None,
            });
            continue;
        }
        if line.starts_with("diff --git ") {
            in_hunk = false;
        }
        if line.starts_with("New file: ") {
            new = 1;
            continue;
        }
        if !in_hunk
            && (line.starts_with("diff --git ")
                || line.starts_with("index ")
                || line.starts_with("--- ")
                || line.starts_with("+++ "))
        {
            continue;
        }
        let kind = if in_hunk {
            line.chars().next().unwrap_or(' ')
        } else {
            ' '
        };
        let (old_number, new_number) = match kind {
            '+' => {
                let n = new;
                new += 1;
                (None, Some(n))
            }
            '-' => {
                let n = old;
                old += 1;
                (Some(n), None)
            }
            ' ' if in_hunk => {
                let pair = (Some(old), Some(new));
                old += 1;
                new += 1;
                pair
            }
            _ => (None, None),
        };
        rows.push(ReviewLine {
            kind,
            text: if in_hunk && matches!(kind, '+' | '-' | ' ') {
                line.get(1..).unwrap_or("").into()
            } else {
                line.into()
            },
            old: old_number,
            new: new_number,
        });
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn friendly_commit_dates_follow_local_calendar_days() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-06T16:30:00+05:30").unwrap();
        for (date, expected) in [
            ("2026-11-06T16:00:00+05:30", "Today"),
            ("2026-11-06T15:30:00+05:30", "1 hr ago"),
            ("2026-11-06T12:30:00+05:30", "4 hrs ago"),
            ("2026-11-05T23:59:00+05:30", "Yesterday"),
            ("2026-11-05T23:30:00-05:00", "6 hrs ago"),
            ("2026-10-05T12:00:00+05:30", "5th Oct"),
            ("2026-10-01T12:00:00+05:30", "1st Oct"),
            ("2026-10-02T12:00:00+05:30", "2nd Oct"),
            ("2026-10-03T12:00:00+05:30", "3rd Oct"),
            ("2026-10-11T12:00:00+05:30", "11th Oct"),
            ("2026-10-12T12:00:00+05:30", "12th Oct"),
            ("2026-10-13T12:00:00+05:30", "13th Oct"),
            ("2025-11-06T12:00:00+05:30", "6th Nov 2025"),
        ] {
            assert_eq!(commit_date_label(date, now), expected, "{date}");
        }
    }

    #[test]
    fn historical_reviews_handle_roots_renames_deletions_binary_and_merges() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q"]).unwrap();
        git(root, &["config", "user.name", "Tester"]).unwrap();
        git(root, &["config", "user.email", "test@example.invalid"]).unwrap();
        let commit = |message: &str| {
            git(root, &["add", "."]).unwrap();
            git(root, &["commit", "--allow-empty", "-qm", message]).unwrap();
            head_id(root).unwrap()
        };
        let unusual = ":(glob)*.txt";
        std::fs::write(root.join(unusual), "before\n").unwrap();
        std::fs::write(root.join("old\tname\n.txt"), "rename me\n").unwrap();
        std::fs::write(root.join("delete.txt"), "gone\n").unwrap();
        std::fs::write(root.join("binary"), b"old\0binary").unwrap();
        let initial = commit("initial");
        let files = commit_files(root, &initial).unwrap();
        assert_eq!(files.len(), 4);
        assert!(files.iter().all(|f| f.status == "A"));
        let file = files.iter().find(|f| f.path == Path::new(unusual)).unwrap();
        assert!(
            commit_diff(root, &initial, file)
                .unwrap()
                .contains("+before")
        );
        git(root, &["mv", "old\tname\n.txt", "new name.txt"]).unwrap();
        let rename = commit("rename");
        let files = commit_files(root, &rename).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0].previous.as_deref(),
            Some(Path::new("old\tname\n.txt"))
        );
        assert!(
            commit_diff(root, &rename, &files[0])
                .unwrap()
                .contains("rename to new name.txt")
        );
        std::fs::write(root.join(unusual), "after\n").unwrap();
        std::fs::remove_file(root.join("delete.txt")).unwrap();
        std::fs::write(root.join("binary"), b"new\0binary").unwrap();
        let changed = commit("changes");
        let empty = commit("empty");
        assert!(commit_files(root, &empty).unwrap().is_empty());
        // Current disk/index changes must never leak into an older commit's diff.
        std::fs::write(root.join(unusual), "uncommitted\n").unwrap();
        let files = commit_files(root, &changed).unwrap();
        for file in &files {
            let diff = commit_diff(root, &changed, file).unwrap();
            assert!(!diff.contains("uncommitted"));
            if file.path == Path::new(unusual) {
                assert!(diff.contains("-before") && diff.contains("+after"));
                assert!(
                    !diff.contains("gone"),
                    "literal path must not expand as a glob"
                );
            } else if file.path == Path::new("delete.txt") {
                assert_eq!(file.status, "D");
                assert!(diff.contains("-gone"));
            } else {
                assert!(diff.contains("Binary files"));
            }
        }
        git(root, &["checkout", "-qb", "side"]).unwrap();
        std::fs::write(root.join("side.txt"), "side\n").unwrap();
        commit("side");
        git(root, &["checkout", "-q", "-"]).unwrap();
        git(root, &["merge", "--no-ff", "-qm", "merge", "side"]).unwrap();
        let merge = head_id(root).unwrap();
        let files = commit_files(root, &merge).unwrap();
        assert!(files.iter().any(|f| f.path == Path::new("side.txt")));
        assert!(
            commit_diff(
                root,
                &merge,
                files
                    .iter()
                    .find(|f| f.path == Path::new("side.txt"))
                    .unwrap()
            )
            .unwrap()
            .contains("+side")
        );
        assert!(commit_files(root, "--all").is_err());
        assert!(commit_files(root, &"0".repeat(40)).is_err());
    }

    #[test]
    fn commit_line_totals_handle_roots_renames_binary_and_empty_commits() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q"]).unwrap();
        git(root, &["config", "user.name", "Tester"]).unwrap();
        git(root, &["config", "user.email", "test@example.invalid"]).unwrap();
        let commit = |message: &str| {
            git(root, &["add", "."]).unwrap();
            git(root, &["commit", "--allow-empty", "-qm", message]).unwrap();
        };
        let old = "old\tfile\nname";
        let new = "new\tfile\nname";
        std::fs::write(root.join(old), "one\ntwo\nthree\n").unwrap();
        std::fs::write(root.join("binary"), b"hello\0world").unwrap();
        commit("initial");
        git(root, &["mv", old, new]).unwrap();
        commit("rename");
        std::fs::write(root.join(new), "one\nchanged\nthree\nfour\n").unwrap();
        std::fs::write(root.join("binary"), b"new\0binary").unwrap();
        commit("edit");
        commit("empty");
        let commits = recent_commits(root).unwrap();
        assert_eq!(
            commits
                .iter()
                .map(|c| (c.subject.as_str(), c.added, c.removed))
                .collect::<Vec<_>>(),
            vec![
                ("empty", 0, 0),
                ("edit", 2, 1),
                ("rename", 0, 0),
                ("initial", 3, 0)
            ]
        );
        // Merge rows compare against the first parent, without repeating a commit.
        git(root, &["checkout", "-qb", "side"]).unwrap();
        std::fs::write(root.join("side.txt"), "side\n").unwrap();
        commit("side change");
        git(root, &["checkout", "-q", "-"]).unwrap();
        git(root, &["merge", "--no-ff", "-qm", "merge side", "side"]).unwrap();
        let commits = recent_commits(root).unwrap();
        assert_eq!(
            (
                commits[0].subject.as_str(),
                commits[0].added,
                commits[0].removed
            ),
            ("merge side", 1, 0)
        );
        assert_eq!(commits.len(), 6);
    }

    #[test]
    fn review_tracks_hunk_numbers_and_does_not_count_file_headers() {
        let rows = review_lines(
            "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -8,2 +8,2 @@\n keep\n-old\n+new\n",
        );
        assert_eq!(rows.iter().filter(|r| r.kind == '+').count(), 1);
        assert_eq!(rows.iter().filter(|r| r.kind == '-').count(), 1);
        assert_eq!(rows[2].old, Some(9));
        assert_eq!(rows[3].new, Some(9));
        let rows = review_lines("New file: a\n+first\n+second\n");
        assert_eq!(rows[1].new, Some(2));
    }

    #[test]
    fn capped_status_keeps_whole_records_even_mid_character() {
        assert_eq!(complete_records(b" M a\0?? b\0".to_vec()), b" M a\0?? b\0");
        // The cap can split a multi-byte path; the partial record is dropped
        // before decoding so the listing still loads.
        let mut cut = " M first\0?? café".as_bytes().to_vec();
        cut.pop();
        let whole = complete_records(cut);
        assert_eq!(String::from_utf8(whole).unwrap(), " M first\0");
        assert!(complete_records(b"?? partial".to_vec()).is_empty());
        assert!(complete_records(Vec::new()).is_empty());
    }

    #[test]
    fn saves_preserve_exact_bytes_and_reject_external_edits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.txt");
        let original = "\u{feff}hello\r\n\tworld\r\n";
        std::fs::write(&path, original).unwrap();
        let edited = original.replace("hello", "updated");
        save_file(&path, original, &edited).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), edited);
        std::fs::write(&path, "external edit").unwrap();
        assert!(save_file(&path, &edited, "my edit").is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "external edit");
    }
    #[test]
    fn symlink_save_keeps_the_link_and_executable_permissions() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("run.sh");
        let link = dir.path().join("linked.sh");
        std::fs::write(&target, "before").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
        symlink(&target, &link).unwrap();
        save_file(&link, "before", "after").unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "after");
        assert_eq!(
            std::fs::metadata(target).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }
    #[test]
    fn git_review_includes_staged_unstaged_renamed_and_untracked_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q"]).unwrap();
        let name = "first file.txt";
        std::fs::write(root.join(name), "old\n").unwrap();
        git(root, &["add", "--", name]).unwrap();
        assert!(!has_head(root));
        assert!(recent_commits(root).unwrap().is_empty());
        assert!(
            diff(root, &changes(root).unwrap().1[0], false)
                .unwrap()
                .contains("+old")
        );
        git(
            root,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-qm",
                "initial",
            ],
        )
        .unwrap();
        git(root, &["mv", name, "renamed.txt"]).unwrap();
        std::fs::write(root.join("renamed.txt"), "old\nnew\n").unwrap();
        std::fs::write(root.join("new\nfile.txt"), "new content\n").unwrap();
        let (_, files) = changes(root).unwrap();
        assert_eq!(files.len(), 2);
        let renamed = files
            .iter()
            .find(|file| file.path == Path::new("renamed.txt"))
            .unwrap();
        assert_eq!(renamed.previous.as_deref(), Some(Path::new(name)));
        assert!(has_head(root));
        assert!(diff(root, renamed, true).unwrap().contains("+new"));
        let untracked = files.iter().find(|file| file.status == "??").unwrap();
        assert_eq!(untracked.path, Path::new("new\nfile.txt"));
        assert!(
            diff(root, untracked, true)
                .unwrap()
                .contains("+new content")
        );
        assert!(changes(tempfile::tempdir().unwrap().path()).is_err());
    }

    #[test]
    fn review_ignores_repo_fsmonitor_takes_no_lock_and_bounds_diffs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q"]).unwrap();
        let marker = root.join("fsmonitor-ran");
        let hook = root.join("hook.sh");
        std::fs::write(&hook, format!("#!/bin/sh\ntouch '{}'\n", marker.display())).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        git(root, &["config", "core.fsmonitor", hook.to_str().unwrap()]).unwrap();
        std::fs::write(root.join("big.txt"), "x\n".repeat(400_000)).unwrap();
        git(root, &["add", "big.txt"]).unwrap();
        let (_, files) = changes(root).unwrap();
        assert!(
            !marker.exists(),
            "repository fsmonitor command must not run"
        );
        assert!(!root.join(".git/index.lock").exists());
        let big = files
            .iter()
            .find(|f| f.path == Path::new("big.txt"))
            .unwrap();
        let text = diff(root, big, false).unwrap();
        assert!(text.len() <= MAX_PREVIEW_BYTES + 64);
        assert!(text.ends_with("… Diff preview truncated.\n"));
        assert!(!marker.exists());
    }
}
