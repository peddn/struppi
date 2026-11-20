# struppi

A terminal MUD client written in Rust, inspired by [TinTin++](https://github.com/scandum/tintin).
The name is the German name of Tintin's dog.

## Goals and non-goals

- Bring TinTin++'s core ideas (sessions, aliases, actions, gags, highlights,
  substitutions, tickers, delays, classes, events, split screen, mapper) to a
  modern TUI.
- **No script compatibility with TinTin++.** Scripting is Lua; TinTin++ scripts
  will not run and do not need to.
- Not planned: `#chat` (peer-to-peer chat), `#port`, `#daemon`, `#advertise`.

## Design decisions

These were agreed with the maintainer. Do not change them without asking.

| Topic | Decision |
|---|---|
| Scripting | Lua 5.4 embedded via `mlua` (vendored). Coroutines give scripts a blocking-style `wait()`. Avoid 5.4-only features where cheap, so a later move to Luau stays possible. |
| Input commands | A thin `/command` layer for one-liners (`/alias`, `/action`, `/gag`, `/lua ...`). No control flow, no variables; it calls the same Rust API as Lua does. |
| Patterns | Plain regular expressions with capture groups. The `/command` layer may translate TinTin-style `%1`, `%w`, `%d` shorthands to regex. |
| TUI | `ratatui` on `crossterm`. The whole screen is drawn by struppi; no VT100 scroll regions. |
| Split screen | Named panes replace `#split`/`#prompt`/`#draw`: fixed panes (row-addressable), scrolling panes with their own scrollback, docked top/bottom/left/right. Layout is declared from Lua. Triggers can route lines to panes. |
| Async runtime | `tokio`. The script engine and all session state live on **one** task; network and terminal tasks talk to it over channels. No locks around engine state. |
| Rendering | Coalesced, at most about 60 frames per second. |

### Line pipeline

Each line received from the server goes through these stages, in this order.
Scripts rely on the order; keep it stable.

1. `RECEIVED LINE` event
2. actions (triggers)
3. prompt handlers
4. gags
5. substitutions
6. highlights
7. `PROCESSED LINE` event, then the line is appended to its pane

Triggers match against the text with colour codes stripped. Substitutions and
highlights modify the coloured original. Every stored line keeps the raw
text, the stripped text and the parsed styled spans.

Prompts (lines without a trailing newline) are detected through telnet GA/EOR,
with a timeout as fallback. The timeout fallback is not implemented yet; it
only matters once prompt triggers exist (roadmap step 2). Until a prompt is
confirmed by GA/EOR, an unterminated line simply keeps growing.

Of this pipeline, step 1 of the roadmap implements only line assembly and
appending; the stages in between arrive with step 2.

### Screen and input (as built in step 1)

- Layout: output area, one status row, one input row.
- Scrolling back splits the output: history above, a separator, the live tail
  below. The history view is anchored to a scrollback sequence number, so it
  does not move when new output arrives.
- A command is echoed after the pending prompt on the same line, like a
  terminal would show it. Nothing is echoed or stored in history while the
  server has ECHO on (password entry).
- The size reported through NAWS is the output area, not the whole terminal.
- Terminal input is read on a plain thread and forwarded over a channel;
  everything else runs on a current-thread tokio runtime.

## Workspace layout

| Crate | Path | Responsibility |
|---|---|---|
| `struppi-telnet` | `crates/struppi-telnet` | Sans-I/O telnet state machine: bytes in, events out. Option negotiation, subnegotiation, MCCP. No sockets, no async. |
| `struppi-script` | `crates/struppi-script` | Lua host and the `/command` parser. No network or terminal I/O. |
| `struppi-core` | `crates/struppi-core` | Sessions, trigger lists, timers, events, scrollback, logging. |
| `struppi` | `crates/struppi` | The binary: `ratatui` UI, input handling, wiring. Renders state owned by `struppi-core`. |

Dependencies point one way: `struppi` -> `struppi-core` -> `struppi-telnet`,
`struppi-script`. The lower crates never depend on the UI.

## Commands

```sh
cargo build --workspace
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p struppi
```

All four checks (build, test, fmt, clippy) must pass before opening a pull
request. CI runs the same commands.

## Test environment

- **Unit and CI tests** use an in-process mock telnet server. CI has no Python
  and no Evennia.
- **Local integration testing** uses [Evennia](https://www.evennia.com):

  ```sh
  scripts/evennia.sh setup    # once: venv + game in .evennia/ (git-ignored)
  scripts/evennia.sh start    # telnet on localhost:4000
  scripts/evennia.sh stop
  ```

  Then `cargo run -p struppi -- localhost 4000` and log in with
  `connect struppi struppi-test`. On connect Evennia offers SGA, MCCP2,
  MSSP, MSDP, GMCP and MXP, and requests NAWS, TTYPE and LINEMODE.

## Conventions

- Language: code, comments, commit messages, issues and docs are in English.
- `unsafe` is forbidden workspace-wide (`unsafe_code = "forbid"`).
- Libraries return typed errors (`thiserror`); only the binary uses `anyhow`.
  No `unwrap()`/`expect()` on anything that depends on network or user input.
- Server data is untrusted: never panic on malformed telnet, ANSI or UTF-8.
- Every protocol feature and every pipeline stage gets tests.
- Add a dependency only in the pull request that first uses it.

## Workflow

- `main` always builds. Never commit to `main` directly.
- One feature branch and one pull request per roadmap step or issue. Branch
  names: `feat/...`, `fix/...`, `chore/...`, `docs/...`.
- Claude opens pull requests; the maintainer reviews and merges. Claude never
  merges.
- Roadmap steps are tracked as GitHub issues with milestones; each pull
  request references its issue.
- Keep this file up to date when a design decision changes.

## Licensing

struppi is licensed under GPL-3.0-or-later. TinTin++ is LGPL-2.1-or-later,
which may be relicensed under the GPL, so its source may be consulted and
adapted. When code is adapted from TinTin++, say so in a comment and keep the
attribution.

## Roadmap

1. **Playable client**: connect, telnet basics (NAWS, TTYPE, SGA, EOR/GA),
   ANSI scrollback with scrolling back, input line with history.
2. **Usable prototype**: Lua host with alias, trigger, gag, highlight,
   substitute and timers; `/command` layer; loading a script file; a first
   fixed status pane.
3. **Protocols**: MCCP2, GMCP, MSDP, MSSP, TLS, charset handling.
4. **Sessions and panes**: multiple sessions, Lua-declared pane layout, line
   routing, mouse, copy mode, logging.
5. **Classes and events**: classes, the full event set, tab completion.
6. **Mapper**.

Current state: step 1 is implemented (`struppi <host> <port>`). Next step: 2.
