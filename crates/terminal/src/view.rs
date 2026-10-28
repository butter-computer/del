//! GPUI entity and grid painting. Printable input uses EntityInputHandler so
//! the OS owns keyboard layout translation and IME composition.

use crate::{TerminalEvent, TerminalSession, TerminalSize, colors, keys, mouse};
use alacritty_terminal::{
    grid::{Dimensions, Scroll},
    index::{Column, Line, Point as GridPoint, Side},
    selection::{Selection, SelectionType},
    term::TermMode,
    term::cell::Flags,
    vte::ansi::{Color, CursorShape, NamedColor, Rgb},
};
use futures::StreamExt;
use gpui::{prelude::*, *};
use std::{ops::Range, time::Duration};

const FONT_SIZE: f32 = 13.;
const LINE_HEIGHT: f32 = 19.;
actions!(del_terminal, [Copy, Paste]);

/// Register once in the host application's initialization.
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-c", Copy, Some("Terminal")),
        KeyBinding::new("cmd-v", Paste, Some("Terminal")),
        KeyBinding::new("ctrl-shift-c", Copy, Some("Terminal")),
        KeyBinding::new("ctrl-shift-v", Paste, Some("Terminal")),
    ]);
}

pub struct TerminalView {
    session: TerminalSession,
    focus: FocusHandle,
    bounds: Bounds<Pixels>,
    cell_width: Pixels,
    selecting: bool,
    wheel_remainder: f32,
    composition: String,
    composition_selection: Range<usize>,
    title: Option<String>,
    exit: Option<Option<i32>>,
    search_flash: Option<std::ops::RangeInclusive<GridPoint>>,
    search_generation: u64,
    /// Forwards session events; dropped on close so the background shutdown
    /// does not wake this view after it was closed.
    events: std::cell::RefCell<Option<Task<()>>>,
    /// Treat Option as an ESC prefix. Set false to use OS Option characters.
    pub option_as_meta: bool,
}

