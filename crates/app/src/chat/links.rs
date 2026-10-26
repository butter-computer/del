use std::path::{Path, PathBuf};

/// Resolve file links against the conversation folder; web links stay external.
pub fn local_path(target: &str, cwd: &Path) -> Option<PathBuf> {
    if let Ok(url) = url::Url::parse(target) {
        return (url.scheme() == "file").then(|| url.to_file_path().ok()).flatten();
    }
    let base = url::Url::from_directory_path(cwd).ok()?;
    base.join(target).ok()?.to_file_path().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resolves_transcript_file_links_without_launching_an_application() {
        let cwd = Path::new("/Users/example/project");
        assert_eq!(local_path("index.html", cwd), Some(cwd.join("index.html")));
        assert_eq!(local_path("file:///tmp/my%20file.html#L4", cwd), Some(PathBuf::from("/tmp/my file.html")));
        assert_eq!(local_path("/tmp/index.html", cwd), Some(PathBuf::from("/tmp/index.html")));
        assert_eq!(local_path("https://example.com/index.html", cwd), None);
        assert_eq!(local_path("mailto:hello@example.com", cwd), None);
    }
}
