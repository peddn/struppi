//! Sans-I/O telnet protocol state machine: bytes in, events out.
//!
//! This crate performs no I/O and knows nothing about sessions or the UI.
//! Feed bytes read from the socket into [`Telnet::receive`], handle the
//! returned [`Event`]s, and write whatever [`Telnet::take_output`] returns
//! back to the socket.
//!
//! Server data is untrusted: no input can make the parser panic, and
//! subnegotiation payloads are capped at [`MAX_SUBNEGOTIATION`] bytes.

pub mod consts;

use consts::{cmd, opt};

/// Upper bound for a buffered subnegotiation payload. Longer payloads are
/// truncated, which keeps memory bounded when a server never sends `IAC SE`.
pub const MAX_SUBNEGOTIATION: usize = 64 * 1024;

/// Something the client engine has to react to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Application data with all telnet commands removed.
    Data(Vec<u8>),
    /// `IAC GA` or `IAC EOR`: the server marked the end of a prompt.
    Prompt,
    /// The server turned its own echo on (`true`) or off (`false`).
    ///
    /// While the server echoes, the client should not show what is typed;
    /// servers use this for password entry.
    RemoteEcho(bool),
    /// A subnegotiation for an option this crate does not handle itself.
    Subnegotiation { option: u8, data: Vec<u8> },
}

/// Static client information reported during negotiation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Client name, sent as the first TTYPE answer (MTTS convention).
    pub client_name: String,
    /// Terminal type, sent as the second TTYPE answer.
    pub terminal_type: String,
    /// MTTS capability bitvector, sent as the third TTYPE answer.
    pub mtts: u32,
}

/// MTTS capability bits, see <https://tintin.mudhalla.net/protocols/mtts/>.
pub mod mtts {
    pub const ANSI: u32 = 1;
    pub const UTF8: u32 = 4;
    pub const COLORS_256: u32 = 8;
    pub const TRUECOLOR: u32 = 256;
}

impl Default for Config {
    fn default() -> Self {
        Self {
            client_name: "STRUPPI".to_owned(),
            terminal_type: "XTERM-256COLOR".to_owned(),
            mtts: mtts::ANSI | mtts::UTF8 | mtts::COLORS_256 | mtts::TRUECOLOR,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Data,
    Iac,
    /// Saw `IAC WILL/WONT/DO/DONT`, waiting for the option byte.
    Negotiate(u8),
    /// Saw `IAC SB`, waiting for the option byte.
    SubOption,
    /// Inside a subnegotiation payload.
    Sub,
    /// Saw `IAC` inside a subnegotiation payload.
    SubIac,
}

/// The telnet protocol state machine for one connection.
#[derive(Debug)]
pub struct Telnet {
    config: Config,
    state: State,
    /// Options we have enabled on our side (we sent `WILL`).
    local: [bool; 256],
    /// Options the server has enabled on its side (we sent `DO`).
    remote: [bool; 256],
    sub_option: u8,
    sub_data: Vec<u8>,
    output: Vec<u8>,
    window: (u16, u16),
    ttype_step: usize,
}

impl Telnet {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            state: State::Data,
            local: [false; 256],
            remote: [false; 256],
            sub_option: 0,
            sub_data: Vec::new(),
            output: Vec::new(),
            window: (80, 24),
            ttype_step: 0,
        }
    }

    /// Whether we would enable `option` on our side when the server asks.
    fn supports_local(option: u8) -> bool {
        matches!(option, opt::NAWS | opt::TTYPE)
    }

    /// Whether we let the server enable `option` on its side.
    fn supports_remote(option: u8) -> bool {
        matches!(option, opt::ECHO | opt::SGA | opt::EOR)
    }

    /// Whether we have enabled `option` on our side.
    pub fn local_enabled(&self, option: u8) -> bool {
        self.local[usize::from(option)]
    }

    /// Whether the server has enabled `option` on its side.
    pub fn remote_enabled(&self, option: u8) -> bool {
        self.remote[usize::from(option)]
    }