impl EventEmitter<TerminalEvent> for TerminalView {}
impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TerminalView {
    /// Construct with `cx.new(|cx| TerminalView::new(session, window, cx))`.
    pub fn new(mut session: TerminalSession, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut events = session
            .take_events()
            .expect("terminal event receiver already taken");
        let focus = cx.focus_handle();
        cx.on_focus(&focus, window, |this, _, cx| {
            this.report_focus(true);
            cx.notify();
        })
        .detach();
        cx.on_blur(&focus, window, |this, _, cx| {
            this.report_focus(false);
            this.selecting = false;
            this.composition.clear();
            cx.notify();
        })
        .detach();
        let events = cx.spawn_in(window, async move |this, cx| {
            while let Some(event) = events.next().await {
                let exited = matches!(event, TerminalEvent::Exited { .. });
                if this
                    .update_in(cx, |this, window, cx| {
                        match &event {
                            TerminalEvent::Wakeup => {
                                this.session.ack_wakeup();
                            }
                            TerminalEvent::TitleChanged(title) => this.title = title.clone(),
                            TerminalEvent::Bell => window.play_system_bell(),
                            TerminalEvent::Input => {},
                            TerminalEvent::Exited { code } => this.exit = Some(*code),
                        }
                        cx.emit(event);
                        cx.notify();
                    })
                    .is_err()
                    || exited
                {
                    break;
                }
            }
        });
        Self {
            events: std::cell::RefCell::new(Some(events)),
            session,
            focus,
            bounds: Bounds::default(),
            cell_width: zoom_px(8.),
            selecting: false,
            wheel_remainder: 0.,
            composition: String::new(),
            composition_selection: 0..0,
            title: None,
            exit: None,
            search_flash: None,
            search_generation: 0,
            option_as_meta: true,
        }
    }

    pub fn session(&self) -> &TerminalSession {
        &self.session
    }
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.focus.focus(window, cx);
    }
    /// Non-blocking: the process tree is terminated and reaped off the UI thread.
    pub fn close(&self) {
        self.events.borrow_mut().take();
        self.session.close_in_background(Duration::from_millis(200));
    }

    /// Reveal a saved transcript match if it is present in the live terminal buffer.
    pub fn reveal_search_match(
        &mut self,
        source: &str,
        range: Range<usize>,
        cx: &mut Context<Self>,
    ) -> bool {
        if !crate::search::searchable(source, range.clone()) {
            return false;
        }
        // Only copying the grid text needs the lock; matching runs after it
        // is released so the PTY reader is not stalled by the search.
        let snapshot = self
            .session
            .with_term(|term| crate::search::snapshot(term.grid()));
        let Some(found) = snapshot.locate(source, range) else {
            return false;
        };
        self.session.with_term(|term| {
            let offset = (term.screen_lines() as i32 / 3 - found.start().line.0).max(0);
            term.scroll_display(Scroll::Bottom);
            term.scroll_display(Scroll::Delta(offset));
        });
        self.search_flash = Some(found);
        self.search_generation += 1;
        let generation = self.search_generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1400))
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.search_generation == generation {
                    this.search_flash = None;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
        true
    }

    fn report_focus(&self, focused: bool) {
        if self.session.mode().contains(TermMode::FOCUS_IN_OUT) {
            self.session.write(if focused {
                b"\x1b[I".to_vec()
            } else {
                b"\x1b[O".to_vec()
            });
        }
    }
    fn input(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) {
        if !bytes.is_empty() {
            cx.emit(TerminalEvent::Input);
        }
        self.session.with_term(|term| {
            term.selection = None;
            term.scroll_display(Scroll::Bottom);
        });
        self.search_flash = None;
        self.session.write(bytes);
        cx.notify();
    }
    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.composition.is_empty() {
            return;
        }
        let k = &event.keystroke;
        if k.modifiers.shift && !k.modifiers.control && !k.modifiers.alt && !k.modifiers.platform {
            let scroll = match k.key.as_str() {
                "pageup" => Some(Scroll::PageUp),
                "pagedown" => Some(Scroll::PageDown),
                "home" => Some(Scroll::Top),
                "end" => Some(Scroll::Bottom),
                _ => None,
            };
            if let Some(scroll) = scroll {
                self.session.with_term(|t| t.scroll_display(scroll));
                cx.notify();
                cx.stop_propagation();
                return;
            }
        }
        if let Some(bytes) = keys::encode_key(k, self.session.mode(), self.option_as_meta) {
            self.input(bytes, cx);
            cx.stop_propagation();
        }
    }
    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.session.with_term(|t| t.selection_to_string()) {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }
    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) {
            self.input(keys::encode_paste(&text, self.session.mode()), cx);
        }
    }
    fn cell_at(&self, position: Point<Pixels>) -> (usize, usize, Side) {
        let x = ((position.x - self.bounds.left()) / self.cell_width).max(0.);
        let y = ((position.y - self.bounds.top()) / zoom_px(LINE_HEIGHT)).max(0.);
        let size = self.session.size();
        (
            (x as usize).min(size.cols.saturating_sub(1) as usize),
            (y as usize).min(size.rows.saturating_sub(1) as usize),
            if x.fract() < 0.5 {
                Side::Left
            } else {
                Side::Right
            },
        )
    }
    fn report_mouse(
        &self,
        action: mouse::MouseAction,
        button: mouse::MouseButton,
        position: Point<Pixels>,
        modifiers: Modifiers,
    ) -> bool {
        let mode = self.session.mode();
        if modifiers.shift || !mouse::reporting(mode) {
            return false;
        }
        let (col, row, _) = self.cell_at(position);
        if let Some(bytes) = mouse::encode_mouse(
            action,
            button,
            col,
            row,
            mouse::MouseMods {
                shift: modifiers.shift,
                alt: modifiers.alt,
                ctrl: modifiers.control,
            },
            mode,
        ) {
            self.session.write(bytes);
        }
        true
    }
    fn mouse_down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus(window, cx);
        cx.stop_propagation();
        if self.report_mouse(
            mouse::MouseAction::Press,
            mouse_button(Some(e.button)),
            e.position,
            e.modifiers,
        ) {
            return;
        }
        if e.button != MouseButton::Left {
            return;
        }
        let (col, row, side) = self.cell_at(e.position);
        self.selecting = true;
        self.session.with_term(|term| {
            let point = GridPoint::new(
                Line(row as i32 - term.grid().display_offset() as i32),
                Column(col),
            );
            let ty = match e.click_count {
                2 => SelectionType::Semantic,
                3.. => SelectionType::Lines,
                _ => SelectionType::Simple,
            };
            if e.modifiers.shift
                && let Some(selection) = term.selection.as_mut()
            {
                selection.update(point, side);
            } else {
                term.selection = Some(Selection::new(ty, point, side));
            }
        });
        cx.notify();
    }
    fn mouse_up(&mut self, e: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.report_mouse(
            mouse::MouseAction::Release,
            mouse_button(Some(e.button)),
            e.position,
            e.modifiers,
        );
        self.selecting = false;
        cx.notify();
    }
    fn mouse_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.report_mouse(
            mouse::MouseAction::Motion,
            mouse_button(e.pressed_button),
            e.position,
            e.modifiers,
        ) {
            return;
        }
        if self.selecting && e.dragging() {
            let (col, row, side) = self.cell_at(e.position);
            self.session.with_term(|t| {
                let p = GridPoint::new(
                    Line(row as i32 - t.grid().display_offset() as i32),
                    Column(col),
                );
                if let Some(selection) = t.selection.as_mut() {
                    selection.update(p, side);
                }
            });
            cx.notify();
        }
    }
    fn scroll(&mut self, e: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.wheel_remainder += match e.delta {
            ScrollDelta::Lines(p) => p.y,
            ScrollDelta::Pixels(p) => f32::from(p.y) / zoom_px(LINE_HEIGHT).as_f32(),
        };
        let lines = self.wheel_remainder.trunc() as i32;
        self.wheel_remainder -= lines as f32;
        if lines == 0 {
            return;
        }
        let button = if lines > 0 {
            mouse::MouseButton::WheelUp
        } else {
            mouse::MouseButton::WheelDown
        };
        if mouse::reporting(self.session.mode()) && !e.modifiers.shift {
            for _ in 0..lines.unsigned_abs().min(100) {
                self.report_mouse(mouse::MouseAction::Press, button, e.position, e.modifiers);
            }
        } else if !e.modifiers.shift
            && let Some(bytes) = mouse::wheel_as_arrows(lines.clamp(-100, 100), self.session.mode())
        {
            self.session.write(bytes);
        } else {
            self.session
                .with_term(|t| t.scroll_display(Scroll::Delta(lines)));
        }
        cx.notify();
        cx.stop_propagation();
    }
}

