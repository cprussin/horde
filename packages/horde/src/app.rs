//! The session multiplexer: a Turborepo-style TUI with a left sidebar of
//! running sessions (tagged local/remote) and a right pane rendering the
//! selected session's live terminal.  vi keys move the selection, Enter
//! focuses (input goes to the session), Ctrl-B blurs back to the list.

use std::collections::HashMap;
use std::io::{self, Stdout};
use std::sync::mpsc::{self, Sender};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use horde_proto::ServerFrame;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::{Frame, Terminal};
use tui_term::widget::PseudoTerminal;

use crate::config::Config;
use crate::discovery::{self, Host, SessionInfo};
use crate::host::{self, Decision};
use crate::session_conn::{make_hello, SessionConn};
use crate::{keys, router, tui};

pub type SessionId = String;

pub enum Msg {
    Frame(SessionId, ServerFrame),
    Closed(SessionId),
    Discovery(Vec<SessionInfo>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    List,
    Focused,
}

struct Connected {
    conn: SessionConn,
    parser: vt100::Parser,
    size: (u16, u16),
    exited: Option<i32>,
}

/// A session to create-and-focus at startup (from `horde --project`/prompt).
pub struct Initial {
    pub projects: Vec<String>,
    pub prompt: String,
    pub claude_args: Vec<String>,
    pub host: Host,
}

const SIDEBAR_W: u16 = 28;

type Backend = CrosstermBackend<Stdout>;

pub struct App {
    config: Config,
    remotes: Vec<String>,
    sessions: Vec<SessionInfo>,
    selected: usize,
    mode: Mode,
    conns: HashMap<SessionId, Connected>,
    tx: Option<Sender<Msg>>,
    term_size: (u16, u16),
    should_quit: bool,
    status: Option<String>,
}

impl App {
    pub fn new(config: Config, remotes: Vec<String>) -> App {
        App {
            config,
            remotes,
            sessions: Vec::new(),
            selected: 0,
            mode: Mode::List,
            conns: HashMap::new(),
            tx: None,
            term_size: crossterm::terminal::size().unwrap_or((80, 24)),
            should_quit: false,
            status: None,
        }
    }

    pub fn run(&mut self, initial: Option<Initial>) -> Result<()> {
        let mut terminal = enter_tui()?;
        let (tx, rx) = mpsc::channel();
        self.tx = Some(tx.clone());
        discovery::spawn_worker(
            self.remotes.clone(),
            self.connect_timeout(),
            Duration::from_secs(2),
            tx,
        );

        if let Some(init) = initial {
            let extras = init.projects[1..].to_vec();
            if let Err(e) = self.create_and_focus(
                init.host,
                init.projects[0].clone(),
                extras,
                &init.prompt,
                init.claude_args,
            ) {
                self.status = Some(format!("{e}"));
            }
        }

        let result = self.event_loop(&mut terminal, &rx);
        leave_tui(&mut terminal)?;
        result
    }

    fn connect_timeout(&self) -> u64 {
        self.config.connect_timeout
    }

    fn event_loop(
        &mut self,
        terminal: &mut Terminal<Backend>,
        rx: &mpsc::Receiver<Msg>,
    ) -> Result<()> {
        let frame = Duration::from_millis(33);
        let mut last_draw = Instant::now() - frame;
        let mut dirty = true;
        loop {
            if self.should_quit {
                return Ok(());
            }
            if dirty && last_draw.elapsed() >= frame {
                terminal.draw(|f| self.render(f))?;
                last_draw = Instant::now();
                dirty = false;
            }
            if event::poll(Duration::from_millis(16))? {
                match event::read()? {
                    Event::Key(k) if k.kind != KeyEventKind::Release => self.on_key(k, terminal)?,
                    Event::Resize(c, r) => {
                        self.term_size = (c, r);
                        self.resize_selected();
                    }
                    Event::Paste(s) => self.on_paste(s),
                    _ => {}
                }
                dirty = true;
            }
            while let Ok(msg) = rx.try_recv() {
                self.handle(msg);
                dirty = true;
            }
        }
    }

    // --- input ---------------------------------------------------------------

    fn on_key(&mut self, key: KeyEvent, terminal: &mut Terminal<Backend>) -> Result<()> {
        match self.mode {
            Mode::Focused => {
                if key.code == KeyCode::Char('b') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    self.mode = Mode::List;
                    return Ok(());
                }
                if let Some(id) = self.selected_id() {
                    if let Some(c) = self.conns.get(&id) {
                        c.conn.send_stdin(keys::encode(&key));
                    }
                }
            }
            Mode::List => self.on_list_key(key, terminal)?,
        }
        Ok(())
    }

