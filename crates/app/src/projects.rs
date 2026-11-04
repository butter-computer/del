//! Directory identity and owned Git worktrees. Git runs only on background workers.
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Debug, Default)]
pub struct GitInfo {
    pub root: PathBuf,
    pub current: String,
    pub branches: Vec<String>,
    /// Commits ahead of the upstream branch, when one is configured.
    pub ahead: u32,
    pub behind: u32,
    pub changed_files: Option<usize>,
    pub changed_lines: Option<(u64, u64)>,
    /// The checkout's directory name when it is a linked worktree.
    pub worktree: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct LaunchOptions {
    pub branch: Option<String>,
    pub worktree: bool,
    // Stable across failed provider startup/retries; never recycle another session's checkout.
    pub token: String,
}

/// The Git executable, resolved once. `/usr/bin/git` is only Apple's
/// `xcode-select` shim: without Command Line Tools it pops an install dialog,
/// so it is used only when a developer directory is actually installed.
pub fn git_program() -> Option<&'static Path> {
    static GIT: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    GIT.get_or_init(|| {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let candidates = std::env::split_paths(&path)
            .chain(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"].map(PathBuf::from))
            .map(|dir| dir.join("git"));
        let mut shim_checked = None;
        for candidate in candidates {
            if !candidate.is_file() {
                continue;
            }
            let real = candidate
                .canonicalize()
                .unwrap_or_else(|_| candidate.clone());
            if real == Path::new("/usr/bin/git")
                && !*shim_checked.get_or_insert_with(developer_tools_installed)
            {
                continue;
            }
            return Some(candidate);
        }
        None
    })
    .as_deref()
}

fn developer_tools_installed() -> bool {
    let mut command = Command::new("/usr/bin/xcode-select");
    command.arg("-p");
    bounded(&mut command, std::time::Duration::from_secs(5)).is_ok_and(|output| {
        output.status.success()
            && Path::new(String::from_utf8_lossy(&output.stdout).trim()).is_dir()
    })
}

/// Run with a deadline. Output goes through files so a full pipe cannot
/// deadlock the child while we wait.
fn bounded(
    command: &mut Command,
    timeout: std::time::Duration,
) -> std::io::Result<std::process::Output> {
    use std::io::{Read, Seek};
    let mut stdout = tempfile::tempfile()?;
    let mut stderr = tempfile::tempfile()?;
    let mut child = command
        .stdin(std::process::Stdio::null())
        .stdout(stdout.try_clone()?)
        .stderr(stderr.try_clone()?)
        .spawn()?;
    let start = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("{} timed out", command.get_program().to_string_lossy()),
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    let (mut out, mut err) = (Vec::new(), Vec::new());
    stdout.rewind()?;
    stdout.read_to_end(&mut out)?;
    stderr.rewind()?;
    stderr.take(64 * 1024).read_to_end(&mut err)?;
    Ok(std::process::Output {
        status,
        stdout: out,
        stderr: err,
    })
}

fn git(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let program = git_program().ok_or("Git is not installed.")?;
    // Checkouts can be slow on large repositories; metadata queries are not.
    let timeout = match args.first() {
        Some(&"worktree") | Some(&"switch") => std::time::Duration::from_secs(300),
        _ => std::time::Duration::from_secs(20),
    };
    let output = bounded(
        Command::new(program)
            // Read-only inspection must not take index locks or start an
            // fsmonitor daemon configured by the repository.
            .args(["--no-optional-locks", "-c", "core.fsmonitor=false", "-C"])
            .arg(cwd)
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0"),
        timeout,
    )
    .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim_end_matches('\n')
        .to_owned())
}

