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
            let real = candidate.canonicalize().unwrap_or_else(|_| candidate.clone());
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
    bounded(&mut command, std::time::Duration::from_secs(5))
        .is_ok_and(|output| {
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
    Ok(std::process::Output { status, stdout: out, stderr: err })
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
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
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
        .unwrap_or_else(|| PathBuf::from(top));
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
    GitInfo {
        root,
        current,
        branches,
    }
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
    if options.worktree {
        if options.token.is_empty() {
            return Err("Missing worktree launch identity.".into());
        }
        let parent = storage.join("worktrees").join(&options.token);
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
        let commit = git(
            cwd,
            &["rev-parse", "--verify", &format!("{base}^{{commit}}")],
        )?;
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
                &commit,
            ],
        )?;
        return Ok(path);
    }
    if base != info.current {
        if !git(cwd, &["status", "--porcelain"])?.is_empty() {
            return Err("This checkout has local changes. Use New worktree or commit/stash them before switching branches.".into());
        }
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
            Some(dir) => crate::sessions::load_or_set_aside(&dir.join("projects.json"), "project preferences"),
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
    fn options() -> LaunchOptions {
        LaunchOptions {
            branch: Some("feature".into()),
            worktree: true,
            token: uuid::Uuid::new_v4().to_string(),
        }
    }
    #[test]
    fn preferences_without_newer_fields_still_load() {
        let saved: ProjectPreferences = serde_json::from_str(r#"{"names":{"/repo":"Repo"}}"#).unwrap();
        assert_eq!(saved.names.len(), 1);
        assert!(saved.collapsed.is_empty());
    }
    #[test]
    fn git_runs_with_a_deadline() {
        let error = bounded(Command::new("/bin/sleep").arg("5"), std::time::Duration::from_millis(50)).unwrap_err();
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
    fn local_switch_refuses_dirty_checkout_and_worktree_refuses_unknown_base() {
        let repo = repository();
        let storage = tempfile::tempdir().unwrap();
        let local = LaunchOptions {
            branch: Some("feature".into()),
            ..Default::default()
        };
        std::fs::write(repo.path().join("dirty"), "keep").unwrap();
        assert!(prepare(repo.path(), &local, storage.path()).is_err());
        std::fs::remove_file(repo.path().join("dirty")).unwrap();
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