fn mouse_button(button: Option<MouseButton>) -> mouse::MouseButton {
    match button {
        Some(MouseButton::Left) => mouse::MouseButton::Left,
        Some(MouseButton::Middle) => mouse::MouseButton::Middle,
        Some(MouseButton::Right) => mouse::MouseButton::Right,
        _ => mouse::MouseButton::None,
    }
}
fn color(c: Rgb) -> Hsla {
    rgb((u32::from(c.r) << 16) | (u32::from(c.g) << 8) | u32::from(c.b)).into()
}

fn transparent_background(background: Color, flags: Flags) -> bool {
    // Explicit colours, even black matching the default palette, are content.
    background == Color::Named(NamedColor::Background) && !flags.contains(Flags::INVERSE)
}

struct PaintCell {
    col: usize,
    row: usize,
    ch: char,
    /// Combining characters; empty (and unallocated) for nearly every cell.
    zerowidth: Vec<char>,
    fg: Hsla,
    bg: Option<Hsla>,
    flags: Flags,
}
struct Frame {
    cells: Vec<PaintCell>,
    cursor: Option<(usize, usize, CursorShape)>,
    width: Pixels,
}

impl TerminalView {
    fn prepare(&mut self, bounds: Bounds<Pixels>, window: &mut Window) -> Frame {
        self.bounds = bounds;
        let font = font("Menlo");
        self.cell_width = window
            .text_system()
            .advance(
                window.text_system().resolve_font(&font),
                zoom_px(FONT_SIZE),
                'M',
            )
            .map(|s| s.width)
            .unwrap_or(zoom_px(8.));
        let cols = ((bounds.size.width / self.cell_width).floor() as u16).max(2);
        let rows = ((bounds.size.height / zoom_px(LINE_HEIGHT)).floor() as u16).max(1);
        let _ = self.session.resize(TerminalSize {
            cols,
            rows,
            cell_width: f32::from(self.cell_width).ceil() as u16,
            cell_height: zoom_px(LINE_HEIGHT).as_f32().ceil() as u16,
        });
        self.session.with_term(|term| {
            let content = term.renderable_content();
            let mut cells = Vec::new();
            for indexed in content.display_iter {
                let row = indexed.point.line.0 + content.display_offset as i32;
                if row < 0 {
                    continue;
                }
                let cell = indexed.cell;
                let mut fg = colors::resolve(
                    if cell.flags.contains(Flags::BOLD) {
                        colors::bold_variant(cell.fg)
                    } else {
                        cell.fg
                    },
                    content.colors,
                );
                let mut bg = colors::resolve(cell.bg, content.colors);
                if cell.flags.contains(Flags::DIM) {
                    fg = Rgb {
                        r: fg.r / 2,
                        g: fg.g / 2,
                        b: fg.b / 2,
                    };
                }
                if cell.flags.contains(Flags::INVERSE) {
                    std::mem::swap(&mut fg, &mut bg);
                }
                let selected = content.selection.is_some_and(|s| s.contains(indexed.point));
                cells.push(PaintCell {
                    col: indexed.point.column.0,
                    row: row as usize,
                    ch: cell.c,
                    zerowidth: cell.zerowidth().map(<[char]>::to_vec).unwrap_or_default(),
                    fg: color(fg),
                    bg: if self
                        .search_flash
                        .as_ref()
                        .is_some_and(|range| range.contains(&indexed.point))
                    {
                        Some(rgba(0xe8b04a66).into())
                    } else if selected {
                        Some(rgba(0x43556fff).into())
                    } else if transparent_background(cell.bg, cell.flags) {
                        None
                    } else {
                        Some(color(bg))
                    },
                    flags: cell.flags,
                });
            }
            let c = content.cursor;
            let row = c.point.line.0 + content.display_offset as i32;
            let cursor = (row >= 0 && row < rows as i32 && c.shape != CursorShape::Hidden)
                .then_some((c.point.column.0, row as usize, c.shape));
            Frame {
                cells,
                cursor,
                width: self.cell_width,
            }
        })
    }
}

