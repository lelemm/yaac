use super::*;
use std::time::Duration;

fn term(cols: usize, rows: usize) -> Terminal {
    Terminal::new(cols, rows, || {})
}

fn settle(t: &Terminal) {
    let end = std::time::Instant::now() + Duration::from_secs(10);
    while !t.idle() {
        assert!(std::time::Instant::now() < end, "the terminal thread did not finish");
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn frame(t: &Terminal) -> Frame {
    settle(t);
    let mut f = Frame::default();
    t.snapshot(&mut f);
    f
}

fn text(f: &Frame) -> Vec<String> {
    f.lines.iter().map(|r| r.text().trim_end().to_owned()).collect()
}

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/fixtures/{name}.vt", env!("CARGO_MANIFEST_DIR"))).expect("fixture")
}

/// A fixture replayed at its recorded size (69 × 28).
fn replay(name: &str) -> (Terminal, Frame) {
    let t = term(69, 28);
    t.feed(&fixture(name));
    let f = frame(&t);
    (t, f)
}

// ---- recorded streams ----------------------------------------------------------------------

#[test]
fn bash_readline_prompt_with_colours() {
    let (_, f) = replay("bash-prompt");
    let lines = text(&f);
    assert_eq!(lines[0], "dev@devbox:~/work/auth$ ls -F --color=always");
    assert!(lines.iter().any(|l| l == "dev@devbox:~/work/auth$ cargo build --rel"));
    // The prompt is bold green, the path bold blue (PS1), `docs/` blue from ls.
    let c = f.lines[0].cells[0];
    assert_eq!((c.fg, c.flags.has(Flags::BOLD)), (Color::Index(2), true));
    assert_eq!(f.lines[0].cells[11].fg, Color::Index(4));
    assert!(f.modes.bracketed_paste, "bash asks for bracketed paste");
    assert!(!f.modes.alt_screen);
    assert_eq!((f.cursor.row, f.cursor.col), (17, 41));
    assert!(f.cursor.visible);
}

#[test]
fn vim_full_screen_on_the_alternate_screen() {
    let (_, f) = replay("vim");
    let lines = text(&f);
    assert!(f.modes.alt_screen && f.modes.app_cursor);
    assert_eq!(lines[0], "  1 use crate::session::{Session, Store};");
    assert!(lines[26].starts_with("src/app.rs") && lines[26].contains("20,9"));
    // The status line is reverse video: resolved to swapped colours.
    assert_eq!(f.lines[26].cells[0].bg, Color::Fg);
    assert_eq!((f.cursor.row, f.cursor.col), (19, 12), "line 20, column 9, after the 4-column gutter");
}

#[test]
fn vim_session_returns_to_the_shell_screen() {
    let (_, f) = replay("vim-session");
    assert!(!f.modes.alt_screen, "vim left the alternate screen");
    assert!(!f.modes.app_cursor);
}

#[test]
fn less_pages_and_quits() {
    let (_, f) = replay("less");
    assert!(!f.modes.alt_screen);
}

#[test]
fn htop_draws_meters_and_the_process_list() {
    let (_, f) = replay("htop");
    let lines = text(&f);
    assert!(f.modes.alt_screen);
    assert!(lines[5].contains("PID USER"));
    assert!(lines.iter().any(|l| l.contains("cargo test --workspace")));
    assert!(lines[27].starts_with("F1Help"));
    assert!(!f.cursor.visible, "htop hides the cursor");
    // The header row has a coloured background.
    assert_ne!(f.lines[5].cells[1].bg, Color::Bg);
}

#[test]
fn powershell_psreadline_prompt() {
    let (_, f) = replay("powershell-prompt");
    let lines = text(&f);
    assert_eq!(lines[0], "PS ~/work/auth> Get-ChildItem -Name src");
    assert_eq!(lines[6], "PS ~/work/auth> $PSVersionTable.PSVersion.Major");
    assert_eq!(lines[8], "PS ~/work/auth> Get-Content README.md | Select-String 'cargo'");
    // PSReadLine colours the command (bright yellow) and the string (cyan) as it is typed.
    assert_eq!(f.lines[8].cells[16].fg, Color::Index(11));
    assert_eq!(f.lines[8].cells[54].fg, Color::Index(6));
}

// ---- snapshots and damage ------------------------------------------------------------------

#[test]
fn snapshot_damage_is_exactly_the_changed_rows() {
    let t = term(40, 10);
    t.feed(b"line one\r\nline two\r\n$ ");
    let mut f = Frame::default();
    settle(&t);
    assert!(t.snapshot(&mut f));
    assert!(f.full, "the first frame is full");
    assert_eq!(f.damaged.len(), 10);
    // Nothing new: nothing to draw.
    assert!(!t.snapshot(&mut f));
    assert!(f.damaged.is_empty());
    // An echo changes one row only.
    t.feed(b"x");
    settle(&t);
    assert!(t.snapshot(&mut f));
    assert!(!f.full);
    assert_eq!(f.damaged, vec![2]);
    assert_eq!(f.lines[2].text().trim_end(), "$ x");
    // Moving the cursor alone changes no cells, but the frame changed.
    t.feed(b"\x1b[D");
    settle(&t);
    assert!(t.snapshot(&mut f));
    assert!(f.damaged.is_empty());
    assert_eq!(f.cursor.col, 2);
}

#[test]
fn scrolling_output_damages_every_row_that_moved() {
    let t = term(20, 4);
    t.feed(b"a\r\nb\r\nc\r\nd");
    let mut f = Frame::default();
    settle(&t);
    t.snapshot(&mut f);
    t.feed(b"\r\ne");
    settle(&t);
    t.snapshot(&mut f);
    assert_eq!(f.damaged, vec![0, 1, 2, 3]);
    assert_eq!(text(&f), ["b", "c", "d", "e"]);
    assert_eq!(f.history, 1);
}

#[test]
fn small_output_is_parsed_at_once_for_the_next_frame() {
    // An echo is parsed on the caller's thread: no wait before the snapshot shows it.
    let t = term(20, 2);
    t.feed(b"$ ");
    let mut f = Frame::default();
    t.snapshot(&mut f);
    t.feed(b"l");
    assert!(t.idle());
    t.snapshot(&mut f);
    assert_eq!(f.lines[0].text().trim_end(), "$ l");
}

#[test]
fn hidden_attributes_and_wide_characters() {
    let t = term(20, 2);
    t.feed("\x1b[7minv\x1b[0m \x1b[8msecret\x1b[0m 中\x1b[2mdim\x1b[9ms\x1b[0m".as_bytes());
    let f = frame(&t);
    let r = &f.lines[0];
    assert_eq!((r.cells[0].fg, r.cells[0].bg), (Color::Bg, Color::Fg), "inverse is resolved");
    assert_eq!(r.cells[4].ch, ' ', "hidden text is blanked");
    assert!(r.cells[11].flags.has(Flags::WIDE) && r.cells[12].flags.has(Flags::SPACER));
    assert!(r.cells[13].flags.has(Flags::DIM));
    assert!(r.cells[16].flags.has(Flags::STRIKE));
    assert_eq!(r.text().trim_end(), "inv        中dims");
}

#[test]
fn truecolor_and_256_colours() {
    let t = term(20, 2);
    t.feed(b"\x1b[38;2;1;2;3;48;5;200mX");
    let f = frame(&t);
    assert_eq!((f.lines[0].cells[0].fg, f.lines[0].cells[0].bg), (Color::Rgb(1, 2, 3), Color::Index(200)));
    assert_eq!(Theme::DARK.indexed(16), Rgb(0, 0, 0));
    assert_eq!(Theme::DARK.indexed(231), Rgb(255, 255, 255));
    assert_eq!(Theme::DARK.indexed(232), Rgb(8, 8, 8));
}

#[test]
fn runs_group_cells_by_style() {
    let t = term(10, 1);
    t.feed(b"ab\x1b[31mcd\x1b[0m");
    let f = frame(&t);
    let runs: Vec<_> = f.lines[0].runs().map(|r| (r.start, r.len, r.fg)).collect();
    assert_eq!(runs, [(0, 2, Color::Fg), (2, 2, Color::Index(1)), (4, 6, Color::Fg)]);
}

// ---- input ---------------------------------------------------------------------------------

#[test]
fn key_encoding_table() {
    let normal = Modes::default();
    let app = Modes { app_cursor: true, app_keypad: true, ..Modes::default() };
    let k = |key, mods, modes: &Modes| encode_key(key, mods, modes).unwrap();
    let ctrl = Mods::CTRL;
    let cases: &[(Key, Mods, &Modes, &[u8])] = &[
        (Key::Enter, Mods::NONE, &normal, b"\r"),
        (Key::Enter, Mods::ALT, &normal, b"\x1b\r"),
        (Key::Tab, Mods::NONE, &normal, b"\t"),
        (Key::Tab, Mods::SHIFT, &normal, b"\x1b[Z"),
        (Key::Backspace, Mods::NONE, &normal, b"\x7f"),
        (Key::Backspace, ctrl, &normal, b"\x08"),
        (Key::Backspace, Mods::ALT, &normal, b"\x1b\x7f"),
        (Key::Escape, Mods::NONE, &normal, b"\x1b"),
        (Key::Up, Mods::NONE, &normal, b"\x1b[A"),
        (Key::Up, Mods::NONE, &app, b"\x1bOA"),
        (Key::Left, ctrl, &normal, b"\x1b[1;5D"),
        (Key::Right, Mods { shift: true, alt: true, ..Mods::NONE }, &app, b"\x1b[1;4C"),
        (Key::Home, Mods::NONE, &normal, b"\x1b[H"),
        (Key::End, Mods::NONE, &app, b"\x1bOF"),
        (Key::Insert, Mods::NONE, &normal, b"\x1b[2~"),
        (Key::Delete, ctrl, &normal, b"\x1b[3;5~"),
        (Key::PageUp, Mods::NONE, &normal, b"\x1b[5~"),
        (Key::PageDown, Mods::SHIFT, &normal, b"\x1b[6;2~"),
        (Key::F(1), Mods::NONE, &normal, b"\x1bOP"),
        (Key::F(4), Mods::SHIFT, &normal, b"\x1b[1;2S"),
        (Key::F(5), Mods::NONE, &normal, b"\x1b[15~"),
        (Key::F(12), ctrl, &normal, b"\x1b[24;5~"),
        (Key::Char('c'), ctrl, &normal, b"\x03"),
        (Key::Char('C'), ctrl, &normal, b"\x03"),
        (Key::Char(' '), ctrl, &normal, b"\x00"),
        (Key::Char('['), ctrl, &normal, b"\x1b"),
        (Key::Char('/'), ctrl, &normal, b"\x1f"),
        (Key::Char('x'), Mods::ALT, &normal, b"\x1bx"),
        (Key::Char('b'), Mods { ctrl: true, alt: true, ..Mods::NONE }, &normal, b"\x1b\x02"),
        (Key::Keypad(KeypadKey::Digit(5)), Mods::NONE, &app, b"\x1bOu"),
        (Key::Keypad(KeypadKey::Enter), Mods::NONE, &app, b"\x1bOM"),
        (Key::Keypad(KeypadKey::Digit(5)), Mods::NONE, &normal, b"5"),
    ];
    for (key, mods, modes, want) in cases {
        assert_eq!(k(*key, *mods, modes), *want, "{key:?} {mods:?}");
    }
    let lnm = Modes { line_feed_new_line: true, ..Modes::default() };
    assert_eq!(k(Key::Enter, Mods::NONE, &lnm), b"\r\n");
}

#[test]
fn conpty_win32_input_mode_for_psreadline() {
    let t = term(20, 2);
    assert!(!t.modes().win32_input);
    // ConPTY asks for it in pieces that may split anywhere.
    t.feed(b"\x1b[?9001");
    t.feed(b";1004h");
    settle(&t);
    let m = t.modes();
    assert!(m.win32_input && m.focus_events);
    // Ctrl+Space and Ctrl+Backspace reach PSReadLine as exact key events.
    assert_eq!(t.key(Key::Char(' '), Mods::CTRL).unwrap(), b"\x1b[32;57;32;1;8;1_\x1b[32;57;32;0;8;1_");
    assert_eq!(t.key(Key::Backspace, Mods::CTRL).unwrap(), b"\x1b[8;14;127;1;8;1_\x1b[8;14;127;0;8;1_");
    // Arrows are enhanced keys.
    assert_eq!(t.key(Key::Up, Mods::NONE).unwrap(), b"\x1b[38;72;0;1;256;1_\x1b[38;72;0;0;256;1_");
    // Plain text stays text.
    assert_eq!(t.text("é"), "é".as_bytes());
    t.feed(b"\x1b[?9001l");
    settle(&t);
    assert_eq!(t.key(Key::Char(' '), Mods::CTRL).unwrap(), b"\x00");
}

#[test]
fn mouse_reports() {
    let off = Modes::default();
    assert_eq!(encode_mouse(MouseButton::Left, MouseAction::Press, 0, 0, Mods::NONE, &off), None);
    let sgr = Modes { mouse_click: true, sgr_mouse: true, ..Modes::default() };
    assert_eq!(encode_mouse(MouseButton::Left, MouseAction::Press, 4, 9, Mods::NONE, &sgr).unwrap(), b"\x1b[<0;10;5M");
    assert_eq!(encode_mouse(MouseButton::Left, MouseAction::Release, 4, 9, Mods::NONE, &sgr).unwrap(), b"\x1b[<0;10;5m");
    assert_eq!(encode_mouse(MouseButton::WheelUp, MouseAction::Press, 0, 0, Mods::CTRL, &sgr).unwrap(), b"\x1b[<80;1;1M");
    assert_eq!(encode_mouse(MouseButton::Left, MouseAction::Motion, 0, 0, Mods::NONE, &sgr), None, "clicks only");
    let drag = Modes { mouse_drag: true, ..sgr };
    assert_eq!(encode_mouse(MouseButton::Left, MouseAction::Motion, 1, 1, Mods::NONE, &drag).unwrap(), b"\x1b[<32;2;2M");
    assert_eq!(encode_mouse(MouseButton::None, MouseAction::Motion, 1, 1, Mods::NONE, &drag), None, "no button: only 1003");
    let x10 = Modes { mouse_click: true, ..Modes::default() };
    assert_eq!(encode_mouse(MouseButton::Right, MouseAction::Press, 0, 2, Mods::NONE, &x10).unwrap(), b"\x1b[M\x22\x23\x21");
    assert_eq!(encode_mouse(MouseButton::Right, MouseAction::Release, 0, 2, Mods::NONE, &x10).unwrap(), b"\x1b[M\x23\x23\x21");
}

#[test]
fn wheel_goes_to_the_program_or_the_scrollback() {
    let t = term(10, 3);
    for i in 0..10 {
        t.feed(format!("{i}\r\n").as_bytes());
    }
    settle(&t);
    // A shell: the view scrolls back.
    assert_eq!(t.wheel(2, 0, 0, Mods::NONE), None);
    let f = frame(&t);
    assert_eq!(f.display_offset, 2);
    // Typing returns to the bottom.
    t.text("x");
    assert_eq!(frame(&t).display_offset, 0);
    // less on the alternate screen: arrow keys.
    t.feed(b"\x1b[?1049h");
    settle(&t);
    assert_eq!(t.wheel(-2, 0, 0, Mods::NONE).unwrap(), b"\x1b[B\x1b[B");
    // A program that wants the mouse: wheel reports; Shift keeps it local.
    t.feed(b"\x1b[?1000h\x1b[?1006h");
    settle(&t);
    assert_eq!(t.wheel(1, 1, 2, Mods::NONE).unwrap(), b"\x1b[<64;3;2M");
}

// ---- clipboard (P339) ----------------------------------------------------------------------

#[test]
fn paste_normalises_line_endings_and_asks_only_without_brackets() {
    let plain = Modes::default();
    let p = encode_paste("a\r\nb\nc", &plain);
    assert_eq!(p.bytes, b"a\rb\rc");
    assert!(p.confirm && p.lines == 3);
    // One line with a trailing newline is not "several lines".
    assert!(!encode_paste("ls -l\n", &plain).confirm);
    let bracketed = Modes { bracketed_paste: true, ..Modes::default() };
    let p = encode_paste("echo 1\necho 2\x1b[201~rm -rf", &bracketed);
    assert_eq!(p.bytes, b"\x1b[200~echo 1\recho 2[201~rm -rf\x1b[201~", "ESC removed so the paste cannot end the bracket");
    assert!(!p.confirm, "bracketed paste never runs line by line");
    let full_screen = Modes { alt_screen: true, ..Modes::default() };
    assert!(!encode_paste("a\nb", &full_screen).confirm);
}

#[test]
fn clipboard_chords() {
    use Shortcut::{Copy, Paste, Send};
    let c = Key::Char('c');
    let v = Key::Char('v');
    let cs = Mods { ctrl: true, shift: true, ..Mods::NONE };
    assert_eq!(shortcut(c, Mods::CTRL, false, false), Send, "Ctrl+C with nothing selected interrupts");
    assert_eq!(shortcut(c, Mods::CTRL, true, false), Copy, "Ctrl+C with a selection copies");
    assert_eq!(shortcut(Key::Char('C'), cs, false, false), Copy);
    assert_eq!(shortcut(v, Mods::CTRL, false, false), Paste);
    assert_eq!(shortcut(v, Mods::CTRL, false, true), Send, "the setting sends Ctrl+V to the shell");
    assert_eq!(shortcut(Key::Char('V'), cs, false, true), Paste);
    assert_eq!(shortcut(Key::Insert, Mods::SHIFT, false, false), Paste);
    assert_eq!(shortcut(Key::Insert, Mods::CTRL, true, false), Copy);
    assert_eq!(shortcut(c, Mods { ctrl: true, alt: true, ..Mods::NONE }, true, false), Send);
}

#[test]
fn copied_text_is_clean() {
    // Soft-wrapped rows join, trailing spaces go, wide characters are not doubled.
    let t = term(10, 4);
    t.feed("0123456789abcdef\r\nend   \r\n中文 ok".as_bytes());
    settle(&t);
    t.select_all();
    assert_eq!(t.selection_text().unwrap(), "0123456789abcdef\nend\n中文 ok");
    // A partial selection across the wrap.
    t.select_start(0, 5, CellSide::Left, SelectKind::Simple);
    t.select_to(1, 2, CellSide::Right);
    assert_eq!(t.selection_text().unwrap(), "56789abc");
    // Word and line selection.
    t.select_start(3, 0, CellSide::Left, SelectKind::Word);
    assert_eq!(t.selection_text().unwrap(), "中文");
    t.select_start(2, 1, CellSide::Left, SelectKind::Line);
    assert_eq!(t.selection_text().unwrap(), "end", "no trailing newline: a pasted line does not run");
    t.clear_selection();
    assert_eq!(t.selection_text(), None);
}

#[test]
fn osc52_writes_the_clipboard_and_never_reads_it() {
    let t = term(20, 2);
    t.feed(b"\x1b]52;c;aGVsbG8=\x07");
    settle(&t);
    assert_eq!(t.take_events().clipboard.as_deref(), Some("hello"));
    t.feed(b"\x1b]52;c;?\x07");
    settle(&t);
    let ev = t.take_events();
    assert!(ev.input.is_empty(), "a read is refused: nothing is sent back");
    assert_eq!(ev.clipboard, None);
}

// ---- the rest ------------------------------------------------------------------------------

#[test]
fn title_bell_and_replies() {
    let t = term(20, 2);
    t.feed(b"\x1b]2;vim src/app.rs\x07\x07\x1b[6n");
    let f = frame(&t);
    assert_eq!(f.title.as_deref(), Some("vim src/app.rs"));
    let ev = t.take_events();
    assert!(ev.title_changed);
    assert_eq!(ev.bells, 1);
    assert_eq!(ev.input, b"\x1b[1;1R", "the cursor position report");
    // Colour queries are answered from the theme (vim asks for the background).
    t.set_theme(Theme::LIGHT, (8, 16));
    t.feed(b"\x1b]11;?\x07");
    settle(&t);
    assert_eq!(t.take_events().input, b"\x1b]11;rgb:ffff/ffff/ffff\x07");
}

#[test]
fn links_osc8_and_urls() {
    let t = term(30, 3);
    t.feed(b"see \x1b]8;;https://yaac.dev/docs\x07the docs\x1b]8;;\x07 or https://example.com/a_(b). ok");
    let f = frame(&t);
    let l = f.link_at(0, 6).unwrap();
    assert_eq!((l.uri.as_str(), l.cells.as_slice()), ("https://yaac.dev/docs", &[(0, 4, 11)][..]));
    assert!(f.lines[0].cells[5].flags.has(Flags::LINK));
    // A URL across the soft wrap, without the trailing full stop.
    let u = f.link_at(1, 2).unwrap();
    assert_eq!(u.uri, "https://example.com/a_(b)");
    assert_eq!(u.cells, [(0, 16, 29), (1, 0, 10)]);
    assert_eq!(f.link_at(0, 1), None);
}

#[test]
fn search_finds_and_highlights() {
    let t = term(30, 5);
    t.feed(b"alpha denied\r\nbeta ok\r\ngamma DENIED\r\n$ ");
    settle(&t);
    t.search("denied");
    let f = frame(&t);
    let s = f.search.unwrap();
    assert!(s.found);
    assert_eq!(s.matches.len(), 2, "case-insensitive for a lower-case query");
    assert_eq!(s.focused, Some(Span { start: (2, 6), end: (2, 11), block: false }), "the newest match first");
    t.search_next(true);
    let f = frame(&t);
    assert_eq!(f.search.unwrap().focused.unwrap().start, (0, 6));
    t.search("Denied");
    assert!(!frame(&t).search.unwrap().found);
    t.search_close();
    assert!(frame(&t).search.is_none());
}

#[test]
fn resize_reflows() {
    let mut t = term(10, 4);
    t.feed(b"0123456789abcdef");
    assert_eq!(text(&frame(&t))[..2], ["0123456789", "abcdef"]);
    t.resize(20, 4);
    assert_eq!(text(&frame(&t))[0], "0123456789abcdef");
    t.resize(5, 4);
    let f = frame(&t);
    assert_eq!((f.cols, f.rows), (5, 4));
    assert!(f.full);
}

#[test]
fn clear_keeps_the_prompt_line() {
    let t = term(20, 4);
    t.feed(b"a\r\nb\r\nc\r\n$ ls");
    settle(&t);
    t.clear();
    let f = frame(&t);
    assert_eq!(text(&f)[0], "$ ls");
    assert_eq!(f.history, 0);
    assert_eq!((f.cursor.row, f.cursor.col), (0, 4));
}

/// Parser throughput, MB/s (release: `cargo test -p yaac-term --release -- --ignored --nocapture`).
#[test]
#[ignore = "benchmark"]
fn throughput() {
    let mut chunk = Vec::new();
    for i in 0..2000 {
        chunk.extend_from_slice(format!("\x1b[32m{i:>6}\x1b[0m Line of ordinary output with \x1b[1mbold\x1b[0m and colour, 80 cols wide....\r\n").as_bytes());
    }
    let mut ls = Vec::new();
    while ls.len() < 100 << 20 {
        ls.extend_from_slice(&chunk);
    }
    let t = term(200, 60);
    let start = std::time::Instant::now();
    for c in ls.chunks(64 * 1024) {
        t.feed(c);
    }
    settle(&t);
    let secs = start.elapsed().as_secs_f64();
    println!("parsed {} MB in {secs:.2} s: {:.0} MB/s", ls.len() >> 20, (ls.len() >> 20) as f64 / secs);
    let mut f = Frame::default();
    let start = std::time::Instant::now();
    for _ in 0..100 {
        t.feed(b"x");
        t.snapshot(&mut f);
    }
    println!("echo + snapshot (200 × 60): {:.1} µs", start.elapsed().as_secs_f64() * 1e4);
}
