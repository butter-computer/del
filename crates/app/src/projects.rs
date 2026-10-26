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

fn git(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
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
        std::fs::write(
            &manifest,
            serde_json::to_vec_pretty(&owned).map_err(|e| e.to_string())?,
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
    pub collapsed: std::collections::HashSet<PathBuf>,
}
impl ProjectPreferences {
    pub fn load() -> Self {
        del_agent::data_dir()
            .and_then(|p| std::fs::read(p.join("projects.json")).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }
    pub fn save(&self) -> std::io::Result<()> {
        let Some(dir) = del_agent::data_dir() else {
            return Ok(());
        };
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("projects.json");
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(tmp, path)
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