    /// Bytes that must be written to the socket. Clears the internal buffer.
    pub fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.output)
    }

    /// Queues application data for sending, escaping `IAC` bytes.
    pub fn send(&mut self, data: &[u8]) {
        for &byte in data {
            if byte == cmd::IAC {
                self.output.push(cmd::IAC);
            }
            self.output.push(byte);
        }
    }

    /// Records the window size and reports it if NAWS is active.
    pub fn set_window_size(&mut self, columns: u16, rows: u16) {
        if self.window == (columns, rows) {
            return;
        }
        self.window = (columns, rows);
        if self.local_enabled(opt::NAWS) {
            self.send_naws();
        }
    }

    /// Processes bytes received from the server.
    pub fn receive(&mut self, input: &[u8]) -> Vec<Event> {
        let mut events = Vec::new();
        let mut data = Vec::new();

        for &byte in input {
            match self.state {
                State::Data => {
                    if byte == cmd::IAC {
                        self.state = State::Iac;
                    } else {
                        data.push(byte);
                    }
                }
                State::Iac => match byte {
                    cmd::IAC => {
                        data.push(cmd::IAC);
                        self.state = State::Data;
                    }
                    cmd::WILL | cmd::WONT | cmd::DO | cmd::DONT => {
                        self.state = State::Negotiate(byte);
                    }
                    cmd::SB => self.state = State::SubOption,
                    cmd::GA | cmd::EOR => {
                        flush(&mut data, &mut events);
                        events.push(Event::Prompt);
                        self.state = State::Data;
                    }
                    // NOP, DM, BRK, IP, AO, AYT, EC, EL, a stray SE and
                    // anything unknown: ignore.
                    _ => self.state = State::Data,
                },
                State::Negotiate(command) => {
                    self.state = State::Data;
                    if let Some(event) = self.negotiate(command, byte) {
                        flush(&mut data, &mut events);
                        events.push(event);
                    }
                }
                State::SubOption => {
                    self.sub_option = byte;
                    self.sub_data.clear();
                    self.state = State::Sub;
                }
                State::Sub => {
                    if byte == cmd::IAC {
                        self.state = State::SubIac;
                    } else {
                        self.push_sub(byte);
                    }
                }
                State::SubIac => match byte {
                    cmd::IAC => {
                        self.push_sub(cmd::IAC);
                        self.state = State::Sub;
                    }
                    cmd::SE => {
                        self.state = State::Data;
                        let payload = std::mem::take(&mut self.sub_data);
                        if let Some(event) = self.subnegotiation(self.sub_option, payload) {
                            flush(&mut data, &mut events);
                            events.push(event);
                        }
                    }
                    // Malformed: IAC followed by something else inside a
                    // subnegotiation. Drop the IAC and carry on.
                    _ => self.state = State::Sub,
                },
            }
        }

        flush(&mut data, &mut events);
        events
    }

    fn push_sub(&mut self, byte: u8) {
        if self.sub_data.len() < MAX_SUBNEGOTIATION {
            self.sub_data.push(byte);
        }
    }

    /// Handles `IAC <command> <option>`. Replies only when our state changes,
    /// which prevents negotiation loops.
    fn negotiate(&mut self, command: u8, option: u8) -> Option<Event> {
        let index = usize::from(option);
        match command {
            cmd::WILL => {
                if self.remote[index] {
                    return None;
                }
                if Self::supports_remote(option) {
                    self.remote[index] = true;
                    self.reply(cmd::DO, option);
                    if option == opt::ECHO {
                        return Some(Event::RemoteEcho(true));
                    }
                } else {
                    self.reply(cmd::DONT, option);
                }
                None
            }
            cmd::WONT => {
                if !self.remote[index] {
                    return None;
                }
                self.remote[index] = false;
                self.reply(cmd::DONT, option);
                (option == opt::ECHO).then_some(Event::RemoteEcho(false))
            }
            cmd::DO => {
                if self.local[index] {
                    return None;
                }
                if Self::supports_local(option) {
                    self.local[index] = true;
                    self.reply(cmd::WILL, option);
                    if option == opt::NAWS {
                        self.send_naws();
                    }
                } else {
                    self.reply(cmd::WONT, option);
                }
                None
            }
            cmd::DONT => {
                if self.local[index] {
                    self.local[index] = false;
                    self.reply(cmd::WONT, option);
                }
                None
            }
            _ => None,
        }
    }

    fn subnegotiation(&mut self, option: u8, data: Vec<u8>) -> Option<Event> {
        match option {
            opt::TTYPE => {
                if data.first() == Some(&consts::TTYPE_SEND) && self.local_enabled(opt::TTYPE) {
                    self.send_ttype();
                }
                None
            }
            _ => Some(Event::Subnegotiation { option, data }),
        }
    }

    fn reply(&mut self, command: u8, option: u8) {
        self.output.extend_from_slice(&[cmd::IAC, command, option]);
    }

    fn send_subnegotiation(&mut self, option: u8, payload: &[u8]) {
        self.output.extend_from_slice(&[cmd::IAC, cmd::SB, option]);
        for &byte in payload {
            if byte == cmd::IAC {
                self.output.push(cmd::IAC);
            }
            self.output.push(byte);
        }
        self.output.extend_from_slice(&[cmd::IAC, cmd::SE]);
    }

    fn send_naws(&mut self) {
        let (columns, rows) = self.window;
        let mut payload = Vec::with_capacity(4);
        payload.extend_from_slice(&columns.to_be_bytes());
        payload.extend_from_slice(&rows.to_be_bytes());
        self.send_subnegotiation(opt::NAWS, &payload);
    }

    /// Answers a TTYPE SEND. Follows the MTTS convention: client name, then
    /// terminal type, then `MTTS <bits>`, which is repeated from then on.
    fn send_ttype(&mut self) {
        let answer = match self.ttype_step {
            0 => self.config.client_name.clone(),
            1 => self.config.terminal_type.clone(),
            _ => format!("MTTS {}", self.config.mtts),
        };
        self.ttype_step = self.ttype_step.saturating_add(1);

        let mut payload = vec![consts::TTYPE_IS];
        payload.extend_from_slice(answer.as_bytes());
        self.send_subnegotiation(opt::TTYPE, &payload);
    }
}