fn paint_text(
    text: String,
    origin: Point<Pixels>,
    fg: Hsla,
    flags: Flags,
    window: &mut Window,
    cx: &mut App,
) {
    let mut face = font("Menlo");
    if flags.contains(Flags::BOLD) {
        face = face.bold();
    }
    if flags.contains(Flags::ITALIC) {
        face = face.italic();
    }
    let run = TextRun {
        len: text.len(),
        font: face,
        color: fg,
        background_color: None,
        underline: flags
            .intersects(Flags::ALL_UNDERLINES)
            .then_some(UnderlineStyle {
                color: Some(fg),
                thickness: zoom_px(1.),
                wavy: flags.contains(Flags::UNDERCURL),
            }),
        strikethrough: flags
            .contains(Flags::STRIKEOUT)
            .then_some(StrikethroughStyle {
                color: Some(fg),
                thickness: zoom_px(1.),
            }),
    };
    let line = window
        .text_system()
        .shape_line(text.into(), zoom_px(FONT_SIZE), &[run], None);
    let _ = line.paint(
        origin,
        zoom_px(LINE_HEIGHT),
        TextAlign::Left,
        None,
        window,
        cx,
    );
}

struct BackgroundSpan {
    col: usize,
    row: usize,
    cols: usize,
    color: Hsla,
}

/// Merges horizontally adjacent cells sharing a background into one quad.
fn background_spans(cells: &[PaintCell]) -> Vec<BackgroundSpan> {
    let mut spans: Vec<BackgroundSpan> = Vec::new();
    for cell in cells {
        let Some(background) = cell.bg else {
            continue;
        };
        match spans.last_mut() {
            Some(span)
                if span.row == cell.row
                    && span.col + span.cols == cell.col
                    && span.color == background =>
            {
                span.cols += 1;
            }
            _ => spans.push(BackgroundSpan {
                col: cell.col,
                row: cell.row,
                cols: 1,
                color: background,
            }),
        }
    }
    spans
}

#[derive(Debug, PartialEq)]
struct TextSpan {
    col: usize,
    row: usize,
    text: String,
    fg: Hsla,
    flags: Flags,
}

