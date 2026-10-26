//! Local file editing and read-only Git review used by the workspace sidebar.
use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

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

fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    Ok(output.stdout)
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
    let status =
        String::from_utf8(status).map_err(|_| "Git paths contain unsupported text encoding")?;
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

pub fn diff(root: &Path, file: &ChangedFile) -> Result<String, String> {
    if file.status == "??" {
        let path = root.join(&file.path);
        let meta = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        let text = if meta.file_type().is_symlink() {
            std::fs::read_link(&path)
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .into_owned()
        } else {
            if meta.len() > 512 * 1024 {
                return Ok("New file is too large to preview.".into());
            }
            match std::fs::read_to_string(&path) {
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
    let head = git(root, &["rev-parse", "--verify", "HEAD"]).is_ok();
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(root)
        .args(["diff", "--no-ext-diff", "--no-textconv", "--no-color"]);
    if head {
        command.arg("HEAD");
    } else {
        command.arg("--cached");
    }
    command.arg("--").arg(&file.path);
    if let Some(previous) = &file.previous {
        command.arg(previous);
    }
    let output = command.output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    if !head {
        let extra = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["diff", "--no-ext-diff", "--no-textconv", "--no-color", "--"])
            .arg(&file.path)
            .output()
            .map_err(|e| e.to_string())?;
        if !extra.status.success() {
            return Err(String::from_utf8_lossy(&extra.stderr).into_owned());
        }
        text.push_str(&String::from_utf8_lossy(&extra.stdout));
    }
    if text.is_empty() {
        text = "No textual diff (the index or file metadata may have changed).".into();
    }
    if text.len() > 512 * 1024 {
        let mut end = 512 * 1024;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str("\n… Diff preview truncated.\n");
    }
    Ok(text)
}

pub fn selected_context(label: &str, text: &str, range: std::ops::Range<usize>) -> String {
    let range = if range.is_empty() {
        0..text.len()
    } else {
        range
    };
    let mut start = range.start.min(text.len());
    while !text.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = range.end.min(text.len()).max(start);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let snippet = text.get(start..end).unwrap_or(text);
    let first = text[..start].bytes().filter(|b| *b == b'\n').count() + 1;
    let last = first
        + snippet
            .trim_end_matches('\n')
            .bytes()
            .filter(|b| *b == b'\n')
            .count();
    let fence = "`".repeat(
        snippet
            .split(|c| c != '`')
            .map(str::len)
            .max()
            .unwrap_or(0)
            .max(2)
            + 1,
    );
    format!("{label} (lines {first}–{last}):\n{fence}\n{snippet}\n{fence}")
}

#[cfg(test)]
mod tests {
    use super::*;
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
        assert!(
            diff(root, &changes(root).unwrap().1[0])
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
        assert!(diff(root, renamed).unwrap().contains("+new"));
        let untracked = files.iter().find(|file| file.status == "??").unwrap();
        assert_eq!(untracked.path, Path::new("new\nfile.txt"));
        assert!(diff(root, untracked).unwrap().contains("+new content"));
        assert!(changes(&tempfile::tempdir().unwrap().path()).is_err());
    }
    #[test]
    fn context_keeps_exact_selection_and_source_line_numbers() {
        let text = "first\n世界\n```\nlast\n";
        let start = text.find("世界").unwrap();
        let end = text.find("last").unwrap();
        let context = selected_context("File /tmp/code", text, start..end);
        assert!(context.contains("lines 2–3"));
        assert!(context.contains("````\n世界\n```\n\n````"));
    }
}
