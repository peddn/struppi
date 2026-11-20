//! Application state and the screen layout.

use ratatui::Frame;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style as TuiStyle};
use struppi_core::{Connection, History, NetEvent, Session};
use tokio::sync::mpsc;
use unicode_width::UnicodeWidthStr;

use crate::input::Editor;
use crate::view;

/// Rows used below the output area: status bar and input line.
const CHROME_ROWS: u16 = 2;

/// Below this output height the live tail is not shown while scrolled back.
const MIN_SPLIT_HEIGHT: u16 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnState {
    Connecting,
    Connected,
    Disconnected,
}

pub struct App {
    session: Session,
    connection: Option<Connection>,
    net_tx: mpsc::Sender<NetEvent>,
    host: String,
    port: u16,
    state: ConnState,
    editor: Editor,
    history: History,
    /// When scrolled back: the history view shows the lines before this
    /// scrollback sequence number. `None` follows live output.
    view_end: Option<u64>,
    quit_armed: bool,
    should_quit: bool,
    needs_clear: bool,
    /// Columns and rows of the output area.
    output_size: (u16, u16),
}

impl App {
    pub fn new(host: String, port: u16, net_tx: mpsc::Sender<NetEvent>) -> Self {
        Self {
            session: Session::new(),
            connection: None,
            net_tx,
            host,
            port,
            state: ConnState::Disconnected,
            editor: Editor::default(),
            history: History::default(),
            view_end: None,
            quit_armed: false,
            should_quit: false,
            needs_clear: false,
            output_size: (80, 22),
        }
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    /// Whether the terminal should be cleared before the next draw. Resets
    /// the request.
    pub fn take_clear(&mut self) -> bool {
        std::mem::take(&mut self.needs_clear)
    }

    /// Starts connecting. Must be called from within a tokio runtime.
    pub fn connect(&mut self) {
        self.session
            .notice(&format!("-- Connecting to {}:{} ...", self.host, self.port));
        self.connection = Some(Connection::open(
            self.host.clone(),
            self.port,
            self.net_tx.clone(),
        ));
        self.state = ConnState::Connecting;
    }

    /// Records the terminal size and reports the output area to the server.
    pub fn set_size(&mut self, columns: u16, rows: u16) {
        self.output_size = (columns, rows.saturating_sub(CHROME_ROWS));
        self.session.resize(self.output_size.0, self.output_size.1);
        self.flush();
    }

    /// Hands pending output to the connection.
    fn flush(&mut self) {
        let bytes = self.session.take_output();
        if let Some(connection) = &self.connection {
            connection.send(bytes);
        }
    }

    pub fn on_net(&mut self, event: NetEvent) {
        match event {
            NetEvent::Connected => {
                self.state = ConnState::Connected;
                self.session
                    .notice(&format!("-- Connected to {}:{}", self.host, self.port));
                // Negotiation replies may already be queued, e.g. a resize.
                self.flush();
            }
            NetEvent::Data(bytes) => {
                self.session.receive(&bytes);
                self.flush();
            }
            NetEvent::Closed(reason) => {
                let message = match (self.state, reason) {
                    (ConnState::Connected, None) => "-- Connection closed by the server".to_owned(),
                    (ConnState::Connected, Some(reason)) => format!("-- Connection lost: {reason}"),
                    (_, Some(reason)) => format!("-- Could not connect: {reason}"),
                    (_, None) => "-- Could not connect".to_owned(),
                };
                self.connection = None;
                self.state = ConnState::Disconnected;
                self.session.disconnected();
                self.session.take_output();
                self.session.notice(&message);
                self.session
                    .notice("-- Press Enter on an empty line to reconnect, Ctrl-D to quit");
            }
        }
    }

    pub fn on_terminal(&mut self, event: Event) {
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => self.on_key(key),
            Event::Paste(text) => self.on_paste(&text),
            Event::Resize(columns, rows) => self.set_size(columns, rows),
            _ => {}
        }
    }