/// Flags that change how a glyph is shaped or decorated.
const STYLE_FLAGS: Flags = Flags::BOLD
    .union(Flags::ITALIC)
    .union(Flags::ALL_UNDERLINES)
    .union(Flags::STRIKEOUT);

/// Groups consecutive cells into shaped runs. Only printable ASCII joins a
/// run: Menlo's advance for those equals the cell width, so the run stays on
/// the grid. Anything else (wide, combining, fallback-font glyphs) is painted
/// on its own at its cell origin. The block-cursor cell is drawn black.
fn text_spans(cells: &[PaintCell], block: Option<(usize, usize)>) -> Vec<TextSpan> {
    let mut spans: Vec<TextSpan> = Vec::new();
    // Whether the last span may still be extended by the next cell.
    let mut open = false;
    let finish = |spans: &mut Vec<TextSpan>, open: &mut bool| {
        if std::mem::take(open)
            && let Some(span) = spans.last_mut()
            && !span.flags.intersects(Flags::ALL_UNDERLINES | Flags::STRIKEOUT)
        {
            // Plain trailing spaces paint nothing.
            span.text.truncate(span.text.trim_end_matches(' ').len());
        }
    };
    for cell in cells {
        if cell
            .flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER | Flags::HIDDEN)
        {
            finish(&mut spans, &mut open);
            continue;
        }
        let flags = cell.flags & STYLE_FLAGS;
        let decorated = flags.intersects(Flags::ALL_UNDERLINES | Flags::STRIKEOUT);
        let fg = if block == Some((cell.col, cell.row)) {
            rgb(0).into()
        } else {
            cell.fg
        };
        let gridded = cell.zerowidth.is_empty()
            && !cell.flags.contains(Flags::WIDE_CHAR)
            && (cell.ch.is_ascii_graphic() || cell.ch == ' ');
        if gridded
            && open
            && let Some(span) = spans.last_mut()
            && span.row == cell.row
            && span.col + span.text.len() == cell.col
            && span.fg == fg
            && span.flags == flags
        {
            span.text.push(cell.ch);
            continue;
        }
        finish(&mut spans, &mut open);
        if cell.ch == ' ' && cell.zerowidth.is_empty() && !decorated {
            continue;
        }
        let mut text = String::from(cell.ch);
        text.extend(&cell.zerowidth);
        spans.push(TextSpan {
            col: cell.col,
            row: cell.row,
            text,
            fg,
            flags,
        });
        open = gridded;
    }
    finish(&mut spans, &mut open);
    spans
}