pub fn inspect(cwd: &Path) -> GitInfo {
    let canonical = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let Ok(top) = git(cwd, &["rev-parse", "--show-toplevel"]) else {
        return GitInfo {
            root: canonical,
            ..Default::default()
        };
    };
    // All linked worktrees belong to the main repository project.
    let common = git(
        cwd,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .ok();
    let root = common
        .as_deref()
        .map(Path::new)
        .filter(|p| p.file_name().is_some_and(|n| n == ".git"))
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(&top));
    let current = git(cwd, &["symbolic-ref", "--short", "HEAD"]).unwrap_or_else(|_| "HEAD".into());
    let branches = git(
        cwd,
        &[
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/heads",
            "refs/remotes",
        ],
    )
    .unwrap_or_default()
    .lines()
    .filter(|s| !s.ends_with("/HEAD"))
    .map(str::to_owned)
    .collect();
    let status = git(
        cwd,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )
    .ok();
    let changed_files = status.as_deref().map(changed_file_count);
    let changed_lines = status
        .as_deref()
        .and_then(|status| line_changes(cwd, status));
    let ahead = git(cwd, &["rev-list", "--count", "@{upstream}..HEAD"])
        .ok()
        .and_then(|count| count.parse().ok())
        .unwrap_or(0);
    let behind = git(cwd, &["rev-list", "--count", "HEAD..@{upstream}"])
        .ok()
        .and_then(|count| count.parse().ok())
        .unwrap_or(0);
    let top = PathBuf::from(&top);
    let worktree = (top != root)
        .then(|| {
            top.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .flatten();
    GitInfo {
        root,
        current,
        branches,
        ahead,
        behind,
        changed_files,
        changed_lines,
        worktree,
    }
}

/// Net index + worktree changes against HEAD, plus untracked text files.
fn line_changes(cwd: &Path, status: &str) -> Option<(u64, u64)> {
    let base = git(cwd, &["rev-parse", "--verify", "HEAD"])
        .or_else(|_| git(cwd, &["hash-object", "-t", "tree", "/dev/null"]))
        .ok()?;
    let diff = git(
        cwd,
        &[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "--numstat",
            "-z",
            &base,
            "--",
        ],
    )
    .ok()?;
    let mut counts = (0u64, 0u64);
    for entry in diff.split('\0').filter(|s| !s.is_empty()) {
        let mut parts = entry.splitn(3, '\t');
        let (added, removed) = (parts.next()?, parts.next()?);
        // Binary changes still contribute to the file count, never fake line counts.
        if added != "-" {
            counts.0 = counts.0.saturating_add(added.parse::<u64>().ok()?);
        }
        if removed != "-" {
            counts.1 = counts.1.saturating_add(removed.parse::<u64>().ok()?);
        }
    }
    let mut entries = status.split('\0').filter(|s| !s.is_empty());
    while let Some(entry) = entries.next() {
        if let Some(path) = entry.strip_prefix("?? ") {
            let path = cwd.join(path);
            let metadata = std::fs::symlink_metadata(&path).ok()?;
            if metadata.is_symlink() {
                counts.0 += 1;
                continue;
            }
            // Don't scan huge generated files on every footer refresh.
            if metadata.len() > 8 * 1024 * 1024 {
                return None;
            }
            let bytes = std::fs::read(path).ok()?;
            if !bytes.iter().take(8192).any(|b| *b == 0) {
                counts.0 += bytes.iter().filter(|b| **b == b'\n').count() as u64
                    + u64::from(!bytes.is_empty() && bytes.last() != Some(&b'\n'));
            }
        } else if entry
            .as_bytes()
            .iter()
            .take(2)
            .any(|b| matches!(b, b'R' | b'C'))
        {
            entries.next();
        }
    }
    Some(counts)
}

fn changed_file_count(status: &str) -> usize {
    let mut entries = status.split('\0').filter(|entry| !entry.is_empty());
    let mut count = 0;
    while let Some(entry) = entries.next() {
        count += 1;
        if entry
            .as_bytes()
            .iter()
            .take(2)
            .any(|b| matches!(b, b'R' | b'C'))
        {
            entries.next();
        }
    }
    count
}

#[derive(Serialize, Deserialize)]
struct OwnedWorktree {
    source: PathBuf,
    path: PathBuf,
    branch: String,
    base: String,
    #[serde(default)]
    base_commit: String,
}

pub fn prepare(cwd: &Path, options: &LaunchOptions, storage: &Path) -> Result<PathBuf, String> {
    if options.branch.is_none() && !options.worktree {
        return Ok(cwd.to_path_buf());
    }
    let info = inspect(cwd);
    if info.current.is_empty() {
        return Err("This folder is not a Git repository.".into());
    }
    let base = options.branch.as_deref().unwrap_or(&info.current);
    let parent = storage.join("worktrees").join(&options.token);
    // Keep a fallback checkout on retries, even if the source has since become
    // clean or switched branches. Never stash, reset, or copy the user's edits.
    let prepared = !options.token.is_empty() && parent.join("worktree.json").exists();
    let dirty_switch = base != info.current && !git(cwd, &["status", "--porcelain"])?.is_empty();
    if options.worktree || prepared || dirty_switch {
        if options.token.is_empty() {
            return Err("Missing worktree launch identity.".into());
        }
        let path = parent.join("checkout");
        let manifest = parent.join("worktree.json");
        let branch = format!("del/session-{}", options.token);
        let owned = OwnedWorktree {
            source: info.root,
            path: path.clone(),
            branch: branch.clone(),
            base: base.into(),
            base_commit: git(
                cwd,
                &["rev-parse", "--verify", &format!("{base}^{{commit}}")],
            )?,
        };
        if path.exists() {
            let saved: OwnedWorktree =
                serde_json::from_slice(&std::fs::read(&manifest).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
            if saved.source != owned.source
                || saved.base != owned.base
                || saved.path != path
                || git(&path, &["symbolic-ref", "--short", "HEAD"])? != branch
            {
                return Err(
                    "The saved worktree no longer matches this draft. Choose a new workspace."
                        .into(),
                );
            }
            return Ok(path);
        }
        // Verify a commit first, including for unborn repositories and stale branch selections.
        let commit = &owned.base_commit;
        std::fs::create_dir_all(&parent).map_err(|e| e.to_string())?;
        crate::sessions::write_atomic(
            &manifest,
            &serde_json::to_vec_pretty(&owned).map_err(|e| e.to_string())?,
            0o644,
        )
        .map_err(|e| e.to_string())?;
        git(
            cwd,
            &[
                "worktree",
                "add",
                "-b",
                &branch,
                path.to_str().ok_or("Invalid worktree path")?,
                commit,
            ],
        )?;
        return Ok(path);
    }
    if base != info.current {
        if !info.branches.iter().any(|branch| branch == base) {
            return Err("The selected branch no longer exists.".into());
        }
        git(cwd, &["switch", "--", base])?;
    }
    Ok(cwd.to_path_buf())
}

#[derive(Default, Serialize, Deserialize)]
pub struct ProjectPreferences {
    #[serde(default)]
    pub names: std::collections::HashMap<PathBuf, String>,
    #[serde(default)]
    pub archived: std::collections::HashSet<PathBuf>,
    #[serde(default)]
    pub deleted: std::collections::HashSet<PathBuf>,
    #[serde(default)]
    pub last_directory: Option<PathBuf>,
    #[serde(default)]
    pub directories: std::collections::HashMap<PathBuf, PathBuf>,
    #[serde(default)]
    pub roots: std::collections::HashSet<PathBuf>,
    #[serde(default)]
    pub collapsed: std::collections::HashSet<PathBuf>,
    #[serde(default)]
    pub collapsed_sections: std::collections::HashSet<String>,
}
impl ProjectPreferences {
    /// Unreadable preferences are preserved aside and reported, never overwritten.
    pub fn load() -> (Self, Option<String>) {
        match del_agent::data_dir() {
            Some(dir) => crate::sessions::load_or_set_aside(
                &dir.join("projects.json"),
                "project preferences",
            ),
            None => Default::default(),
        }
    }
    pub fn save(&self) -> std::io::Result<()> {
        let Some(dir) = del_agent::data_dir() else {
            return Ok(());
        };
        crate::sessions::write_atomic(
            &dir.join("projects.json"),
            &serde_json::to_vec_pretty(self)?,
            crate::sessions::PRIVATE,
        )
    }
}

/// Remove only owned, unused checkouts still at their original commit and clean.
/// Branches are deliberately retained; cleanup never discards a commit.
pub fn cleanup(root: &Path, in_use: &[PathBuf], storage: &Path) -> Result<usize, String> {
    let parent = storage.join("worktrees");
    if !parent.exists() {
        return Ok(0);
    }
    let mut removed = 0;
    for entry in std::fs::read_dir(parent).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let manifest = entry.path().join("worktree.json");
        let Ok(bytes) = std::fs::read(&manifest) else {
            continue;
        };
        let Ok(owned) = serde_json::from_slice::<OwnedWorktree>(&bytes) else {
            continue;
        };
        if owned.source != root
            || in_use.iter().any(|p| p.starts_with(&owned.path))
            || !owned.path.exists()
        {
            continue;
        }
        if owned.path != entry.path().join("checkout") || owned.base_commit.is_empty() {
            continue;
        }
        if git(&owned.path, &["rev-parse", "HEAD"])? != owned.base_commit
            || git(&owned.path, &["symbolic-ref", "--short", "HEAD"])? != owned.branch
            || !git(
                &owned.path,
                &[
                    "status",
                    "--porcelain",
                    "--untracked-files=all",
                    "--ignored",
                ],
            )?
            .is_empty()
        {
            continue;
        }
        git(
            root,
            &[
                "worktree",
                "remove",
                owned.path.to_str().ok_or("Invalid worktree path")?,
            ],
        )?;
        std::fs::remove_file(manifest).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_dir(entry.path());
        removed += 1;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn repository() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-b", "main"]).unwrap();
        git(
            dir.path(),
            &[
                "-c",
                "user.name=Del Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "--allow-empty",
                "-m",
                "initial",
            ],
        )
        .unwrap();
        git(dir.path(), &["branch", "feature"]).unwrap();
        dir
    }
    #[test]
    fn line_totals_include_staged_unstaged_untracked_and_ignore_binary_lines() {
        let dir = repository();
        std::fs::write(dir.path().join("tracked"), "old\nkeep\n").unwrap();
        git(dir.path(), &["add", "."]).unwrap();
        git(
            dir.path(),
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-m",
                "seed",
            ],
        )
        .unwrap();
        std::fs::write(dir.path().join("tracked"), "new\nkeep\n").unwrap();
        git(dir.path(), &["add", "."]).unwrap();
        std::fs::write(dir.path().join("tracked"), "new\nkeep\nextra\n").unwrap();
        std::fs::write(dir.path().join("new\nfile"), "one\ntwo").unwrap();
        std::fs::write(dir.path().join("binary"), [0, 1, 2]).unwrap();
        let info = inspect(dir.path());
        assert_eq!(info.changed_files, Some(3));
        assert_eq!(info.changed_lines, Some((4, 1)));
    }
    fn options() -> LaunchOptions {
        LaunchOptions {
            branch: Some("feature".into()),
            worktree: true,
            token: uuid::Uuid::new_v4().to_string(),
        }
    }
    #[test]
    fn preferences_without_newer_fields_still_load() {
        let saved: ProjectPreferences =
            serde_json::from_str(r#"{"names":{"/repo":"Repo"}}"#).unwrap();
        assert_eq!(saved.names.len(), 1);
        assert!(saved.collapsed.is_empty());
    }
    #[test]
    fn git_runs_with_a_deadline() {
        let error = bounded(
            Command::new("/bin/sleep").arg("5"),
            std::time::Duration::from_millis(50),
        )
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert!(git_program().is_some());
    }
    #[test]
    fn worktree_is_isolated_retryable_and_belongs_to_source_project() {
        let repo = repository();
        let storage = tempfile::tempdir().unwrap();
        let options = options();
        std::fs::write(repo.path().join("local.txt"), "keep me").unwrap();
        let path = prepare(repo.path(), &options, storage.path()).unwrap();
        assert_eq!(inspect(&path).root, repo.path().canonicalize().unwrap());
        assert_eq!(
            git(repo.path(), &["symbolic-ref", "--short", "HEAD"]).unwrap(),
            "main"
        );
        assert!(!path.join("local.txt").exists());
        assert_eq!(
            prepare(repo.path(), &options, storage.path()).unwrap(),
            path
        );
        let second = prepare(repo.path(), &self::options(), storage.path()).unwrap();
        assert_ne!(second, path);
    }
    #[test]
    fn dirty_branch_launch_uses_selected_tip_and_reuses_it_after_source_changes() {
        let repo = repository();
        let storage = tempfile::tempdir().unwrap();
        git(repo.path(), &["switch", "feature"]).unwrap();
        std::fs::write(repo.path().join("target.txt"), "selected branch tip").unwrap();
        git(repo.path(), &["add", "."]).unwrap();
        git(
            repo.path(),
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-m",
                "target tip",
            ],
        )
        .unwrap();
        let tip = git(repo.path(), &["rev-parse", "HEAD"]).unwrap();
        git(repo.path(), &["switch", "main"]).unwrap();
        // Even an untracked file (such as .concord/) currently blocks launch.
        std::fs::write(repo.path().join("local.txt"), "keep me").unwrap();
        let options = LaunchOptions {
            worktree: false,
            ..options()
        };
        let path = prepare(repo.path(), &options, storage.path()).unwrap();
        assert_ne!(path, repo.path());
        assert_eq!(git(&path, &["rev-parse", "HEAD"]).unwrap(), tip);
        assert_eq!(
            std::fs::read_to_string(path.join("target.txt")).unwrap(),
            "selected branch tip"
        );
        assert!(!path.join("local.txt").exists());
        assert_eq!(inspect(repo.path()).current, "main");
        assert_eq!(
            std::fs::read_to_string(repo.path().join("local.txt")).unwrap(),
            "keep me"
        );
        // A provider-start retry must keep its prepared workspace even if the
        // source becomes clean or the selected branch advances in the meantime.
        std::fs::remove_file(repo.path().join("local.txt")).unwrap();
        git(repo.path(), &["switch", "feature"]).unwrap();
        git(
            repo.path(),
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "--allow-empty",
                "-m",
                "later tip",
            ],
        )
        .unwrap();
        assert_eq!(
            prepare(repo.path(), &options, storage.path()).unwrap(),
            path
        );
        assert_eq!(git(&path, &["rev-parse", "HEAD"]).unwrap(), tip);
    }
    #[test]
    fn clean_local_switch_and_unknown_base() {
        let repo = repository();
        let storage = tempfile::tempdir().unwrap();
        let local = LaunchOptions {
            branch: Some("feature".into()),
            ..Default::default()
        };
        prepare(repo.path(), &local, storage.path()).unwrap();
        assert_eq!(inspect(repo.path()).current, "feature");
        let bad = LaunchOptions {
            branch: Some("missing".into()),
            ..options()
        };
        assert!(prepare(repo.path(), &bad, storage.path()).is_err());
    }
    #[test]
    fn cleanup_preserves_referenced_dirty_and_committed_worktrees() {
        let repo = repository();
        let storage = tempfile::tempdir().unwrap();
        let root = inspect(repo.path()).root;
        let used = prepare(repo.path(), &options(), storage.path()).unwrap();
        let dirty = prepare(repo.path(), &options(), storage.path()).unwrap();
        std::fs::write(dirty.join("file"), "keep").unwrap();
        let committed = prepare(repo.path(), &options(), storage.path()).unwrap();
        git(
            &committed,
            &[
                "-c",
                "user.name=Del Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "--allow-empty",
                "-m",
                "work",
            ],
        )
        .unwrap();
        let unused = prepare(repo.path(), &options(), storage.path()).unwrap();
        let branch = inspect(&unused).current;
        assert_eq!(
            cleanup(&root, std::slice::from_ref(&used), storage.path()).unwrap(),
            1
        );
        assert!(used.exists() && dirty.exists() && committed.exists());
        assert!(!unused.exists());
        assert!(git(repo.path(), &["rev-parse", "--verify", &branch]).is_ok());
    }
    #[test]
    fn plain_launch_and_current_branch_never_touch_the_checkout() {
        // No branch or worktree requested: no Git command runs, even outside a repository.
        let plain = tempfile::tempdir().unwrap();
        assert_eq!(
            prepare(plain.path(), &LaunchOptions::default(), plain.path()).unwrap(),
            plain.path()
        );
        let repo = repository();
        let storage = tempfile::tempdir().unwrap();
        std::fs::write(repo.path().join("dirty"), "keep").unwrap();
        let current = LaunchOptions {
            branch: Some("main".into()),
            ..Default::default()
        };
        assert_eq!(
            prepare(repo.path(), &current, storage.path()).unwrap(),
            repo.path()
        );
        assert_eq!(
            std::fs::read_to_string(repo.path().join("dirty")).unwrap(),
            "keep"
        );
        // Switching from a dirty source preserves its index and files.
        git(repo.path(), &["add", "dirty"]).unwrap();
        std::fs::write(repo.path().join("dirty"), "unstaged").unwrap();
        let status = git(repo.path(), &["status", "--porcelain"]).unwrap();
        let other = LaunchOptions {
            worktree: false,
            ..options()
        };
        let path = prepare(repo.path(), &other, storage.path()).unwrap();
        assert_ne!(path, repo.path());
        assert!(!path.join("dirty").exists());
        assert_eq!(
            git(repo.path(), &["status", "--porcelain"]).unwrap(),
            status
        );
        assert_eq!(git(repo.path(), &["show", ":dirty"]).unwrap(), "keep");
        assert_eq!(
            std::fs::read_to_string(repo.path().join("dirty")).unwrap(),
            "unstaged"
        );
        assert_eq!(inspect(repo.path()).current, "main");
        assert!(repo.path().join("dirty").exists());
    }

    #[test]
    fn cleanup_ignores_foreign_manifests_and_preserves_switched_or_ignored_checkouts() {
        let repo = repository();
        let storage = tempfile::tempdir().unwrap();
        let root = inspect(repo.path()).root;
        assert_eq!(cleanup(&root, &[], storage.path()).unwrap(), 0);
        // A manifest naming the user's own checkout (or anything outside its
        // owned entry) is never acted on.
        let foreign = storage.path().join("worktrees/foreign");
        std::fs::create_dir_all(&foreign).unwrap();
        let manifest = OwnedWorktree {
            source: root.clone(),
            path: repo.path().to_path_buf(),
            branch: "main".into(),
            base: "main".into(),
            base_commit: git(repo.path(), &["rev-parse", "HEAD"]).unwrap(),
        };
        std::fs::write(
            foreign.join("worktree.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        let corrupt = storage.path().join("worktrees/corrupt");
        std::fs::create_dir_all(&corrupt).unwrap();
        std::fs::write(corrupt.join("worktree.json"), "{not json").unwrap();
        // Checked-out another branch, or holding only ignored files: both are kept.
        let switched = prepare(repo.path(), &options(), storage.path()).unwrap();
        git(&switched, &["switch", "-c", "user-branch"]).unwrap();
        let ignored = prepare(repo.path(), &options(), storage.path()).unwrap();
        // Linked worktrees share the repository's info/exclude.
        std::fs::create_dir_all(repo.path().join(".git/info")).unwrap();
        std::fs::write(repo.path().join(".git/info/exclude"), "*.log\n").unwrap();
        std::fs::write(ignored.join("build.log"), "x").unwrap();
        // Records whose directory is inside an owned checkout keep it.
        let nested = prepare(repo.path(), &options(), storage.path()).unwrap();
        std::fs::create_dir_all(nested.join("sub")).unwrap();
        let unused = prepare(repo.path(), &options(), storage.path()).unwrap();
        assert_eq!(
            cleanup(&root, &[nested.join("sub")], storage.path()).unwrap(),
            1
        );
        assert!(repo.path().join(".git").exists());
        assert_eq!(inspect(repo.path()).current, "main");
        assert!(foreign.join("worktree.json").exists() && corrupt.exists());
        assert!(switched.exists() && ignored.join("build.log").exists() && nested.exists());
        assert!(!unused.exists());
        // Another project's cleanup never removes this project's checkouts.
        let elsewhere = repository();
        let kept = prepare(repo.path(), &options(), storage.path()).unwrap();
        assert_eq!(
            cleanup(&inspect(elsewhere.path()).root, &[], storage.path()).unwrap(),
            0
        );
        assert!(kept.exists());
    }

    #[test]
    fn preferences_round_trip_and_ignore_unknown_fields() {
        let saved: ProjectPreferences = serde_json::from_str(
            r#"{"names":{"/repo":"Repo"},"collapsed":["/repo"],"collapsed_sections":["Pinned"],
                "directories":{"/repo/sub":"/repo"},"future_setting":{"x":1}}"#,
        )
        .unwrap();
        let again: ProjectPreferences =
            serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap();
        assert_eq!(again.names.get(Path::new("/repo")).unwrap(), "Repo");
        assert!(again.collapsed.contains(Path::new("/repo")));
        assert!(again.collapsed_sections.contains("Pinned"));
        assert_eq!(
            again.directories.get(Path::new("/repo/sub")).unwrap(),
            Path::new("/repo")
        );
        assert!(again.archived.is_empty() && again.last_directory.is_none());
    }

    #[test]
    fn folders_without_git_remain_distinct_and_unborn_repos_fail_clearly() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        assert_ne!(inspect(first.path()).root, inspect(second.path()).root);
        assert!(prepare(first.path(), &options(), second.path()).is_err());
        git(first.path(), &["init", "-b", "main"]).unwrap();
        assert!(
            prepare(
                first.path(),
                &LaunchOptions {
                    branch: Some("main".into()),
                    ..options()
                },
                second.path()
            )
            .is_err()
        );
    }
}

#[cfg(test)]
mod status_tests {
    #[test]
    fn porcelain_counts_files_once_even_with_renames_and_newlines() {
        assert_eq!(super::changed_file_count(""), 0);
        assert_eq!(
            super::changed_file_count(" M file\0?? new\nfile\0R  renamed\0original\0"),
            3
        );
        assert_eq!(super::changed_file_count(" R renamed\0original\0"), 1);
    }
}