    fn on_list_key(&mut self, key: KeyEvent, terminal: &mut Terminal<Backend>) -> Result<()> {
        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('j') | KeyCode::Down => self.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_selection(-1),
            KeyCode::Char('g') => self.set_selection(0),
            KeyCode::Char('G') => self.set_selection(self.row_count().saturating_sub(1)),
            KeyCode::Char('n') => self.new_session(terminal)?,
            KeyCode::Enter => {
                if self.selected == self.sessions.len() {
                    self.new_session(terminal)?;
                } else if self.ensure_connected(self.selected) {
                    self.mode = Mode::Focused;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn on_paste(&self, text: String) {
        if self.mode == Mode::Focused {
            if let Some(id) = self.selected_id() {
                if let Some(c) = self.conns.get(&id) {
                    c.conn.send_stdin(keys::encode_paste(&text));
                }
            }
        }
    }

    // --- selection -----------------------------------------------------------

    /// Rows = one per session plus the trailing "+ new session" entry.
    fn row_count(&self) -> usize {
        self.sessions.len() + 1
    }

    fn move_selection(&mut self, delta: isize) {
        let max = self.row_count().saturating_sub(1) as isize;
        let next = (self.selected as isize + delta).clamp(0, max);
        self.set_selection(next as usize);
    }

    fn set_selection(&mut self, idx: usize) {
        self.selected = idx.min(self.row_count().saturating_sub(1));
        // Browsing previews the session: connect lazily so the pane shows it.
        if self.selected < self.sessions.len() {
            self.ensure_connected(self.selected);
        }
    }

    fn selected_id(&self) -> Option<SessionId> {
        self.sessions
            .get(self.selected)
            .map(|s| key(&s.host, &s.meta.project))
    }

    // --- connections ---------------------------------------------------------

    /// Ensure the session at `idx` has a live connection; returns whether one
    /// exists afterward.
    fn ensure_connected(&mut self, idx: usize) -> bool {
        let Some(info) = self.sessions.get(idx).cloned() else {
            return false;
        };
        let id = key(&info.host, &info.meta.project);
        let (cols, rows) = self.pane_size();
        if let Some(c) = self.conns.get_mut(&id) {
            if c.size != (cols, rows) {
                c.conn.send_resize(cols, rows);
                c.parser.set_size(rows.max(1), cols.max(1));
                c.size = (cols, rows);
            }
            return true;
        }
        // Attach to an existing session: empty Hello (the daemon ignores
        // launch params for a running session, applies the size).
        let hello = make_hello(&info.meta.project, &info.meta.extras, "", &[], cols, rows);
        match SessionConn::connect(&info.host, hello, id.clone(), self.tx.clone().unwrap()) {
            Ok(conn) => {
                self.conns.insert(
                    id,
                    Connected {
                        conn,
                        parser: vt100::Parser::new(rows.max(1), cols.max(1), 0),
                        size: (cols, rows),
                        exited: None,
                    },
                );
                true
            }
            Err(e) => {
                self.status = Some(format!("connect failed: {e}"));
                false
            }
        }
    }

    fn create_and_focus(
        &mut self,
        host: Host,
        project: String,
        extras: Vec<String>,
        prompt: &str,
        claude_args: Vec<String>,
    ) -> Result<()> {
        let (cols, rows) = self.pane_size();
        let id = key(&host, &project);
        let hello = make_hello(&project, &extras, prompt, &claude_args, cols, rows);
        let conn = SessionConn::connect(&host, hello, id.clone(), self.tx.clone().unwrap())?;
        self.conns.insert(
            id.clone(),
            Connected {
                conn,
                parser: vt100::Parser::new(rows.max(1), cols.max(1), 0),
                size: (cols, rows),
                exited: None,
            },
        );
        self.upsert_session(SessionInfo {
            host,
            meta: horde_proto::SessionMeta {
                project,
                extras,
                ..Default::default()
            },
        });
        self.select_id(&id);
        self.mode = Mode::Focused;
        Ok(())
    }

    fn new_session(&mut self, terminal: &mut Terminal<Backend>) -> Result<()> {
        // The prompt box manages its own terminal, so drop out of ours first.
        leave_tui(terminal)?;
        let prompt = tui::read_prompt(&self.config.history_file);
        *terminal = enter_tui()?;
        terminal.clear()?;
        self.term_size = crossterm::terminal::size().unwrap_or(self.term_size);

        let prompt = match prompt {
            Ok(Some(p)) => p,
            Ok(None) => return Ok(()), // cancelled
            Err(e) => {
                self.status = Some(format!("prompt failed: {e}"));
                return Ok(());
            }
        };

        self.status = Some("routing…".to_string());
        let projects = match router::route(&self.config, &prompt) {
            Ok(p) if !p.is_empty() => p,
            Ok(_) => {
                self.status = Some("no project matched the request".to_string());
                return Ok(());
            }
            Err(e) => {
                self.status = Some(format!("routing failed: {e}"));
                return Ok(());
            }
        };
        let host = match host::pick_host(&self.config, None) {
            Ok(Decision::Local) => Host::Local,
            Ok(Decision::Remote) => Host::Remote(self.config.remote.clone().unwrap_or_default()),
            Err(e) => {
                self.status = Some(format!("{e}"));
                return Ok(());
            }
        };
        self.status = None;
        let extras = projects[1..].to_vec();
        self.create_and_focus(host, projects[0].clone(), extras, &prompt, Vec::new())
    }

    // --- message handling ----------------------------------------------------

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Frame(id, ServerFrame::Output(bytes)) => {
                if let Some(c) = self.conns.get_mut(&id) {
                    c.parser.process(&bytes);
                }
            }
            Msg::Frame(_, ServerFrame::Ready) => {}
            Msg::Frame(id, ServerFrame::Exit(code)) => {
                // The session ended; mark it and drop it from the list.
                if let Some(c) = self.conns.get_mut(&id) {
                    c.exited = Some(code);
                }
                self.remove_session(&id);
                self.mode = Mode::List;
            }
            Msg::Frame(id, ServerFrame::Error(e)) => {
                self.status = Some(format!("{}: {e}", short_id(&id)));
            }
            Msg::Closed(id) => {
                // Transport dropped; forget the connection so re-select
                // reconnects (the session may still be running).
                self.conns.remove(&id);
                if self.mode == Mode::Focused && self.selected_id().as_deref() == Some(id.as_str())
                {
                    self.mode = Mode::List;
                }
            }
            Msg::Discovery(snapshot) => self.merge_discovery(snapshot),
        }
    }

    fn merge_discovery(&mut self, mut snapshot: Vec<SessionInfo>) {
        let selected_key = self.selected_id();
        // Keep connected sessions that haven't shown up in the snapshot yet
        // (just-created, mid-startup).
        for (id, _) in self.conns.iter() {
            if !snapshot
                .iter()
                .any(|s| &key(&s.host, &s.meta.project) == id)
            {
                if let Some((host, project)) = split_id(id) {
                    snapshot.push(SessionInfo {
                        host,
                        meta: horde_proto::SessionMeta {
                            project,
                            ..Default::default()
                        },
                    });
                }
            }
        }
        snapshot.sort_by(|a, b| {
            (a.host.label(), &a.meta.project).cmp(&(b.host.label(), &b.meta.project))
        });
        self.sessions = snapshot;
        // Preserve the selection by identity.
        if let Some(k) = selected_key {
            if let Some(i) = self
                .sessions
                .iter()
                .position(|s| key(&s.host, &s.meta.project) == k)
            {
                self.selected = i;
            } else {
                self.selected = self.selected.min(self.sessions.len());
            }
        } else {
            self.selected = self.selected.min(self.sessions.len());
        }
    }

    fn upsert_session(&mut self, info: SessionInfo) {
        let id = key(&info.host, &info.meta.project);
        if !self
            .sessions
            .iter()
            .any(|s| key(&s.host, &s.meta.project) == id)
        {
            self.sessions.push(info);
        }
    }

    fn remove_session(&mut self, id: &str) {
        self.conns.remove(id);
        self.sessions
            .retain(|s| key(&s.host, &s.meta.project) != id);
        self.selected = self.selected.min(self.sessions.len());
    }

    fn select_id(&mut self, id: &str) {
        if let Some(i) = self
            .sessions
            .iter()
            .position(|s| key(&s.host, &s.meta.project) == id)
        {
            self.selected = i;
        }
    }

    fn resize_selected(&mut self) {
        if self.selected < self.sessions.len() {
            self.ensure_connected(self.selected);
        }
    }

    // --- layout / rendering --------------------------------------------------

    fn pane_size(&self) -> (u16, u16) {
        let (cols, rows) = self.term_size;
        let w = cols.saturating_sub(SIDEBAR_W).saturating_sub(2).max(1);
        let h = rows.saturating_sub(1).saturating_sub(2).max(1);
        (w, h)
    }

    fn render(&self, frame: &mut Frame) {
        let outer = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(1)])
            .split(frame.area());
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(SIDEBAR_W), Constraint::Min(0)])
            .split(outer[0]);

        self.render_sidebar(frame, cols[0]);
        self.render_pane(frame, cols[1]);
        self.render_keybar(frame, outer[1]);
    }

    fn render_sidebar(&self, frame: &mut Frame, area: Rect) {
        let mut items: Vec<ListItem> = self
            .sessions
            .iter()
            .map(|s| {
                let (tag, color) = match &s.host {
                    Host::Local => ("local".to_string(), Color::Green),
                    Host::Remote(h) => (h.clone(), Color::Cyan),
                };
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{tag} "), Style::default().fg(color)),
                    Span::raw(s.meta.project.clone()),
                ]))
            })
            .collect();
        items.push(ListItem::new(Line::from(Span::styled(
            "+ new session",
            Style::default().add_modifier(Modifier::DIM),
        ))));

        let focused = self.mode == Mode::Focused;
        let border = if focused {
            Style::default().add_modifier(Modifier::DIM)
        } else {
            Style::default()
        };
        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(border)
                    .title(" sessions "),
            )
            .highlight_style(
                Style::default()
                    .bg(if focused {
                        Color::DarkGray
                    } else {
                        Color::Blue
                    })
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▌");
        let mut state = ListState::default();
        state.select(Some(self.selected));
        frame.render_stateful_widget(list, area, &mut state);
    }

    fn render_pane(&self, frame: &mut Frame, area: Rect) {
        let focused = self.mode == Mode::Focused;
        let title = if self.selected < self.sessions.len() {
            let s = &self.sessions[self.selected];
            format!(" {} · {} ", s.host.label(), s.meta.project)
        } else {
            " new session ".to_string()
        };
        let border = if focused {
            Style::default().fg(Color::Blue)
        } else {
            Style::default()
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border)
            .title(title);
        let inner = block.inner(area);
        frame.render_widget(&block, area);

        if self.selected >= self.sessions.len() {
            frame.render_widget(placeholder("press enter to start a new session"), inner);
            return;
        }
        if inner.width < 20 || inner.height < 5 {
            frame.render_widget(placeholder("pane too small"), inner);
            return;
        }
        let id = key(
            &self.sessions[self.selected].host,
            &self.sessions[self.selected].meta.project,
        );
        match self.conns.get(&id) {
            Some(c) => {
                let screen = c.parser.screen();
                frame.render_widget(PseudoTerminal::new(screen), inner);
                if focused && !screen.hide_cursor() {
                    let (row, col) = screen.cursor_position();
                    frame.set_cursor_position((inner.x + col, inner.y + row));
                }
            }
            None => frame.render_widget(placeholder("connecting…"), inner),
        }
    }

    fn render_keybar(&self, frame: &mut Frame, area: Rect) {
        let text = if let Some(status) = &self.status {
            format!("  {status}")
        } else {
            match self.mode {
                Mode::List => "  j/k move · ↵ focus · n new · q quit".to_string(),
                Mode::Focused => "  ^B blur · keys go to the session".to_string(),
            }
        };
        frame.render_widget(
            Paragraph::new(Line::from(text)).style(Style::default().add_modifier(Modifier::DIM)),
            area,
        );
    }
}

