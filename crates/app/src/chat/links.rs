use std::path::{Path, PathBuf};

/// Where a transcript link may go.
#[derive(Debug, PartialEq, Eq)]
pub enum LinkTarget {
    /// A local file, shown in Del's file viewer.
    Local(PathBuf),
    /// A web or mail link, handed to the default browser/mail app.
    External(String),
    /// Any other scheme (custom app URLs, network shares, remote `file://`
    /// hosts, system settings…). Agent output is untrusted, so never launch it.
    Blocked,
}

/// Only these schemes leave Del; everything else could launch arbitrary apps.
const EXTERNAL_SCHEMES: [&str; 3] = ["http", "https", "mailto"];

pub fn classify(target: &str, cwd: &Path) -> LinkTarget {
    match url::Url::parse(target) {
        Ok(url) if url.scheme() == "file" => {
            // `to_file_path` rejects remote hosts; only `file:///` and `file://localhost/`.
            url.to_file_path()
                .map_or(LinkTarget::Blocked, LinkTarget::Local)
        }
        Ok(url) if EXTERNAL_SCHEMES.contains(&url.scheme()) => {
            LinkTarget::External(url.to_string())
        }
        Ok(_) => LinkTarget::Blocked,
        Err(_) => local_path(target, cwd).map_or(LinkTarget::Blocked, LinkTarget::Local),
    }
}

/// Resolve file links against the conversation folder; web links stay external.
pub fn local_path(target: &str, cwd: &Path) -> Option<PathBuf> {
    if let Ok(url) = url::Url::parse(target) {
        return (url.scheme() == "file")
            .then(|| url.to_file_path().ok())
            .flatten();
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
        assert_eq!(
            local_path("file:///tmp/my%20file.html#L4", cwd),
            Some(PathBuf::from("/tmp/my file.html"))
        );
        assert_eq!(
            local_path("/tmp/index.html", cwd),
            Some(PathBuf::from("/tmp/index.html"))
        );
        assert_eq!(local_path("https://example.com/index.html", cwd), None);
        assert_eq!(local_path("mailto:hello@example.com", cwd), None);
    }

    #[test]
    fn only_web_and_mail_links_open_outside_del() {
        let cwd = Path::new("/Users/example/project");
        assert_eq!(
            classify("https://example.com/a?b=1", cwd),
            LinkTarget::External("https://example.com/a?b=1".into())
        );
        assert_eq!(
            classify("HTTP://example.com/", cwd),
            LinkTarget::External("http://example.com/".into())
        );
        assert_eq!(
            classify("mailto:hello@example.com", cwd),
            LinkTarget::External("mailto:hello@example.com".into())
        );
        assert_eq!(
            classify("src/main.rs", cwd),
            LinkTarget::Local(cwd.join("src/main.rs"))
        );
        assert_eq!(
            classify("/tmp/index.html", cwd),
            LinkTarget::Local(PathBuf::from("/tmp/index.html"))
        );
        assert_eq!(
            classify("file:///tmp/a.txt", cwd),
            LinkTarget::Local(PathBuf::from("/tmp/a.txt"))
        );
        assert_eq!(
            classify("file://localhost/tmp/a.txt", cwd),
            LinkTarget::Local(PathBuf::from("/tmp/a.txt"))
        );
        for blocked in [
            "file://otherhost/share/a.txt",
            "smb://server/share",
            "vscode://file/etc/passwd",
            "x-apple.systempreferences:com.apple.preference.security",
            "javascript:alert(1)",
            "ftp://example.com/file",
            "ssh://example.com",
            "tel:123",
        ] {
            assert_eq!(classify(blocked, cwd), LinkTarget::Blocked, "{blocked}");
        }
    }
}
