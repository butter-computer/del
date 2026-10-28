//! Locate transcript text in the terminal's rendered grid, including scrollback.
use alacritty_terminal::{
    grid::{Dimensions, Grid},
    index::{Column, Line, Point},
    term::cell::{Cell, Flags},
};
use std::ops::{Range, RangeInclusive};

/// Normalized text with one entry per source cell instead of one per byte:
/// `offsets[i]` is the byte offset where cell `points[i]` begins.
#[derive(Default)]
struct Text {
    value: String,
    offsets: Vec<usize>,
    points: Vec<Point>,
}
impl Text {
    fn push(&mut self, ch: char, point: Point) {
        let ch = if ch.is_whitespace() { ' ' } else { ch };
        if ch == ' ' && self.value.ends_with(' ') {
            return;
        }
        let start = self.value.len();
        self.value.extend(ch.to_lowercase());
        if self.value.len() > start {
            self.offsets.push(start);
            self.points.push(point);
        }
    }
    fn normalized(text: &str) -> String {
        let mut out = Self::default();
        for ch in text.chars() {
            out.push(ch, Point::new(Line(0), Column(0)));
        }
        out.value
    }
    /// Cell containing the byte at `offset`.
    fn point_at(&self, offset: usize) -> Point {
        let index = self.offsets.partition_point(|&start| start <= offset);
        self.points[index.saturating_sub(1)]
    }
}

#[cfg(test)]
pub fn locate(
    grid: &Grid<Cell>,
    source: &str,
    range: Range<usize>,
) -> Option<RangeInclusive<Point>> {
    if !searchable(source, range.clone()) {
        return None;
    }
    snapshot(grid).locate(source, range)
}

/// Whether `range` of `source` is a non-blank query worth scanning for.
pub fn searchable(source: &str, range: Range<usize>) -> bool {
    source
        .get(range)
        .is_some_and(|query| !Text::normalized(query).trim().is_empty())
}

/// Normalized grid text. Build it under the terminal lock, then search it
/// after releasing the lock.
pub struct Snapshot(Text);

pub fn snapshot(grid: &Grid<Cell>) -> Snapshot {
    let lines = (grid.bottommost_line().0 - grid.topmost_line().0 + 1).max(0) as usize;
    let mut text = Text {
        value: String::with_capacity(lines * 32),
        offsets: Vec::with_capacity(lines * 32),
        points: Vec::with_capacity(lines * 32),
    };
    for line in grid.topmost_line().0..=grid.bottommost_line().0 {
        let row = &grid[Line(line)];
        let wrapped = row[grid.last_column()].flags.contains(Flags::WRAPLINE);
        let end = if wrapped {
            grid.columns()
        } else {
            (0..grid.columns())
                .rfind(|&col| row[Column(col)].c != ' ')
                .map_or(0, |col| col + 1)
        };
        for col in 0..end {
            let cell = &row[Column(col)];
            if cell.flags.intersects(
                Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER | Flags::HIDDEN,
            ) {
                continue;
            }
            let point = Point::new(Line(line), Column(col));
            text.push(cell.c, point);
            if let Some(extra) = cell.zerowidth() {
                for &ch in extra {
                    text.push(ch, point);
                }
            }
        }
        if !wrapped {
            text.push(' ', Point::new(Line(line), grid.last_column()));
        }
    }
    Snapshot(text)
}

impl Snapshot {
    pub fn locate(&self, source: &str, range: Range<usize>) -> Option<RangeInclusive<Point>> {
        let query = Text::normalized(source.get(range.clone())?);
        if query.trim().is_empty() {
            return None;
        }
        let before = Text::normalized(&source[..range.start]);
        let after = Text::normalized(&source[range.end..]);
        let text = &self.0;
        // Nearby text disambiguates repeated short queries without depending on TUI decorations.
        let (start, _) = text.value.match_indices(&query).max_by_key(|(start, _)| {
            let prefix = text.value[..*start]
                .chars()
                .rev()
                .zip(before.chars().rev())
                .take_while(|(a, b)| a == b)
                .take(120)
                .count();
            let suffix = text.value[*start + query.len()..]
                .chars()
                .zip(after.chars())
                .take_while(|(a, b)| a == b)
                .take(120)
                .count();
            prefix + suffix
        })?;
        Some(text.point_at(start)..=text.point_at(start + query.len() - 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TerminalSize;
    use alacritty_terminal::{
        term::{Config, Term},
        vte::ansi::Processor,
    };
    struct Listener;
    impl alacritty_terminal::event::EventListener for Listener {}

    // Use the emulator itself so wrapping, scrollback, and Unicode cell widths are realistic.
    fn terminal(text: &str, cols: u16, rows: u16) -> Term<Listener> {
        let mut term = Term::new(
            Config::default(),
            &TerminalSize {
                cols,
                rows,
                ..Default::default()
            },
            Listener,
        );
        let mut parser: Processor = Processor::new();
        parser.advance(&mut term, text.as_bytes());
        term
    }
    #[test]
    fn finds_wrapped_match_in_scrollback() {
        let term = terminal("123456needle wraps here\r\nnext\r\nlast\r\nend", 10, 2);
        let found = locate(term.grid(), "needle wraps", 0..12).unwrap();
        assert!(found.start().line.0 < 0);
        assert!(found.end().line > found.start().line);
    }
    #[test]
    fn context_selects_correct_occurrence_and_unicode_cells() {
        let term = terminal("wrong MATCH here\r\nright match there\r\nİ界 hello", 40, 5);
        let found = locate(term.grid(), "right match there", 6..11).unwrap();
        assert_eq!(*found.start(), Point::new(Line(1), Column(6)));
        let found = locate(term.grid(), "İ界", 0.."İ界".len()).unwrap();
        assert_eq!(*found.start(), Point::new(Line(2), Column(0)));
        assert_eq!(*found.end(), Point::new(Line(2), Column(1)));
        assert!(locate(term.grid(), "absent", 0..6).is_none());
    }
}
