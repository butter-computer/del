//! Incremental reads of Claude's own title records; no inference from chat text.
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

#[derive(Default)]
struct TitleFile {
    offset: u64,
    generated: Option<String>,
    custom: Option<String>,
}

#[derive(Default)]
pub(crate) struct Titles(HashMap<PathBuf, TitleFile>);

impl Titles {
    pub fn read(&mut self, root: &Path, cwd: &Path, id: &str) -> std::io::Result<Option<String>> {
        // Session IDs are filenames, never paths from the provider.
        if uuid::Uuid::parse_str(id).is_err() {
            return Ok(None);
        }
        let project: String = cwd
            .to_string_lossy()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let projects = root.join("projects");
        let filename = format!("{id}.jsonl");
        let mut path = projects.join(project).join(&filename);
        if !path.is_file() {
            // Claude may canonicalize a symlinked workspace or encode a long path differently.
            let Ok(entries) = std::fs::read_dir(projects) else {
                return Ok(None);
            };
            let Some(found) = entries
                .flatten()
                .map(|e| e.path().join(&filename))
                .find(|p| p.is_file())
            else {
                return Ok(None);
            };
            path = found;
        }
        let mut file = std::fs::File::open(&path)?;
        let size = file.metadata()?.len();
        let state = self.0.entry(path).or_default();
        if size < state.offset {
            *state = TitleFile::default();
        }
        file.seek(SeekFrom::Start(state.offset))?;
        let mut reader = BufReader::new(file);
        let mut line = String::new();
        loop {
            line.clear();
            let count = reader.read_line(&mut line)?;
            if count == 0 || !line.ends_with('\n') {
                break;
            }
            state.offset += count as u64;
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            if value["sessionId"]
                .as_str()
                .is_some_and(|session| session != id)
            {
                continue;
            }
            let (field, slot) = match value["type"].as_str() {
                Some("custom-title") => ("customTitle", &mut state.custom),
                Some("ai-title") => ("aiTitle", &mut state.generated),
                _ => continue,
            };
            if let Some(title) = value[field]
                .as_str()
                .map(str::trim)
                .filter(|t| !t.is_empty())
            {
                *slot = Some(title.to_owned());
            }
        }
        Ok(state.custom.clone().or_else(|| state.generated.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn tracks_provider_titles_and_renames_without_using_message_text() {
        let root = std::env::temp_dir().join(format!("del-claude-titles-{}", uuid::Uuid::new_v4()));
        let project = root.join("projects/-tmp-project");
        std::fs::create_dir_all(&project).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let path = project.join(format!("{id}.jsonl"));
        let mut file = std::fs::File::create(&path).unwrap();
        writeln!(file, r#"{{"type":"user","message":{{"content":"hello"}}}}"#).unwrap();
        let mut titles = Titles::default();
        assert_eq!(
            titles.read(&root, Path::new("/tmp/project"), &id).unwrap(),
            None
        );
        writeln!(
            file,
            r#"{{"type":"ai-title","aiTitle":"Provider title","sessionId":"{id}"}}"#
        )
        .unwrap();
        assert_eq!(
            titles
                .read(&root, Path::new("/tmp/project"), &id)
                .unwrap()
                .as_deref(),
            Some("Provider title")
        );
        write!(file, r#"{{"type":"custom-title","customTitle":"Renamed"}}"#).unwrap();
        assert_eq!(
            titles
                .read(&root, Path::new("/tmp/project"), &id)
                .unwrap()
                .as_deref(),
            Some("Provider title")
        );
        writeln!(file).unwrap();
        writeln!(
            file,
            r#"{{"type":"ai-title","aiTitle":"Later generated title"}}"#
        )
        .unwrap();
        assert_eq!(
            titles
                .read(&root, Path::new("/tmp/project"), &id)
                .unwrap()
                .as_deref(),
            Some("Renamed")
        );
        assert_eq!(
            titles
                .read(&root, Path::new("/tmp/project"), "../wrong")
                .unwrap(),
            None
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
