//! Client engine: sessions, trigger lists, timers, events and scrollback.
//!
//! The engine runs on a single task; network and input tasks talk to it
//! through channels.

pub mod history;
pub mod line;
pub mod net;
pub mod scrollback;
pub mod session;

pub use history::History;
pub use line::{Color, Line, Span, Style};
pub use net::{Connection, NetEvent};
pub use scrollback::Scrollback;
pub use session::Session;