impl Render for TerminalView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let before = cx.entity();
        let painted = before.clone();
        let mut root = div()
            .id("terminal")
            .key_context("Terminal")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .min_h_0()
            .overflow_hidden()
            .cursor(CursorStyle::IBeam)
            .on_key_down(cx.listener(Self::key_down))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::paste))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_scroll_wheel(cx.listener(Self::scroll))
            .child(
                div().flex_1().min_h_0().overflow_hidden().p_3().child(
                    canvas(
                        move |bounds, window, cx| {
                            before.update(cx, |this, _| this.prepare(bounds, window))
                        },
                        move |bounds, frame, window, cx| {
                            let view = painted.read(cx);
                            let focus = view.focus.clone();
                            let composition = view.composition.clone();
                            let focused = focus.is_focused(window) && window.is_window_active();
                            window.handle_input(
                                &focus,
                                ElementInputHandler::new(bounds, painted.clone()),
                                cx,
                            );
                            let cursor = frame.cursor.filter(|_| focused);
                            // Paint every background before glyphs: a wide glyph
                            // extends into its spacer cell.
                            for span in background_spans(&frame.cells) {
                                let origin = bounds.origin
                                    + point(
                                        frame.width * span.col as f32,
                                        zoom_px(LINE_HEIGHT * span.row as f32),
                                    );
                                window.paint_quad(fill(
                                    Bounds::new(
                                        origin,
                                        size(frame.width * span.cols as f32, zoom_px(LINE_HEIGHT)),
                                    ),
                                    span.color,
                                ));
                            }
                            let block = cursor
                                .filter(|&(_, _, shape)| shape == CursorShape::Block)
                                .map(|(col, row, _)| (col, row));
                            if let Some((col, row)) = block {
                                let wide = frame.cells.iter().any(|cell| {
                                    cell.col == col
                                        && cell.row == row
                                        && cell.flags.contains(Flags::WIDE_CHAR)
                                });
                                let origin = bounds.origin
                                    + point(
                                        frame.width * col as f32,
                                        zoom_px(LINE_HEIGHT * row as f32),
                                    );
                                let cells = if wide { 2. } else { 1. };
                                window.paint_quad(fill(
                                    Bounds::new(
                                        origin,
                                        size(frame.width * cells, zoom_px(LINE_HEIGHT)),
                                    ),
                                    color(colors::CURSOR),
                                ));
                            }
                            for span in text_spans(&frame.cells, block) {
                                let origin = bounds.origin
                                    + point(
                                        frame.width * span.col as f32,
                                        zoom_px(LINE_HEIGHT * span.row as f32),
                                    );
                                paint_text(span.text, origin, span.fg, span.flags, window, cx);
                            }
                            if let Some((col, row, shape)) = cursor {
                                let origin = bounds.origin
                                    + point(
                                        frame.width * col as f32,
                                        zoom_px(LINE_HEIGHT * row as f32),
                                    );
                                if focused
                                    && matches!(shape, CursorShape::Beam | CursorShape::Underline)
                                {
                                    let b = if shape == CursorShape::Beam {
                                        Bounds::new(
                                            origin,
                                            size(zoom_px(1.0), zoom_px(LINE_HEIGHT)),
                                        )
                                    } else {
                                        Bounds::new(
                                            origin + point(zoom_px(0.), zoom_px(LINE_HEIGHT - 2.)),
                                            size(frame.width, zoom_px(2.)),
                                        )
                                    };
                                    window.paint_quad(fill(b, color(colors::CURSOR)));
                                }
                                if !composition.is_empty() {
                                    paint_text(
                                        composition.clone(),
                                        origin,
                                        rgb(0xffffff).into(),
                                        Flags::UNDERLINE,
                                        window,
                                        cx,
                                    );
                                }
                            }
                        },
                    )
                    .size_full(),
                ),
            );
        if let Some(code) = self.exit {
            root = root.child(
                div()
                    .h(zoom_px(24.))
                    .px_2()
                    .text_size(zoom_px(12.))
                    .text_color(rgb(0x999999))
                    .child(
                        code.map(|c| format!("Process exited with status {c}"))
                            .unwrap_or_else(|| "Process ended by signal".into()),
                    ),
            );
        }
        root
    }
}

// Ranges in this virtual input buffer are UTF-16 offsets, as required by GPUI.
fn byte_offset(text: &str, utf16: usize) -> usize {
    let mut count = 0;
    for (i, c) in text.char_indices() {
        if count + c.len_utf16() > utf16 {
            return i;
        }
        count += c.len_utf16();
    }
    text.len()
}
fn byte_range(text: &str, range: Range<usize>) -> Range<usize> {
    let start = byte_offset(text, range.start);
    start..byte_offset(text, range.end).max(start)
}

impl EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = byte_range(&self.composition, range);
        *actual = Some(
            self.composition[..range.start].encode_utf16().count()
                ..self.composition[..range.end].encode_utf16().count(),
        );
        Some(self.composition[range].to_string())
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.composition_selection.clone(),
            reversed: false,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        (!self.composition.is_empty()).then(|| 0..self.composition.encode_utf16().count())
    }
    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.composition.clear();
        self.composition_selection = 0..0;
        cx.notify();
    }
    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.composition.clear();
        self.composition_selection = 0..0;
        self.input(text.as_bytes().to_vec(), cx);
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = byte_range(
            &self.composition,
            range.unwrap_or(0..self.composition.encode_utf16().count()),
        );
        let start = self.composition[..range.start].encode_utf16().count();
        self.composition.replace_range(range, text);
        let len = text.encode_utf16().count();
        self.composition_selection = selected
            .map(|r| start + r.start..start + r.end)
            .unwrap_or(start + len..start + len);
        cx.notify();
    }
    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let (col, row) = self
            .session
            .with_term(|t| (t.grid().cursor.point.column.0, t.grid().cursor.point.line.0));
        Some(Bounds::new(
            self.bounds.origin
                + point(
                    self.cell_width * col as f32,
                    zoom_px(LINE_HEIGHT * row as f32),
                ),
            size(self.cell_width, zoom_px(LINE_HEIGHT)),
        ))
    }
    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.composition_selection.end)
    }
}

