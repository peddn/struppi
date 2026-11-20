//! The state of one connection: telnet, line assembly and scrollback.
//!
//! [`Session`] performs no I/O. The owner feeds it bytes from the socket with
//! [`Session::receive`] and writes whatever [`Session::take_output`] returns.

use struppi_telnet::{Event, Telnet};

use crate::line::{Color, Line, Style};
use crate::scrollback::Scrollback;

/// A line longer than this many bytes is broken up, which bounds memory when
/// a server never sends a newline.
pub const MAX_LINE_BYTES: usize = 16 * 1024;

/// Style of echoed commands.
const ECHO_STYLE: Style = Style {
    fg: Color::Indexed(3),
    bg: Color::Default,
    bold: false,
    dim: false,
    italic: false,
    underline: false,
    blink: false,
    reverse: false,
    strike: false,
};

/// Style of messages from the client itself.
const NOTICE_STYLE: Style = Style {
    fg: Color::Indexed(6),
    bg: Color::Default,
    bold: false,
    dim: false,
    italic: false,
    underline: false,
    blink: false,
    reverse: false,
    strike: false,
};

#[derive(Debug)]
pub struct Session {
    telnet: Telnet,
    scrollback: Scrollback,
    /// Bytes of the current, not yet terminated line.
    pending: Vec<u8>,
    /// Style in effect at the start of `pending`.
    style: Style,
    /// `pending` parsed for display; `None` when `pending` is empty.
    partial: Option<Line>,
    /// The server marked `pending` as a complete prompt.
    prompt: bool,
    remote_echo: bool,
    window: (u16, u16),
}

impl Session {
    pub fn new() -> Self {
        Self {
            telnet: Telnet::default(),
            scrollback: Scrollback::default(),
            pending: Vec::new(),
            style: Style::default(),
            partial: None,
            prompt: false,
            remote_echo: false,
            window: (80, 24),
        }
    }

    pub fn scrollback(&self) -> &Scrollback {
        &self.scrollback
    }

    /// The current unterminated line, typically a prompt.
    pub fn partial(&self) -> Option<&Line> {
        self.partial.as_ref()
    }

    /// The server echoes input itself, which servers use for password
    /// entry. Input should be masked and kept out of history and scrollback.
    pub fn password_mode(&self) -> bool {
        self.remote_echo
    }

    /// Bytes to write to the socket.
    pub fn take_output(&mut self) -> Vec<u8> {
        self.telnet.take_output()
    }

    /// Reports the size of the output area to the server.
    pub fn resize(&mut self, columns: u16, rows: u16) {
        self.window = (columns, rows);
        self.telnet.set_window_size(columns, rows);
    }

    /// Processes bytes received from the server.
    pub fn receive(&mut self, bytes: &[u8]) {
        for event in self.telnet.receive(bytes) {
            match event {
                Event::Data(data) => self.receive_data(&data),
                Event::Prompt => {
                    if !self.pending.is_empty() {
                        self.prompt = true;
                        if let Some(partial) = &mut self.partial {
                            partial.prompt = true;
                        }
                    }
                }
                Event::RemoteEcho(on) => self.remote_echo = on,
                // GMCP, MSDP and friends arrive with roadmap step 3.
                Event::Subnegotiation { .. } => {}
            }
        }
        self.refresh_partial();
    }

    fn receive_data(&mut self, data: &[u8]) {
        // Output after a finished prompt starts on a new line.
        if self.prompt {
            self.commit_pending();
        }
        for &byte in data {
            match byte {
                b'\n' => self.commit_pending(),
                // CR is part of the CR LF line ending; NUL follows a bare CR.
                b'\r' | 0 => {}
                _ => {
                    self.pending.push(byte);
                    if self.pending.len() >= MAX_LINE_BYTES {
                        self.commit_pending();
                    }
                }
            }
        }
    }

    /// Turns `pending` into a scrollback line, even when it is empty.
    fn commit_pending(&mut self) {
        let raw = String::from_utf8_lossy(&self.pending).into_owned();
        let mut line = Line::parse(&raw, &mut self.style);
        line.prompt = self.prompt;
        self.scrollback.push(line);
        self.pending.clear();
        self.partial = None;
        self.prompt = false;
    }

    fn refresh_partial(&mut self) {
        if self.pending.is_empty() {
            self.partial = None;
            return;
        }
        // A multi-byte character may be split across reads; hide its first
        // bytes until the rest arrives.
        let complete = self.pending.len() - incomplete_utf8_tail(&self.pending);
        let raw = String::from_utf8_lossy(&self.pending[..complete]);
        let mut style = self.style;
        let mut line = Line::parse(&raw, &mut style);
        line.prompt = self.prompt;
        self.partial = Some(line);
    }