fn placeholder(text: &str) -> Paragraph<'_> {
    Paragraph::new(text).style(Style::default().add_modifier(Modifier::DIM))
}

/// Identity key for a session: host label + project.
fn key(host: &Host, project: &str) -> SessionId {
    format!("{}\u{1f}{}", host.label(), project)
}

fn split_id(id: &str) -> Option<(Host, String)> {
    let (h, p) = id.split_once('\u{1f}')?;
    let host = if h == "local" {
        Host::Local
    } else {
        Host::Remote(h.to_string())
    };
    Some((host, p.to_string()))
}

fn short_id(id: &str) -> String {
    id.replace('\u{1f}', "/")
}

fn enter_tui() -> io::Result<Terminal<Backend>> {
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen, EnableBracketedPaste)?;
    Terminal::new(CrosstermBackend::new(io::stdout()))
}

fn leave_tui(terminal: &mut Terminal<Backend>) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableBracketedPaste,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    #[test]
    fn split_id_round_trips() {
        let k = key(&Host::Remote("me@server".into()), "api");
        assert_eq!(
            split_id(&k),
            Some((Host::Remote("me@server".into()), "api".to_string()))
        );
        let k = key(&Host::Local, "blog");
        assert_eq!(split_id(&k), Some((Host::Local, "blog".to_string())));
    }

    /// The risky external integration: a vt100 screen renders into a ratatui
    /// buffer via tui-term's PseudoTerminal.
    #[test]
    fn pseudo_terminal_renders_into_buffer() {
        let mut parser = vt100::Parser::new(3, 10, 0);
        parser.process(b"hi there");
        let mut terminal = Terminal::new(TestBackend::new(10, 3)).unwrap();
        terminal
            .draw(|f| f.render_widget(PseudoTerminal::new(parser.screen()), f.area()))
            .unwrap();
        let buf = terminal.backend().buffer();
        let row0: String = (0..8).map(|x| buf[(x, 0)].symbol()).collect();
        assert_eq!(row0, "hi there");
    }
}