fn zoom_px(value: f32) -> Pixels {
    gpui::px(del_core::zoom::scaled(value))
}
#[cfg(test)]
mod tests {
    use super::{
        Color, Flags, Hsla, NamedColor, PaintCell, Rgb, byte_range, rgb, text_spans,
        transparent_background,
    };

    fn row(text: &str, row: usize, fg: Hsla) -> Vec<PaintCell> {
        text.chars()
            .enumerate()
            .map(|(col, ch)| PaintCell {
                col,
                row,
                ch,
                zerowidth: Vec::new(),
                fg,
                bg: None,
                flags: Flags::empty(),
            })
            .collect()
    }

    fn spans(cells: &[PaintCell], block: Option<(usize, usize)>) -> Vec<(usize, usize, String)> {
        text_spans(cells, block)
            .into_iter()
            .map(|span| (span.col, span.row, span.text))
            .collect()
    }

    #[test]
    fn ascii_runs_join_across_spaces_and_break_on_style_rows_and_cursor() {
        let white: Hsla = rgb(0xffffff).into();
        let mut cells = row("ls  -la   ", 0, white);
        cells.extend(row("ab", 1, white));
        assert_eq!(
            spans(&cells, None),
            [(0, 0, "ls  -la".into()), (0, 1, "ab".into())]
        );
        // The block cursor cell is drawn black, so it is its own run.
        assert_eq!(
            spans(&cells, Some((1, 0))),
            [
                (0, 0, "l".into()),
                (1, 0, "s".into()),
                (4, 0, "-la".into()),
                (0, 1, "ab".into())
            ]
        );
        cells[5].flags = Flags::BOLD;
        assert_eq!(
            spans(&cells[..7], None),
            [(0, 0, "ls  -".into()), (5, 0, "l".into()), (6, 0, "a".into())]
        );
    }

    #[test]
    fn wide_combining_and_hidden_cells_are_painted_alone_on_their_cell() {
        let white: Hsla = rgb(0xffffff).into();
        let mut cells = row("a界 bé!xc", 0, white);
        cells[1].flags = Flags::WIDE_CHAR;
        cells[2].flags = Flags::WIDE_CHAR_SPACER;
        cells[4].ch = 'e';
        cells[4].zerowidth = vec!['\u{301}'];
        cells[6].flags = Flags::HIDDEN;
        assert_eq!(
            spans(&cells, None),
            [
                (0, 0, "a".into()),
                (1, 0, "界".into()),
                (3, 0, "b".into()),
                (4, 0, "e\u{301}".into()),
                (5, 0, "!".into()),
                (7, 0, "c".into())
            ]
        );
        // Decorated spaces are content.
        let mut cells = row("a  ", 0, white);
        for cell in &mut cells {
            cell.flags = Flags::UNDERLINE;
        }
        assert_eq!(spans(&cells, None), [(0, 0, "a  ".into())]);
    }

    #[test]
    fn glass_only_replaces_default_background_not_explicit_colours_or_inverse() {
        assert!(transparent_background(
            Color::Named(NamedColor::Background),
            Flags::empty()
        ));
        for background in [
            Color::Named(NamedColor::Black),
            Color::Indexed(0),
            Color::Spec(Rgb { r: 0, g: 0, b: 0 }),
            Color::Spec(Rgb {
                r: 20,
                g: 80,
                b: 180,
            }),
        ] {
            assert!(!transparent_background(background, Flags::empty()));
        }
        assert!(!transparent_background(
            Color::Named(NamedColor::Background),
            Flags::INVERSE
        ));
    }

    #[test]
    fn ime_ranges_never_split_surrogates_or_utf8() {
        assert_eq!(byte_range("a😀界", 1..3), 1..5);
        assert_eq!(byte_range("a😀界", 2..4), 1..8);
        assert_eq!(byte_range("界", 99..100), 3..3);
    }
}
