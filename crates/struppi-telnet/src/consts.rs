//! Telnet protocol constants (RFC 854 and the MUD-specific extensions).

/// Telnet commands, the byte following `IAC`.
pub mod cmd {
    /// End of record, used as a prompt marker when the EOR option is on.
    pub const EOR: u8 = 239;
    /// End of subnegotiation.
    pub const SE: u8 = 240;
    pub const NOP: u8 = 241;
    /// Go ahead, used as a prompt marker.
    pub const GA: u8 = 249;
    /// Start of subnegotiation.
    pub const SB: u8 = 250;
    pub const WILL: u8 = 251;
    pub const WONT: u8 = 252;
    pub const DO: u8 = 253;
    pub const DONT: u8 = 254;
    /// Interpret as command.
    pub const IAC: u8 = 255;
}

/// Telnet options.
pub mod opt {
    pub const ECHO: u8 = 1;
    /// Suppress go ahead.
    pub const SGA: u8 = 3;
    /// Terminal type, also used for MTTS.
    pub const TTYPE: u8 = 24;
    /// End of record.
    pub const EOR: u8 = 25;
    /// Negotiate about window size.
    pub const NAWS: u8 = 31;
    pub const LINEMODE: u8 = 34;
    pub const CHARSET: u8 = 42;
    pub const MSDP: u8 = 69;
    pub const MSSP: u8 = 70;
    pub const MCCP2: u8 = 86;
    pub const MXP: u8 = 91;
    pub const GMCP: u8 = 201;
}

/// TTYPE subnegotiation: the client states its terminal type.
pub const TTYPE_IS: u8 = 0;
/// TTYPE subnegotiation: the server asks for the terminal type.
pub const TTYPE_SEND: u8 = 1;