impl Default for Telnet {
    fn default() -> Self {
        Self::new(Config::default())
    }
}

fn flush(data: &mut Vec<u8>, events: &mut Vec<Event>) {
    if !data.is_empty() {
        events.push(Event::Data(std::mem::take(data)));
    }
}

#[cfg(test)]
mod tests {
    use super::consts::{cmd::*, opt};
    use super::*;

    fn data(bytes: &[u8]) -> Event {
        Event::Data(bytes.to_vec())
    }

    #[test]
    fn plain_data_passes_through() {
        let mut telnet = Telnet::default();
        assert_eq!(telnet.receive(b"hello\r\n"), vec![data(b"hello\r\n")]);
        assert!(telnet.take_output().is_empty());
    }

    #[test]
    fn escaped_iac_becomes_a_single_byte() {
        let mut telnet = Telnet::default();
        assert_eq!(
            telnet.receive(&[b'a', IAC, IAC, b'b']),
            vec![data(&[b'a', 255, b'b'])]
        );
    }

    #[test]
    fn commands_split_across_reads_are_handled() {
        let mut telnet = Telnet::default();
        assert_eq!(telnet.receive(&[b'a', IAC]), vec![data(b"a")]);
        assert_eq!(telnet.receive(&[WILL]), vec![]);
        assert_eq!(telnet.receive(&[opt::SGA, b'b']), vec![data(b"b")]);
        assert_eq!(telnet.take_output(), vec![IAC, DO, opt::SGA]);
    }

    #[test]
    fn go_ahead_and_end_of_record_mark_prompts() {
        let mut telnet = Telnet::default();
        let events = telnet.receive(&[b'>', b' ', IAC, GA, b'x', IAC, EOR]);
        assert_eq!(
            events,
            vec![data(b"> "), Event::Prompt, data(b"x"), Event::Prompt]
        );
    }

    #[test]
    fn supported_remote_options_are_accepted_once() {
        let mut telnet = Telnet::default();
        telnet.receive(&[IAC, WILL, opt::EOR, IAC, WILL, opt::EOR]);
        assert_eq!(telnet.take_output(), vec![IAC, DO, opt::EOR]);
        assert!(telnet.remote_enabled(opt::EOR));
    }

    #[test]
    fn unsupported_options_are_refused() {
        let mut telnet = Telnet::default();
        telnet.receive(&[IAC, WILL, opt::MCCP2, IAC, DO, opt::LINEMODE]);
        assert_eq!(
            telnet.take_output(),
            vec![IAC, DONT, opt::MCCP2, IAC, WONT, opt::LINEMODE]
        );
        assert!(!telnet.remote_enabled(opt::MCCP2));
        assert!(!telnet.local_enabled(opt::LINEMODE));
    }

    #[test]
    fn wont_and_dont_for_disabled_options_get_no_reply() {
        let mut telnet = Telnet::default();
        telnet.receive(&[IAC, WONT, opt::ECHO, IAC, DONT, opt::NAWS]);
        assert!(telnet.take_output().is_empty());
    }

    #[test]
    fn remote_echo_is_reported() {
        let mut telnet = Telnet::default();
        assert_eq!(
            telnet.receive(&[IAC, WILL, opt::ECHO]),
            vec![Event::RemoteEcho(true)]
        );
        assert_eq!(telnet.take_output(), vec![IAC, DO, opt::ECHO]);
        assert_eq!(
            telnet.receive(&[IAC, WONT, opt::ECHO]),
            vec![Event::RemoteEcho(false)]
        );
        assert_eq!(telnet.take_output(), vec![IAC, DONT, opt::ECHO]);
    }