    /// Sends a line of input to the server and echoes it locally.
    pub fn send_line(&mut self, text: &str) {
        self.telnet.send(text.as_bytes());
        self.telnet.send(b"\r\n");

        let echo = if self.password_mode() { "" } else { text };
        if self.pending.is_empty() {
            if !self.password_mode() {
                self.scrollback.push(Line::local(echo, ECHO_STYLE));
            }
        } else {
            // Like a terminal would show it: the command follows the prompt.
            let raw = String::from_utf8_lossy(&self.pending).into_owned();
            let mut line = Line::parse(&raw, &mut self.style);
            line.prompt = self.prompt;
            line.append(echo, ECHO_STYLE);
            self.scrollback.push(line);
            self.pending.clear();
            self.partial = None;
            self.prompt = false;
        }
    }

    /// Adds a message from the client itself to the scrollback.
    pub fn notice(&mut self, text: &str) {
        if !self.pending.is_empty() {
            self.commit_pending();
        }
        self.scrollback.push(Line::local(text, NOTICE_STYLE));
    }

    /// Resets per-connection state after the connection ended.
    pub fn disconnected(&mut self) {
        if !self.pending.is_empty() {
            self.commit_pending();
        }
        self.telnet = Telnet::default();
        self.telnet.set_window_size(self.window.0, self.window.1);
        self.style = Style::default();
        self.remote_echo = false;
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

/// Length of an incomplete UTF-8 sequence at the end of `bytes`.
fn incomplete_utf8_tail(bytes: &[u8]) -> usize {
    for back in 1..=bytes.len().min(3) {
        let byte = bytes[bytes.len() - back];
        if byte & 0b1100_0000 == 0b1000_0000 {
            continue; // continuation byte, keep looking for the lead byte
        }
        let needed = match byte {
            0xC0..=0xDF => 2,
            0xE0..=0xEF => 3,
            0xF0..=0xF7 => 4,
            _ => return 0,
        };
        return if needed > back { back } else { 0 };
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use struppi_telnet::consts::{cmd::*, opt};

    fn texts(session: &Session) -> Vec<String> {
        let scrollback = session.scrollback();
        (scrollback.first_seq()..scrollback.end_seq())
            .filter_map(|seq| scrollback.get(seq))
            .map(|line| line.text.clone())
            .collect()
    }

    #[test]
    fn lines_are_assembled_across_reads() {
        let mut session = Session::new();
        session.receive(b"Hello\r\nWor");
        assert_eq!(texts(&session), ["Hello"]);
        assert_eq!(session.partial().unwrap().text, "Wor");
        session.receive(b"ld\r\n");
        assert_eq!(texts(&session), ["Hello", "World"]);
        assert!(session.partial().is_none());
    }

    #[test]
    fn bare_lf_and_blank_lines() {
        let mut session = Session::new();
        session.receive(b"a\n\r\n\nb\n");
        assert_eq!(texts(&session), ["a", "", "", "b"]);
    }

    #[test]
    fn colour_carries_over_lines_and_into_the_partial() {
        let mut session = Session::new();
        session.receive(b"\x1b[31mred\r\nstill\r\npart");
        let scrollback = session.scrollback();
        assert_eq!(
            scrollback.get(1).unwrap().spans[0].style.fg,
            Color::Indexed(1)
        );
        assert_eq!(
            session.partial().unwrap().spans[0].style.fg,
            Color::Indexed(1)
        );
    }

    #[test]
    fn split_utf8_is_not_shown_half() {
        let mut session = Session::new();
        let bytes = "Grüße".as_bytes();
        session.receive(&bytes[..3]); // "Gr" + first byte of "ü"
        assert_eq!(session.partial().unwrap().text, "Gr");
        session.receive(&bytes[3..]);
        assert_eq!(session.partial().unwrap().text, "Grüße");
        session.receive(b"\n");
        assert_eq!(texts(&session), ["Grüße"]);
    }

    #[test]
    fn invalid_utf8_is_replaced_not_fatal() {
        let mut session = Session::new();
        session.receive(&[b'a', 0xE4, b'b', 0xC0, 0xF8, b'\n']);
        assert_eq!(texts(&session), ["a\u{FFFD}b\u{FFFD}\u{FFFD}"]);
    }

    #[test]
    fn incomplete_tail_detection() {
        assert_eq!(incomplete_utf8_tail(b"abc"), 0);
        assert_eq!(incomplete_utf8_tail(&[b'a', 0xC3]), 1);
        assert_eq!(incomplete_utf8_tail(&[0xE2, 0x82]), 2);
        assert_eq!(incomplete_utf8_tail(&[0xF0, 0x9F, 0x98]), 3);
        assert_eq!(incomplete_utf8_tail("€".as_bytes()), 0);
        assert_eq!(incomplete_utf8_tail(&[0x80, 0x80, 0x80, 0x80]), 0);
        assert_eq!(incomplete_utf8_tail(&[]), 0);
    }

    #[test]
    fn prompt_marker_finishes_the_line_before_new_output() {
        let mut session = Session::new();
        session.receive(&[b'H', b'P', b'>', b' ', IAC, GA]);
        assert!(session.partial().unwrap().prompt);
        assert!(texts(&session).is_empty());

        session.receive(b"A goblin arrives.\r\n");
        assert_eq!(texts(&session), ["HP> ", "A goblin arrives."]);
        assert!(session.scrollback().get(0).unwrap().prompt);
        assert!(!session.scrollback().get(1).unwrap().prompt);
    }

    #[test]
    fn prompt_marker_without_text_is_ignored() {
        let mut session = Session::new();
        session.receive(&[b'a', b'\n', IAC, GA, b'b', b'\n']);
        assert_eq!(texts(&session), ["a", "b"]);
    }

    #[test]
    fn unmarked_partial_keeps_growing() {
        let mut session = Session::new();
        session.receive(b"Loading.");
        session.receive(b"..");
        session.receive(b" done\r\n");
        assert_eq!(texts(&session), ["Loading... done"]);
    }

    #[test]
    fn sent_lines_are_escaped_terminated_and_echoed() {
        let mut session = Session::new();
        session.send_line("say \u{ff}");
        assert_eq!(session.take_output(), "say \u{ff}\r\n".as_bytes());
        assert_eq!(texts(&session), ["say \u{ff}"]);

        session.send_line("");
        assert_eq!(session.take_output(), b"\r\n");
        assert_eq!(texts(&session).len(), 2);
    }

    #[test]
    fn echo_joins_the_prompt() {
        let mut session = Session::new();
        session.receive(b"> ");
        session.send_line("look");
        assert_eq!(texts(&session), ["> look"]);
        assert!(session.partial().is_none());
        session.receive(b"A room.\r\n");
        assert_eq!(texts(&session), ["> look", "A room."]);
    }

    #[test]
    fn password_mode_hides_input() {
        let mut session = Session::new();
        session.receive(&[b'P', b'a', b's', b's', b':', b' ', IAC, WILL, opt::ECHO]);
        assert!(session.password_mode());
        session.take_output();

        session.send_line("secret");
        assert_eq!(session.take_output(), b"secret\r\n");
        assert_eq!(texts(&session), ["Pass: "]);

        session.receive(&[IAC, WONT, opt::ECHO]);
        assert!(!session.password_mode());
    }

    #[test]
    fn password_mode_without_prompt_echoes_nothing() {
        let mut session = Session::new();
        session.receive(&[IAC, WILL, opt::ECHO]);
        session.send_line("secret");
        assert!(texts(&session).is_empty());
    }

    #[test]
    fn overlong_lines_are_broken_up() {
        let mut session = Session::new();
        session.receive(&vec![b'x'; MAX_LINE_BYTES * 2 + 10]);
        assert_eq!(session.scrollback().len(), 2);
        assert_eq!(session.partial().unwrap().text.len(), 10);
    }

    #[test]
    fn notices_and_disconnect_flush_the_partial() {
        let mut session = Session::new();
        session.receive(b"half");
        session.notice("-- note");
        assert_eq!(texts(&session), ["half", "-- note"]);

        session.receive(&[b'x', IAC, WILL, opt::ECHO]);
        session.disconnected();
        assert_eq!(texts(&session), ["half", "-- note", "x"]);
        assert!(!session.password_mode());
    }

    #[test]
    fn window_size_is_reported_and_survives_reconnect() {
        let mut session = Session::new();
        session.resize(120, 40);
        session.receive(&[IAC, DO, opt::NAWS]);
        let expected = vec![
            IAC,
            WILL,
            opt::NAWS,
            IAC,
            SB,
            opt::NAWS,
            0,
            120,
            0,
            40,
            IAC,
            SE,
        ];
        assert_eq!(session.take_output(), expected);

        session.disconnected();
        session.receive(&[IAC, DO, opt::NAWS]);
        assert_eq!(session.take_output(), expected);
    }
}
