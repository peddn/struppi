# struppi

A modern terminal MUD client written in Rust, inspired by
[TinTin++](https://github.com/scandum/tintin).

> **Status:** early development. You can connect to a MUD and play by hand;
> scripting, triggers and panes are not there yet.

## Usage

```sh
struppi <host> <port>
```

| Key | Action |
|---|---|
| Enter | Send the input line |
| Up / Down | Input history |
| PgUp / PgDn | Scroll back and forward |
| Esc | Return to live output |
| Ctrl-C | Clear the input line; twice on an empty line: quit |
| Ctrl-D | Quit (on an empty line) |
| Ctrl-L | Redraw the screen |

The usual line-editing keys work as well (Ctrl-A, Ctrl-E, Ctrl-U, Ctrl-K,
Ctrl-W, Ctrl-Left, Ctrl-Right).

## What it will be

- Aliases, triggers, gags, highlights, substitutions, tickers and delays, as
  known from TinTin++
- Scripting in Lua, with coroutines for sequential scripts (`wait(2)`)
- A `/command` layer for quick one-liners in the input line
- A TUI with named panes: status lines, side panels and separate scrolling
  buffers for chat or combat logs
- Telnet with MCCP, GMCP, MSDP and TLS
- Multiple sessions and, later, a mapper

struppi is **not** script-compatible with TinTin++.

## Building

```sh
cargo build --release
cargo run --release -p struppi -- <host> <port>
```

Requires a recent stable Rust toolchain.

## Development

See [CLAUDE.md](CLAUDE.md) for the architecture, conventions, workflow and
roadmap. A local [Evennia](https://www.evennia.com) server for testing can be
set up with `scripts/evennia.sh setup`.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
