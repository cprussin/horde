//! Interactive prompt entry: a bordered, multi-line input box with vi-mode
//! editing and persistent history, rendered with ratatui.
//!
//! Returns `Some(prompt)` on submit, `None` on an empty cancel (Ctrl-D with no
//! content); Ctrl-C exits the process directly with status 130.

mod history;

use std::io;
use std::path::Path;
use std::process;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Terminal;
use tui_textarea::{CursorMove, TextArea};

use history::History;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Insert,
    Normal,
}

/// Restores the terminal (raw mode + alternate screen) on drop, covering the
/// error and panic paths.  The submit/cancel paths restore explicitly before
/// returning so the launched session inherits a clean terminal.
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen)?;
        Ok(TerminalGuard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}

fn restore() {
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), LeaveAlternateScreen);
}

pub fn read_prompt(history_file: &Path) -> io::Result<Option<String>> {
    let mut hist = History::load(history_file);
    let guard = TerminalGuard::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;

    let mut textarea = TextArea::default();
    let mut mode = Mode::Insert;
    style(&mut textarea, mode);

    let result = loop {
        terminal.draw(|frame| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(3), Constraint::Length(1)])
                .split(frame.area());
            frame.render_widget(&textarea, chunks[0]);
            frame.render_widget(hint(), chunks[1]);
        })?;

        let key = match event::read()? {
            Event::Key(k) if k.kind != KeyEventKind::Release => k,
            _ => continue,
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        // Bindings that apply regardless of mode.
        if ctrl && key.code == KeyCode::Char('c') {
            restore();
            println!();
            process::exit(130);
        }
        if ctrl && key.code == KeyCode::Char('d') {
            let text = text_of(&textarea);
            break if text.trim().is_empty() {
                None
            } else {
                Some(text)
            };
        }
        if ctrl && key.code == KeyCode::Char('p') {
            if let Some(s) = hist.prev() {
                set_text(&mut textarea, &s, mode);
            }
            continue;
        }
        if ctrl && key.code == KeyCode::Char('n') {
            let s = hist.next();
            set_text(&mut textarea, &s, mode);
            continue;
        }

        match mode {
            Mode::Insert => match key.code {
                KeyCode::Esc => {
                    mode = Mode::Normal;
                    style(&mut textarea, mode);
                }
                KeyCode::Enter => {
                    textarea.insert_newline();
                }
                _ => {
                    textarea.input(key);
                }
            },
            Mode::Normal => match key.code {
                KeyCode::Char('i') => enter_insert(&mut textarea, &mut mode, None),
                KeyCode::Char('a') => {
                    enter_insert(&mut textarea, &mut mode, Some(CursorMove::Forward))
                }
                KeyCode::Char('A') => enter_insert(&mut textarea, &mut mode, Some(CursorMove::End)),
                KeyCode::Char('I') => {
                    enter_insert(&mut textarea, &mut mode, Some(CursorMove::Head))
                }
                KeyCode::Char('o') => {
                    textarea.move_cursor(CursorMove::End);
                    textarea.insert_newline();
                    mode = Mode::Insert;
                    style(&mut textarea, mode);
                }
                KeyCode::Char('h') | KeyCode::Left => textarea.move_cursor(CursorMove::Back),
                KeyCode::Char('l') | KeyCode::Right => textarea.move_cursor(CursorMove::Forward),
                KeyCode::Char('w') => textarea.move_cursor(CursorMove::WordForward),
                KeyCode::Char('b') => textarea.move_cursor(CursorMove::WordBack),
                KeyCode::Char('0') => textarea.move_cursor(CursorMove::Head),
                KeyCode::Char('$') => textarea.move_cursor(CursorMove::End),
                KeyCode::Char('x') => {
                    textarea.delete_next_char();
                }
                // In Normal mode, vertical keys recall history.
                KeyCode::Char('k') | KeyCode::Up => {
                    if let Some(s) = hist.prev() {
                        set_text(&mut textarea, &s, mode);
                    }
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    let s = hist.next();
                    set_text(&mut textarea, &s, mode);
                }
                KeyCode::Enter => {
                    let text = text_of(&textarea);
                    if !text.trim().is_empty() {
                        break Some(text);
                    }
                }
                _ => {}
            },
        }
    };

    // Restore explicitly (then drop the guard, which is a no-op second restore)
    // so the caller's status output and the launched session start clean.
    restore();
    drop(guard);

    if let Some(prompt) = &result {
        hist.append(prompt, history_file);
    }
    Ok(result)
}

fn enter_insert(textarea: &mut TextArea, mode: &mut Mode, mv: Option<CursorMove>) {
    if let Some(m) = mv {
        textarea.move_cursor(m);
    }
    *mode = Mode::Insert;
    style(textarea, *mode);
}

fn text_of(textarea: &TextArea) -> String {
    textarea.lines().join("\n")
}

fn set_text(textarea: &mut TextArea, s: &str, mode: Mode) {
    let lines: Vec<String> = if s.is_empty() {
        vec![String::new()]
    } else {
        s.split('\n').map(str::to_string).collect()
    };
    *textarea = TextArea::new(lines);
    style(textarea, mode);
    textarea.move_cursor(CursorMove::Bottom);
    textarea.move_cursor(CursorMove::End);
}

fn style(textarea: &mut TextArea, mode: Mode) {
    let title = match mode {
        Mode::Insert => " horde · INSERT ",
        Mode::Normal => " horde · NORMAL ",
    };
    let dim = Style::default().add_modifier(Modifier::DIM);
    textarea.set_block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(dim)
            .title(title),
    );
    textarea.set_cursor_line_style(Style::default());
}

fn hint() -> Paragraph<'static> {
    let dim = Style::default().add_modifier(Modifier::DIM);
    Paragraph::new(Line::from(
        "  enter (normal): launch · i: insert · esc: normal · ^p/^n or ↑/↓: history · ^c: cancel",
    ))
    .style(dim)
}
