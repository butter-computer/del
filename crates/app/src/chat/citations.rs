//! Presentation-only conversion of Codex file citations into local Markdown links.
use std::{ops::Range, path::Path};

pub const MARKER: &str = ":codex-file-citation{";

pub fn markdown(source: &str) -> String {
    with_range(source, 0..0).0
}

/// Also translate search-highlight offsets, which refer to the saved raw reply.
pub fn with_range(source: &str, range: Range<usize>) -> (String, Range<usize>) {
    if !source.contains(MARKER) {
        return (source.to_owned(), range);
    }
    let Ok(tree) = ::markdown::to_mdast(source, &::markdown::ParseOptions::gfm()) else {
        return (source.to_owned(), range);
    };
    let mut protected = Vec::new();
    protected_ranges(&tree, &mut protected);
    let mut replacements = Vec::new();
    let mut consumed = 0;
    for (offset, _) in source.match_indices(MARKER) {
        let start = source[..offset].trim_end_matches(':').len();
        if start < consumed
            || source[..start]
                .bytes()
                .rev()
                .take_while(|b| *b == b'\\')
                .count()
                % 2
                != 0
        {
            continue;
        }
        let attributes_start = offset + MARKER.len();
        let Some((length, path)) = attributes(&source[attributes_start..]) else {
            continue;
        };
        let end = attributes_start + length;
        if protected.iter().any(|r| start < r.end && end > r.start) {
            continue;
        }
        // Only absolute local paths become attachments. Never promote arbitrary
        // URL schemes from model output into clickable application links.
        let path = Path::new(&path);
        let (Ok(url), Some(name)) = (url::Url::from_file_path(path), path.file_name()) else {
            continue;
        };
        let name = name.to_string_lossy();
        let mut label = String::new();
        for ch in name.chars() {
            if matches!(ch, '\\' | '[' | ']' | '`' | '*' | '_' | '<' | '>') {
                label.push('\\');
            }
            label.push(ch);
        }
        let link = format!("[{label}](<{url}>)");
        replacements.push((start..end, link));
        consumed = end;
    }
    let map_offset = |offset: usize, end: bool| {
        let mut shift = 0isize;
        for (old, new) in &replacements {
            if offset <= old.start {
                break;
            }
            if offset < old.end {
                return (old.start as isize + shift) as usize + if end { new.len() } else { 0 };
            }
            shift += new.len() as isize - old.len() as isize;
        }
        (offset as isize + shift) as usize
    };
    let range = map_offset(range.start, false)..map_offset(range.end, true);
    let mut rendered = String::with_capacity(source.len());
    let mut cursor = 0;
    for (old, new) in replacements {
        rendered.push_str(&source[cursor..old.start]);
        rendered.push_str(&new);
        cursor = old.end;
    }
    rendered.push_str(&source[cursor..]);
    (rendered, range)
}

fn protected_ranges(node: &::markdown::mdast::Node, ranges: &mut Vec<Range<usize>>) {
    use ::markdown::mdast::Node;
    if matches!(
        node,
        Node::Code(_)
            | Node::InlineCode(_)
            | Node::Html(_)
            | Node::Link(_)
            | Node::LinkReference(_)
            | Node::Image(_)
            | Node::ImageReference(_)
            | Node::Definition(_)
    ) {
        if let Some(position) = node.position() {
            ranges.push(position.start.offset..position.end.offset);
        }
    } else if let Some(children) = node.children() {
        for child in children {
            protected_ranges(child, ranges);
        }
    }
}

/// Read quoted attributes; malformed or partially streamed directives stay text.
fn attributes(source: &str) -> Option<(usize, String)> {
    let mut rest = source;
    let mut path = None;
    loop {
        rest = rest.trim_start();
        if rest.starts_with('}') {
            return Some((source.len() - rest.len() + 1, path?));
        }
        let key_end = rest.find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))?;
        if key_end == 0 {
            return None;
        }
        let key = &rest[..key_end];
        rest = rest[key_end..].trim_start().strip_prefix('=')?.trim_start();
        let mut value = serde_json::Deserializer::from_str(rest).into_iter::<String>();
        let value_text = value.next()?.ok()?;
        rest = &rest[value.byte_offset()..];
        if key == "path" {
            if path.is_some() || value_text.chars().any(char::is_control) {
                return None;
            }
            path = Some(value_text);
        }
        if !rest.starts_with('}') && !rest.starts_with(char::is_whitespace) {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_output_file_citation_as_a_local_link() {
        let source = "Done.\n\n:codex-file-citation{path=\"/Users/nihal/output/pdf/Question-Bank-Answers.pdf\" purpose=\"output\"}";
        assert_eq!(
            markdown(source),
            "Done.\n\n[Question-Bank-Answers.pdf](<file:///Users/nihal/output/pdf/Question-Bank-Answers.pdf>)"
        );
    }
    #[test]
    fn preserves_code_examples_and_malformed_or_nonlocal_citations() {
        for source in [
            r#"`:codex-file-citation{path="/tmp/a.pdf"}`"#,
            "```text\n:codex-file-citation{path=\"/tmp/a.pdf\"}\n```",
            r#"\:codex-file-citation{path="/tmp/a.pdf"}"#,
            r#":codex-file-citation{path="/tmp/a.pdf""#,
            r#":codex-file-citation{path="javascript:alert(1)"}"#,
            r#":codex-file-citation{path="relative.pdf"}"#,
            r#":codex-file-citation{path="/tmp/a" path="/tmp/b"}"#,
        ] {
            assert_eq!(markdown(source), source);
        }
    }

    #[test]
    fn encodes_paths_and_escapes_link_labels() {
        let source = r#"::codex-file-citation{purpose="output" path="/tmp/Q [1] #é.pdf"}"#;
        let output = markdown(source);
        assert_eq!(
            output,
            r#"[Q \[1\] #é.pdf](<file:///tmp/Q%20[1]%20%23%C3%A9.pdf>)"#
        );
        let tree = ::markdown::to_mdast(&output, &::markdown::ParseOptions::gfm()).unwrap();
        let link = &tree.children().unwrap()[0].children().unwrap()[0];
        let ::markdown::mdast::Node::Link(link) = link else {
            panic!("expected file link")
        };
        assert_eq!(
            super::super::links::classify(&link.url, Path::new("/tmp")),
            super::super::links::LinkTarget::Local("/tmp/Q [1] #é.pdf".into())
        );
    }

    #[test]
    fn maps_search_highlights_after_multiple_citations() {
        let source = r#"See :codex-file-citation{path="/tmp/a.pdf"} and ::codex-file-citation{path="/tmp/b.pdf"}. Done."#;
        let start = source.find("Done").unwrap();
        let (rendered, range) = with_range(source, start..start + 4);
        assert_eq!(&rendered[range], "Done");
        assert!(!rendered.contains("codex-file-citation"));
        assert_eq!(rendered.matches("file:///").count(), 2);
    }
}