    #[test]
    fn naws_is_sent_when_enabled_and_on_resize() {
        let mut telnet = Telnet::default();
        telnet.set_window_size(100, 40);
        assert!(telnet.take_output().is_empty(), "NAWS not negotiated yet");

        telnet.receive(&[IAC, DO, opt::NAWS]);
        assert_eq!(
            telnet.take_output(),
            vec![
                IAC,
                WILL,
                opt::NAWS,
                IAC,
                SB,
                opt::NAWS,
                0,
                100,
                0,
                40,
                IAC,
                SE
            ]
        );

        telnet.set_window_size(100, 40);
        assert!(
            telnet.take_output().is_empty(),
            "unchanged size is not resent"
        );

        telnet.set_window_size(120, 50);
        assert_eq!(
            telnet.take_output(),
            vec![IAC, SB, opt::NAWS, 0, 120, 0, 50, IAC, SE]
        );
    }

    #[test]
    fn naws_escapes_255() {
        let mut telnet = Telnet::default();
        telnet.receive(&[IAC, DO, opt::NAWS]);
        telnet.take_output();
        telnet.set_window_size(255, 10);
        assert_eq!(
            telnet.take_output(),
            vec![IAC, SB, opt::NAWS, 0, 255, 255, 0, 10, IAC, SE]
        );
    }

    #[test]
    fn ttype_cycles_through_mtts_answers() {
        let mut telnet = Telnet::default();
        telnet.receive(&[IAC, DO, opt::TTYPE]);
        assert_eq!(telnet.take_output(), vec![IAC, WILL, opt::TTYPE]);

        let send = [IAC, SB, opt::TTYPE, consts::TTYPE_SEND, IAC, SE];
        let mut answers = Vec::new();
        for _ in 0..4 {
            assert!(telnet.receive(&send).is_empty());
            let out = telnet.take_output();
            assert_eq!(&out[..4], &[IAC, SB, opt::TTYPE, consts::TTYPE_IS]);
            assert_eq!(&out[out.len() - 2..], &[IAC, SE]);
            answers.push(String::from_utf8(out[4..out.len() - 2].to_vec()).unwrap());
        }
        assert_eq!(
            answers,
            ["STRUPPI", "XTERM-256COLOR", "MTTS 269", "MTTS 269"]
        );
    }

    #[test]
    fn ttype_send_is_ignored_when_not_negotiated() {
        let mut telnet = Telnet::default();
        telnet.receive(&[IAC, SB, opt::TTYPE, consts::TTYPE_SEND, IAC, SE]);
        assert!(telnet.take_output().is_empty());
    }

    #[test]
    fn unknown_subnegotiations_are_passed_on() {
        let mut telnet = Telnet::default();
        let events = telnet.receive(&[
            b'a',
            IAC,
            SB,
            opt::GMCP,
            b'x',
            IAC,
            IAC,
            b'y',
            IAC,
            SE,
            b'b',
        ]);
        assert_eq!(
            events,
            vec![
                data(b"a"),
                Event::Subnegotiation {
                    option: opt::GMCP,
                    data: vec![b'x', 255, b'y']
                },
                data(b"b"),
            ]
        );
    }

    #[test]
    fn unterminated_subnegotiation_is_bounded() {
        let mut telnet = Telnet::default();
        telnet.receive(&[IAC, SB, opt::GMCP]);
        let chunk = vec![b'x'; 8192];
        for _ in 0..20 {
            assert!(telnet.receive(&chunk).is_empty());
        }
        let events = telnet.receive(&[IAC, SE]);
        match &events[..] {
            [Event::Subnegotiation { data, .. }] => assert_eq!(data.len(), MAX_SUBNEGOTIATION),
            other => panic!("unexpected events: {other:?}"),
        }
    }

    #[test]
    fn outgoing_data_escapes_iac() {
        let mut telnet = Telnet::default();
        telnet.send(&[b'a', 255, b'b']);
        assert_eq!(telnet.take_output(), vec![b'a', 255, 255, b'b']);
    }

    #[test]
    fn arbitrary_bytes_never_panic() {
        // A cheap deterministic fuzz: every byte value in several orders,
        // fed in uneven chunk sizes.
        let mut telnet = Telnet::default();
        let mut seed: u32 = 0x2545_f491;
        let mut bytes = Vec::new();
        for _ in 0..50_000 {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            // Bias towards telnet command bytes to reach every state.
            let byte = if seed % 3 == 0 {
                240 + (seed >> 8) as u8 % 16
            } else {
                (seed >> 8) as u8
            };
            bytes.push(byte);
        }
        for chunk in bytes.chunks(7) {
            telnet.receive(chunk);
            telnet.take_output();
        }
    }
}
