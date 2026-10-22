use alacritty_terminal::{
    event::VoidListener,
    grid::{Dimensions, Scroll},
    index::{Column, Line, Point},
    term::{Config, Term, TermMode, cell::Flags},
    vte::ansi::{Color, NamedColor, Processor},
};
use del_terminal::TerminalSize;

fn term() -> Term<VoidListener> {
    Term::new(
        Config {
            scrolling_history: 100,
            ..Config::default()
        },
        &TerminalSize {
            cols: 20,
            rows: 4,
            ..Default::default()
        },
        VoidListener,
    )
}

#[test]
fn parser_handles_fragmented_unicode_colors_and_cursor() {
    let mut t = term();
    let mut p: Processor = Processor::new();
    for b in "\x1b[31mR\x1b[38;5;196mI\x1b[38;2;1;2;3mT\x1b[0m界e\u{301}".as_bytes() {
        p.advance(&mut t, &[*b]);
    }
    assert_eq!(
        t.grid()[Point::new(Line(0), Column(0))].fg,
        Color::Named(NamedColor::Red)
    );
    assert_eq!(
        t.grid()[Point::new(Line(0), Column(1))].fg,
        Color::Indexed(196)
    );
    assert_eq!(
        t.grid()[Point::new(Line(0), Column(2))].fg,
        Color::Spec(alacritty_terminal::vte::ansi::Rgb { r: 1, g: 2, b: 3 })
    );
    assert!(
        t.grid()[Point::new(Line(0), Column(3))]
            .flags
            .contains(Flags::WIDE_CHAR)
    );
    assert!(
        t.grid()[Point::new(Line(0), Column(4))]
            .flags
            .contains(Flags::WIDE_CHAR_SPACER)
    );
    assert_eq!(
        t.grid()[Point::new(Line(0), Column(5))].zerowidth(),
        Some(&['\u{301}'][..])
    );
    p.advance(&mut t, b"\x1b[2;3HX");
    assert_eq!(t.grid()[Point::new(Line(1), Column(2))].c, 'X');
}

#[test]
fn alternate_screen_modes_and_scrollback() {
    let mut t = term();
    let mut p: Processor = Processor::new();
    p.advance(
        &mut t,
        b"main\x1b[?1049h\x1b[?2004h\x1b[?1002h\x1b[?1006halt",
    );
    assert!(t.mode().contains(
        TermMode::ALT_SCREEN
            | TermMode::BRACKETED_PASTE
            | TermMode::MOUSE_DRAG
            | TermMode::SGR_MOUSE
    ));
    p.advance(&mut t, b"\x1b[?1049l");
    assert!(!t.mode().contains(TermMode::ALT_SCREEN));
    assert_eq!(t.grid()[Point::new(Line(0), Column(0))].c, 'm');
    p.advance(&mut t, b"\r\none\r\ntwo\r\nthree\r\nfour\r\nfive");
    assert!(t.grid().history_size() >= 2);
    t.scroll_display(Scroll::Delta(2));
    assert_eq!(t.grid().display_offset(), 2);
    t.resize(TerminalSize {
        cols: 30,
        rows: 8,
        ..Default::default()
    });
    assert_eq!((t.columns(), t.screen_lines()), (30, 8));
}
