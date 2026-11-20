//! struppi: a modern terminal MUD client inspired by TinTin++.

mod app;
mod input;
mod view;

use std::io::stdout;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, DisableBracketedPaste, EnableBracketedPaste, Event};
use ratatui::crossterm::execute;
use tokio::sync::mpsc;
use tokio::time::Instant;

use app::App;

/// Minimum time between two frames (about 60 frames per second).
const FRAME: Duration = Duration::from_millis(16);

const USAGE: &str = "\
Usage: struppi <host> <port>
       struppi <host>:<port>

Options:
  -h, --help       Print this help
  -V, --version    Print the version

Keys:
  Enter            Send the input line
  Up / Down        Input history
  PgUp / PgDn      Scroll back and forward
  Esc              Return to live output
  Ctrl-C           Clear the input line; twice on an empty line: quit
  Ctrl-D           Quit (on an empty line)
  Ctrl-L           Redraw the screen";

enum Command {
    Connect { host: String, port: u16 },
    Help,
    Version,
}

fn parse_args(args: &[String]) -> Result<Command> {
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        return Ok(Command::Help);
    }
    if args.iter().any(|arg| arg == "-V" || arg == "--version") {
        return Ok(Command::Version);
    }
    let (host, port) = match args {
        [target] => target
            .rsplit_once(':')
            .context("expected <host> <port> or <host>:<port>")?,
        [host, port] => (host.as_str(), port.as_str()),
        _ => bail!("expected <host> <port> or <host>:<port>"),
    };
    if host.is_empty() {
        bail!("the host must not be empty");
    }
    let port = port
        .parse()
        .with_context(|| format!("invalid port: {port}"))?;
    Ok(Command::Connect {
        host: host.to_owned(),
        port,
    })
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (host, port) = match parse_args(&args) {
        Ok(Command::Connect { host, port }) => (host, port),
        Ok(Command::Help) => {
            println!("{USAGE}");
            return Ok(());
        }
        Ok(Command::Version) => {
            println!("struppi {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Err(error) => {
            eprintln!("struppi: {error:#}\n\n{USAGE}");
            std::process::exit(2);
        }
    };

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    let mut terminal = ratatui::init();
    // Without bracketed paste, pasted newlines would arrive as Enter keys;
    // that still works, so a failure here is not fatal.
    let _ = execute!(stdout(), EnableBracketedPaste);
    let result = runtime.block_on(run(&mut terminal, host, port));
    let _ = execute!(stdout(), DisableBracketedPaste);
    ratatui::restore();

    // The input thread is blocked reading the terminal; do not wait for it.
    runtime.shutdown_background();
    result
}

async fn run(terminal: &mut DefaultTerminal, host: String, port: u16) -> Result<()> {
    let (net_tx, mut net_rx) = mpsc::channel(256);
    let (term_tx, mut term_rx) = mpsc::channel::<Event>(64);

    // Terminal input is read on its own thread because it blocks.
    std::thread::spawn(move || {
        while let Ok(event) = event::read() {
            if term_tx.blocking_send(event).is_err() {
                break;
            }
        }
    });

    let mut app = App::new(host, port, net_tx);
    let size = terminal.size()?;
    app.set_size(size.width, size.height);
    app.connect();

    let mut dirty = true;
    let mut next_frame = Instant::now();

    while !app.should_quit() {
        tokio::select! {
            Some(event) = net_rx.recv() => {
                app.on_net(event);
                dirty = true;
            }
            event = term_rx.recv() => {
                // The input thread ended: the terminal is gone.
                let Some(event) = event else { break };
                app.on_terminal(event);
                dirty = true;
            }
            // Drawing is coalesced: however much arrives, at most one frame
            // is drawn per FRAME.
            () = tokio::time::sleep_until(next_frame), if dirty => {
                if app.take_clear() {
                    terminal.clear()?;
                }
                terminal.draw(|frame| app.draw(frame))?;
                dirty = false;
                next_frame = Instant::now() + FRAME;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn host_and_port_in_both_forms() {
        for list in [&["localhost", "4000"][..], &["localhost:4000"]] {
            match parse_args(&args(list)).unwrap() {
                Command::Connect { host, port } => {
                    assert_eq!((host.as_str(), port), ("localhost", 4000))
                }
                _ => panic!("expected a connect command"),
            }
        }
    }

    #[test]
    fn invalid_arguments_are_rejected() {
        for list in [
            &[][..],
            &["localhost"],
            &["localhost", "x"],
            &["localhost", "70000"],
            &[":4000"],
            &["a", "1", "b"],
        ] {
            assert!(
                parse_args(&args(list)).is_err(),
                "{list:?} should be rejected"
            );
        }
    }

    #[test]
    fn help_and_version_flags() {
        assert!(matches!(
            parse_args(&args(&["--help"])).unwrap(),
            Command::Help
        ));
        assert!(matches!(
            parse_args(&args(&["-V"])).unwrap(),
            Command::Version
        ));
    }
}
