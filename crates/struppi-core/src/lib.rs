//! Client engine: sessions, trigger lists, timers, events and scrollback.
//!
//! The engine runs on a single task; network and input tasks talk to it
//! through channels.
