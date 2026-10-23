//! Durable local file imports. Only supported image formats use image transport.
use del_agent::Attachment;
use std::path::Path;

pub fn import_file(source: &Path, directory: &Path) -> Result<Attachment, String> {
    let metadata = std::fs::metadata(source).map_err(|e| e.to_string())?;
    if !metadata.is_file() {
        return Err("Choose files, not folders.".into());
    }
    let name = source.file_name().ok_or("This file has no filename.")?;
    // A separate directory preserves filenames without collisions or overwrites.
    let destination = directory.join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&destination).map_err(|e| e.to_string())?;
    let saved = destination.join(name);
    if let Err(error) = std::fs::copy(source, &saved) {
        let _ = std::fs::remove_dir_all(&destination);
        return Err(error.to_string());
    }
    let format = image::ImageReader::open(&saved)
        .ok()
        .and_then(|reader| reader.with_guessed_format().ok())
        .and_then(|reader| reader.format());
    let mime = match format {
        Some(image::ImageFormat::Png) => "image/png",
        Some(image::ImageFormat::Jpeg) => "image/jpeg",
        Some(image::ImageFormat::WebP) => "image/webp",
        _ => file_mime(source),
    };
    Ok(Attachment {
        path: saved,
        mime: Some(mime.into()),
    })
}

fn file_mime(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "pdf" => "application/pdf",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "xls" => "application/vnd.ms-excel",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "md" | "markdown" => "text/markdown",
        "txt" | "log" => "text/plain",
        "csv" => "text/csv",
        "json" => "application/json",
        "html" | "htm" => "text/html",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "zip" => "application/zip",
        // Unpreviewable images and unknown formats remain ordinary file inputs.
        _ => "application/octet-stream",
    }
}

pub fn is_image(attachment: &Attachment) -> bool {
    matches!(
        attachment.mime.as_deref(),
        Some("image/png" | "image/jpeg" | "image/webp")
    )
}

pub fn filename(attachment: &Attachment) -> String {
    attachment
        .path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

pub fn file_type(attachment: &Attachment) -> String {
    attachment
        .path
        .extension()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_uppercase())
        .unwrap_or_else(|| "FILE".into())
}

pub fn compact_filename(attachment: &Attachment) -> String {
    let filename = filename(attachment);
    let (base, extension) = filename
        .rsplit_once('.')
        .filter(|(base, ext)| !base.is_empty() && !ext.is_empty())
        .map_or((filename.as_str(), None), |(base, ext)| (base, Some(ext)));
    let name = if base.chars().count() > 5 {
        format!(
            "{}…",
            base.chars()
                .take(5)
                .collect::<String>()
                .trim_end_matches([' ', '.', '_', '-'])
        )
    } else {
        base.to_string()
    };
    extension.map_or(name.clone(), |ext| format!("{name}.{ext}"))
}

pub fn file_mark(attachment: &Attachment) -> Option<&'static str> {
    match file_type(attachment).as_str() {
        "PDF" => Some("icons/filetypes/pdf.png"),
        "DOC" | "DOCX" => Some("icons/filetypes/docx.png"),
        "XLS" | "XLSX" => Some("icons/filetypes/xlsx.png"),
        "PPT" | "PPTX" => Some("icons/filetypes/pptx.png"),
        _ => None,
    }
}

pub fn terminal_prompt(text: &str, attachments: &[Attachment]) -> String {
    let mut prompt = text.to_owned();
    for attachment in attachments {
        prompt.push_str("\n[Attached file: ");
        prompt.push_str(&serde_json::to_string(&attachment.path.to_string_lossy()).unwrap());
        prompt.push(']');
    }
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_cards_preserve_extensions_and_unicode_boundaries() {
        let attachment = |name: &str| Attachment {
            path: name.into(),
            mime: None,
        };
        assert_eq!(
            compact_filename(&attachment("research-paper.pdf")),
            "resea….pdf"
        );
        assert_eq!(
            compact_filename(&attachment("你好世界笔记.docx")),
            "你好世界笔….docx"
        );
        assert_eq!(compact_filename(&attachment(".env")), ".env");
        assert_eq!(
            file_mark(&attachment("REPORT.PDF")),
            Some("icons/filetypes/pdf.png")
        );
        assert_eq!(file_mark(&attachment("movie.mp4")), None);
    }

    #[test]
    fn arbitrary_files_preserve_names_bytes_and_survive_source_removal() {
        let source = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();
        for name in [
            "report.pdf",
            "book.xlsx",
            "notes.md",
            "clip.mp4",
            "archive.zip",
            "unknown.xyz",
            "README",
        ] {
            let path = source.path().join(name);
            std::fs::write(&path, b"original file bytes").unwrap();
            let first = import_file(&path, storage.path()).unwrap();
            let second = import_file(&path, storage.path()).unwrap();
            std::fs::remove_file(path).unwrap();
            assert_eq!(filename(&first), name);
            assert_eq!(std::fs::read(&first.path).unwrap(), b"original file bytes");
            assert_ne!(first.path, second.path);
            assert!(!is_image(&first));
        }
        assert!(import_file(source.path(), storage.path()).is_err());
    }

    #[test]
    fn images_are_detected_from_contents_and_documents_are_not_decoded() {
        let source = tempfile::tempdir().unwrap();
        let storage = tempfile::tempdir().unwrap();
        let path = source.path().join("picture.dat");
        image::RgbaImage::new(2, 2)
            .save_with_format(&path, image::ImageFormat::Png)
            .unwrap();
        let attachment = import_file(&path, storage.path()).unwrap();
        assert!(is_image(&attachment));
        assert_eq!(attachment.mime.as_deref(), Some("image/png"));
        assert_eq!(
            std::fs::read(path).unwrap(),
            std::fs::read(attachment.path).unwrap()
        );
    }

    #[test]
    fn terminal_prompt_keeps_file_paths_as_data_in_one_prompt() {
        let attachments = [Attachment {
            path: "/tmp/report \"final\".pdf".into(),
            mime: Some("application/pdf".into()),
        }];
        assert_eq!(
            terminal_prompt("Read this", &attachments),
            "Read this\n[Attached file: \"/tmp/report \\\"final\\\".pdf\"]"
        );
        assert_eq!(terminal_prompt("unchanged", &[]), "unchanged");
    }
}