    fn on_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);

        let was_armed = std::mem::take(&mut self.quit_armed);

        match key.code {
            KeyCode::Char('c') if ctrl => {
                if !self.editor.is_empty() {
                    self.editor.clear();
                    self.history.reset();
                } else if was_armed {
                    self.should_quit = true;
                } else {
                    self.quit_armed = true;
                }
            }
            KeyCode::Char('d') if ctrl => {
                if self.editor.is_empty() {
                    self.should_quit = true;
                } else {
                    self.editor.delete();
                }
            }
            KeyCode::Char('l') if ctrl => self.needs_clear = true,
            KeyCode::Char('a') if ctrl => self.editor.home(),
            KeyCode::Char('e') if ctrl => self.editor.end(),
            KeyCode::Char('u') if ctrl => self.editor.kill_to_start(),
            KeyCode::Char('k') if ctrl => self.editor.kill_to_end(),
            KeyCode::Char('w') if ctrl => self.editor.delete_word_back(),
            KeyCode::Char('p') if ctrl => self.history_older(),
            KeyCode::Char('n') if ctrl => self.history_newer(),
            KeyCode::Char('b') if alt => self.editor.word_left(),
            KeyCode::Char('f') if alt => self.editor.word_right(),
            KeyCode::Backspace if alt || ctrl => self.editor.delete_word_back(),
            KeyCode::Char(c) if !ctrl && !alt => self.editor.insert(c),
            KeyCode::Enter => self.submit(),
            KeyCode::Backspace => self.editor.backspace(),
            KeyCode::Delete => self.editor.delete(),
            KeyCode::Left if ctrl || alt => self.editor.word_left(),
            KeyCode::Right if ctrl || alt => self.editor.word_right(),
            KeyCode::Left => self.editor.left(),
            KeyCode::Right => self.editor.right(),
            KeyCode::Home => self.editor.home(),
            KeyCode::End => self.editor.end(),
            KeyCode::Up => self.history_older(),
            KeyCode::Down => self.history_newer(),
            KeyCode::PageUp => self.scroll_back(self.page()),
            KeyCode::PageDown => self.scroll_forward(self.page()),
            KeyCode::Esc => self.view_end = None,
            _ => {}
        }
    }

    fn on_paste(&mut self, text: &str) {
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        let mut pieces = normalized.split('\n').peekable();
        while let Some(piece) = pieces.next() {
            self.editor.insert_str(piece);
            // Every complete pasted line is sent; the rest stays editable.
            if pieces.peek().is_some() {
                self.submit();
            }
        }
    }

    fn submit(&mut self) {
        self.view_end = None;
        match self.state {
            ConnState::Connected => {
                let text = self.editor.take();
                if self.session.password_mode() {
                    self.history.reset();
                } else {
                    self.history.push(&text);
                }
                self.session.send_line(&text);
                self.flush();
            }
            ConnState::Disconnected if self.editor.is_empty() => self.connect(),
            // Keep what was typed so it is not lost.
            ConnState::Disconnected | ConnState::Connecting => {
                self.session.notice("-- Not connected")
            }
        }
    }

    fn history_older(&mut self) {
        if self.session.password_mode() {
            return;
        }
        if let Some(text) = self.history.older(self.editor.text()).map(str::to_owned) {
            self.editor.set(&text);
        }
    }

    fn history_newer(&mut self) {
        if let Some(text) = self.history.newer().map(str::to_owned) {
            self.editor.set(&text);
        }
    }

    /// Lines to move per PageUp/PageDown.
    fn page(&self) -> u64 {
        u64::from(self.output_size.1 / 2).max(1)
    }

    /// The effective end of the history view, or `None` when following
    /// live output. Accounts for lines evicted since the view was set.
    fn history_end(&self) -> Option<u64> {
        let scrollback = self.session.scrollback();
        let end = self.view_end?;
        (end < scrollback.end_seq()).then(|| end.max(scrollback.first_seq() + 1))
    }

    fn scroll_back(&mut self, lines: u64) {
        let scrollback = self.session.scrollback();
        if scrollback.is_empty() {
            return;
        }
        let current = self.history_end().unwrap_or(scrollback.end_seq());
        let target = current
            .saturating_sub(lines)
            .max(scrollback.first_seq() + 1);
        self.view_end = (target < scrollback.end_seq()).then_some(target);
    }

    fn scroll_forward(&mut self, lines: u64) {
        if let Some(current) = self.history_end() {
            let target = current.saturating_add(lines);
            self.view_end = (target < self.session.scrollback().end_seq()).then_some(target);
        } else {
            self.view_end = None;
        }
    }

    pub fn draw(&self, frame: &mut Frame) {
        let [output, status, input] = Layout::vertical([
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        self.draw_output(frame, output);
        self.draw_status(frame, status);
        self.draw_input(frame, input);
    }

    fn draw_output(&self, frame: &mut Frame, area: Rect) {
        let scrollback = self.session.scrollback();
        let width = usize::from(area.width);
        let buffer = frame.buffer_mut();

        let Some(history_end) = self.history_end() else {
            let rows = view::rows_before(
                scrollback,
                scrollback.end_seq(),
                self.session.partial(),
                width,
                usize::from(area.height),
            );
            view::render_rows(&rows, area, buffer);
            return;
        };

        // Scrolled back: history on top, a separator, and the live tail
        // below so that new output is not missed.
        let live_height = if area.height >= MIN_SPLIT_HEIGHT {
            (area.height / 4).max(3)
        } else {
            0
        };
        let [history, separator, live] = Layout::vertical([
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(live_height),
        ])
        .areas(area);

        let rows = view::rows_before(
            scrollback,
            history_end,
            None,
            width,
            usize::from(history.height),
        );
        view::render_rows(&rows, history, buffer);

        let below = scrollback.end_seq() - history_end;
        let label = format!(" {below} more lines below | PgDn: forward | Esc: back to live ");
        let rule = format!("──{label}{}", "─".repeat(width));
        buffer.set_stringn(
            separator.x,
            separator.y,
            rule,
            width,
            TuiStyle::default().add_modifier(Modifier::DIM),
        );

        let rows = view::rows_before(
            scrollback,
            scrollback.end_seq(),
            self.session.partial(),
            width,
            usize::from(live.height),
        );
        view::render_rows(&rows, live, buffer);
    }

    fn draw_status(&self, frame: &mut Frame, area: Rect) {
        let state = match self.state {
            ConnState::Connecting => "connecting",
            ConnState::Connected => "connected",
            ConnState::Disconnected => "disconnected",
        };
        let left = format!(" struppi | {}:{} | {state}", self.host, self.port);
        let right = if self.quit_armed {
            "Press Ctrl-C again to quit "
        } else if self.session.password_mode() {
            "input hidden "
        } else if self.history_end().is_some() {
            "scrollback "
        } else {
            "PgUp/PgDn: scroll | Ctrl-D: quit "
        };

        // The right side carries the state that matters right now, so it
        // wins when the bar is too narrow for both.
        let width = usize::from(area.width);
        let style = TuiStyle::default().add_modifier(Modifier::REVERSED);
        let buffer = frame.buffer_mut();
        buffer.set_stringn(area.x, area.y, " ".repeat(width), width, style);

        let right_width = right.width().min(width);
        buffer.set_stringn(area.x, area.y, left, width - right_width, style);
        let offset = u16::try_from(width - right_width).unwrap_or(0);
        buffer.set_stringn(area.x + offset, area.y, right, right_width, style);
    }

    fn draw_input(&self, frame: &mut Frame, area: Rect) {
        if area.height == 0 {
            return;
        }
        let width = usize::from(area.width);
        let (text, cursor) = self.editor.visible(width, self.session.password_mode());
        frame
            .buffer_mut()
            .set_stringn(area.x, area.y, text, width, TuiStyle::default());
        let column = u16::try_from(cursor).unwrap_or(0);
        frame.set_cursor_position((area.x + column, area.y));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn app() -> App {
        let (tx, _rx) = mpsc::channel(1);
        App::new("mud.example".to_owned(), 4000, tx)
    }

    /// An app that behaves as if connected, without a real connection.
    fn connected() -> App {
        let mut app = app();
        app.state = ConnState::Connected;
        app
    }

    fn screen(app: &App, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn ctrl(c: char) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            app.on_terminal(key(KeyCode::Char(c)));
        }
    }

    fn scrollback_texts(app: &App) -> Vec<String> {
        let sb = app.session.scrollback();
        (sb.first_seq()..sb.end_seq())
            .filter_map(|seq| sb.get(seq))
            .map(|line| line.text.clone())
            .collect()
    }

    #[test]
    fn output_is_bottom_aligned_above_status_and_input() {
        let mut app = connected();
        app.set_size(80, 6);
        app.on_net(NetEvent::Data(b"Welcome\r\nA dark room.\r\n> ".to_vec()));
        type_text(&mut app, "look");

        let screen = screen(&app, 80, 6);
        assert_eq!(screen[0], "");
        assert_eq!(screen[1], "Welcome");
        assert_eq!(screen[2], "A dark room.");
        assert_eq!(screen[3], ">");
        assert!(screen[4].starts_with(" struppi | mud.example:4000 | connected"));
        assert!(screen[4].ends_with("Ctrl-D: quit"));
        assert_eq!(screen[5], "look");
    }

    #[test]
    fn enter_sends_echoes_and_records_history() {
        let mut app = connected();
        app.on_net(NetEvent::Data(b"> ".to_vec()));
        type_text(&mut app, "look");
        app.on_terminal(key(KeyCode::Enter));

        assert_eq!(scrollback_texts(&app), ["> look"]);
        assert!(app.editor.is_empty());

        app.on_terminal(key(KeyCode::Up));
        assert_eq!(app.editor.text(), "look");
        app.on_terminal(key(KeyCode::Down));
        assert_eq!(app.editor.text(), "");
    }

    #[test]
    fn typing_while_disconnected_keeps_the_text() {
        let mut app = app();
        type_text(&mut app, "look");
        app.on_terminal(key(KeyCode::Enter));
        assert_eq!(app.editor.text(), "look");
        assert_eq!(scrollback_texts(&app), ["-- Not connected"]);
    }

    #[test]
    fn password_mode_masks_input_and_skips_history() {
        let mut app = connected();
        app.set_size(40, 6);
        app.on_net(NetEvent::Data(vec![b'P', b'w', b':', b' ', 255, 251, 1]));
        type_text(&mut app, "geheim");
        let shown = screen(&app, 40, 6);
        assert_eq!(shown[5], "******");
        assert!(shown[4].ends_with("input hidden"));

        app.on_terminal(key(KeyCode::Enter));
        assert_eq!(scrollback_texts(&app), ["Pw: "]);
        app.on_net(NetEvent::Data(vec![255, 252, 1]));
        app.on_terminal(key(KeyCode::Up));
        assert_eq!(app.editor.text(), "");
    }

    #[test]
    fn scrolling_back_splits_the_view_and_stays_put() {
        let mut app = connected();
        app.set_size(30, 14); // 12 output rows
        let lines: String = (1..=40).map(|i| format!("line {i}\r\n")).collect();
        app.on_net(NetEvent::Data(lines.into_bytes()));

        app.on_terminal(key(KeyCode::PageUp)); // 6 lines back
        let shown = screen(&app, 30, 14);
        // 12 rows: 8 history, 1 separator, 3 live.
        assert_eq!(shown[7], "line 34");
        assert!(
            shown[8].starts_with("── 6 more lines below"),
            "{:?}",
            shown[8]
        );
        assert_eq!(shown[11], "line 40");
        assert!(shown[12].ends_with("scrollback"));

        // New output moves the live tail but not the history view.
        app.on_net(NetEvent::Data(b"line 41\r\n".to_vec()));
        let shown = screen(&app, 30, 14);
        assert_eq!(shown[7], "line 34");
        assert!(shown[8].starts_with("── 7 more lines below"));
        assert_eq!(shown[11], "line 41");

        app.on_terminal(key(KeyCode::PageDown));
        assert!(app.history_end().is_some());
        app.on_terminal(key(KeyCode::PageDown));
        assert!(app.history_end().is_none());
        assert_eq!(screen(&app, 30, 14)[11], "line 41");
    }

    #[test]
    fn scrolling_stops_at_the_oldest_line_and_esc_returns() {
        let mut app = connected();
        app.set_size(30, 14);
        app.on_net(NetEvent::Data(b"a\r\nb\r\nc\r\n".to_vec()));
        for _ in 0..5 {
            app.on_terminal(key(KeyCode::PageUp));
        }
        assert_eq!(app.history_end(), Some(1));
        assert_eq!(screen(&app, 30, 14)[7], "a");
        app.on_terminal(key(KeyCode::Esc));
        assert_eq!(app.history_end(), None);
    }

    #[test]
    fn scrolling_an_empty_buffer_does_nothing() {
        let mut app = connected();
        app.on_terminal(key(KeyCode::PageUp));
        assert_eq!(app.history_end(), None);
    }

    #[test]
    fn ctrl_c_clears_then_needs_confirmation() {
        let mut app = connected();
        type_text(&mut app, "oops");
        app.on_terminal(ctrl('c'));
        assert!(app.editor.is_empty() && !app.should_quit());

        app.on_terminal(ctrl('c'));
        assert!(!app.should_quit());
        type_text(&mut app, "x");
        app.on_terminal(key(KeyCode::Backspace));
        app.on_terminal(ctrl('c'));
        assert!(!app.should_quit(), "another key disarms quitting");
        app.on_terminal(ctrl('c'));
        assert!(app.should_quit());
    }

    #[test]
    fn ctrl_d_quits_only_on_an_empty_line() {
        let mut app = connected();
        type_text(&mut app, "ab");
        app.on_terminal(key(KeyCode::Home));
        app.on_terminal(ctrl('d'));
        assert_eq!(app.editor.text(), "b");
        assert!(!app.should_quit());
        app.on_terminal(key(KeyCode::Delete));
        app.on_terminal(ctrl('d'));
        assert!(app.should_quit());
    }

    #[test]
    fn pasted_lines_are_sent_and_the_rest_stays() {
        let mut app = connected();
        app.on_terminal(Event::Paste("north\r\neast\nloo".to_owned()));
        assert_eq!(scrollback_texts(&app), ["north", "east"]);
        assert_eq!(app.editor.text(), "loo");
    }

    #[test]
    fn close_events_explain_what_happened() {
        let mut app = connected();
        app.on_net(NetEvent::Data(b"half".to_vec()));
        app.on_net(NetEvent::Closed(None));
        let texts = scrollback_texts(&app);
        assert_eq!(texts[0], "half");
        assert_eq!(texts[1], "-- Connection closed by the server");

        let mut app = app_connecting();
        app.on_net(NetEvent::Closed(Some("refused".to_owned())));
        assert_eq!(scrollback_texts(&app)[0], "-- Could not connect: refused");
    }

    fn app_connecting() -> App {
        let mut app = app();
        app.state = ConnState::Connecting;
        app
    }

    #[test]
    fn tiny_terminals_do_not_panic() {
        let mut app = connected();
        app.on_net(NetEvent::Data(b"some text\r\nmore\r\n".to_vec()));
        app.on_terminal(key(KeyCode::PageUp));
        for (width, height) in [(1, 1), (2, 2), (5, 3), (80, 0), (0, 10), (3, 9)] {
            app.set_size(width, height);
            if width > 0 && height > 0 {
                screen(&app, width, height);
            }
        }
    }
}
