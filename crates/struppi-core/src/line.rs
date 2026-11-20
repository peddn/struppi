//! Styled text lines and the ANSI escape sequence parser that produces them.

use unicode_width::UnicodeWidthChar;

/// Width of a tab stop in columns.
const TAB_WIDTH: usize = 8;

/// A terminal colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Color {
    /// The terminal's default foreground or background.
    #[default]
    Default,
    /// One of the 256 indexed colours; 0-7 are the classic ANSI colours and
    /// 8-15 their bright variants.
    Indexed(u8),
    Rgb(u8, u8, u8),
}

/// Text attributes set by SGR escape sequences.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Style {
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub blink: bool,
    pub reverse: bool,
    pub strike: bool,
}

impl Style {
    /// A style with only the foreground colour set.
    pub fn fg(color: Color) -> Self {
        Self {
            fg: color,
            ..Self::default()
        }
    }

    /// Applies the parameters of one SGR sequence (`ESC [ ... m`).
    fn apply_sgr(&mut self, params: &[u16]) {
        if params.is_empty() {
            *self = Self::default();
            return;
        }
        let mut iter = params.iter().copied();
        while let Some(param) = iter.next() {
            match param {
                0 => *self = Self::default(),
                1 => self.bold = true,
                2 => self.dim = true,
                3 => self.italic = true,
                4 => self.underline = true,
                5 | 6 => self.blink = true,
                7 => self.reverse = true,
                9 => self.strike = true,
                21 => self.underline = true,
                22 => {
                    self.bold = false;
                    self.dim = false;
                }
                23 => self.italic = false,
                24 => self.underline = false,
                25 => self.blink = false,
                27 => self.reverse = false,
                29 => self.strike = false,
                30..=37 => self.fg = Color::Indexed(low_byte(param - 30)),
                38 => {
                    if let Some(color) = extended_color(&mut iter) {
                        self.fg = color;
                    }
                }
                39 => self.fg = Color::Default,
                40..=47 => self.bg = Color::Indexed(low_byte(param - 40)),
                48 => {
                    if let Some(color) = extended_color(&mut iter) {
                        self.bg = color;
                    }
                }
                49 => self.bg = Color::Default,
                90..=97 => self.fg = Color::Indexed(low_byte(param - 90 + 8)),
                100..=107 => self.bg = Color::Indexed(low_byte(param - 100 + 8)),
                _ => {}
            }
        }
    }
}

fn low_byte(value: u16) -> u8 {
    u8::try_from(value.min(255)).unwrap_or(u8::MAX)
}

/// Parses the tail of `38;5;n` / `38;2;r;g;b` (and the same for 48).
fn extended_color(iter: &mut impl Iterator<Item = u16>) -> Option<Color> {
    match iter.next()? {
        5 => Some(Color::Indexed(low_byte(iter.next()?))),
        2 => {
            let r = low_byte(iter.next()?);
            let g = low_byte(iter.next()?);
            let b = low_byte(iter.next()?);
            Some(Color::Rgb(r, g, b))
        }
        _ => None,
    }
}

/// A run of text with a single style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub style: Style,
}

/// One line of output.
///
/// Three views of the same line are kept: `raw` is what the server sent
/// (including escape sequences), `text` is the plain text that triggers match
/// against, and `spans` is the styled form used for display.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Line {
    pub raw: String,
    pub text: String,
    pub spans: Vec<Span>,
    /// The server marked this line as a prompt (GA/EOR).
    pub prompt: bool,
}

impl Line {
    /// Parses a raw line. `style` is the style in effect at the start of the
    /// line and is updated to the style in effect at its end, because colours
    /// carry over line breaks.
    pub fn parse(raw: &str, style: &mut Style) -> Self {
        let mut parser = Parser {
            style: *style,
            text: String::new(),
            spans: Vec::new(),
            column: 0,
        };
        parser.run(raw);
        *style = parser.style;
        Self {
            raw: raw.to_owned(),
            text: parser.text,
            spans: parser.spans,
            prompt: false,
        }
    }

    /// A line produced by the client itself, in a single style.
    pub fn local(text: &str, style: Style) -> Self {
        let mut line = Self::default();
        line.append(text, style);
        line
    }

    /// Appends client-generated text, for example the echo of a command.
    pub fn append(&mut self, text: &str, style: Style) {
        if text.is_empty() {
            return;
        }
        self.raw.push_str(text);
        self.text.push_str(text);
        push_span(&mut self.spans, text, style);
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

fn push_span(spans: &mut Vec<Span>, text: &str, style: Style) {
    match spans.last_mut() {
        Some(last) if last.style == style => last.text.push_str(text),
        _ => spans.push(Span {
            text: text.to_owned(),
            style,
        }),
    }
}

struct Parser {
    style: Style,
    text: String,
    spans: Vec<Span>,
    column: usize,
}

impl Parser {
    fn run(&mut self, raw: &str) {
        let mut chars = raw.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\x1b' => self.escape(&mut chars),
                '\t' => {
                    let spaces = TAB_WIDTH - self.column % TAB_WIDTH;
                    for _ in 0..spaces {
                        self.push(' ');
                    }
                }
                // Remaining C0 controls, DEL and C1 controls are not printable.
                c if c.is_control() => {}
                c => self.push(c),
            }
        }
    }

    fn push(&mut self, c: char) {
        self.column += c.width().unwrap_or(0);
        self.text.push(c);
        let mut buffer = [0u8; 4];
        push_span(&mut self.spans, c.encode_utf8(&mut buffer), self.style);
    }

    /// Consumes one escape sequence; the ESC itself is already consumed.
    /// Only SGR has an effect, everything else is dropped.
    fn escape(&mut self, chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
        match chars.next() {
            Some('[') => {
                let mut params: Vec<u16> = Vec::new();
                let mut current: Option<u16> = None;
                let mut private = false;
                for c in chars.by_ref() {
                    match c {
                        '0'..='9' => {
                            let digit = c
                                .to_digit(10)
                                .and_then(|d| u16::try_from(d).ok())
                                .unwrap_or(0);
                            current = Some(
                                current
                                    .unwrap_or(0)
                                    .saturating_mul(10)
                                    .saturating_add(digit),
                            );
                        }
                        ';' | ':' => params.push(current.take().unwrap_or(0)),
                        '<' | '=' | '>' | '?' => private = true,
                        // Intermediate bytes.
                        ' '..='/' => {}
                        // Final byte.
                        '@'..='~' => {
                            if let Some(value) = current.take() {
                                params.push(value);
                            }
                            if c == 'm' && !private {
                                self.style.apply_sgr(&params);
                            }
                            return;
                        }
                        // Malformed sequence: stop interpreting it.
                        _ => return,
                    }
                }
            }
            // OSC, DCS, SOS, PM, APC: skip to BEL or ST (ESC \).
            Some(']' | 'P' | 'X' | '^' | '_') => {
                while let Some(c) = chars.next() {
                    if c == '\x07' {
                        return;
                    }
                    if c == '\x1b' {
                        chars.next_if_eq(&'\\');
                        return;
                    }
                }
            }
            // Character set designation: one more byte follows.
            Some('(' | ')' | '*' | '+' | '#' | '%') => {
                chars.next();
            }
            // Any other two-byte escape, or ESC at the end of the line.
            Some(_) | None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(raw: &str) -> Line {
        Line::parse(raw, &mut Style::default())
    }

    #[test]
    fn plain_text_is_one_span() {
        let line = parse("hello world");
        assert_eq!(line.text, "hello world");
        assert_eq!(
            line.spans,
            vec![Span {
                text: "hello world".into(),
                style: Style::default()
            }]
        );
        assert_eq!(line.raw, "hello world");
    }

    #[test]
    fn sgr_colours_split_spans() {
        let line = parse("a\x1b[1;31mb\x1b[0mc");
        assert_eq!(line.text, "abc");
        assert_eq!(line.spans.len(), 3);
        assert_eq!(
            line.spans[1].style,
            Style {
                fg: Color::Indexed(1),
                bold: true,
                ..Style::default()
            }
        );
        assert_eq!(line.spans[2].style, Style::default());
    }

    #[test]
    fn bright_256_and_truecolor() {
        let line = parse("\x1b[92ma\x1b[38;5;208mb\x1b[48;2;1;2;3mc\x1b[103md");
        let styles: Vec<Style> = line.spans.iter().map(|s| s.style).collect();
        assert_eq!(styles[0].fg, Color::Indexed(10));
        assert_eq!(styles[1].fg, Color::Indexed(208));
        assert_eq!(styles[2].bg, Color::Rgb(1, 2, 3));
        assert_eq!(styles[3].bg, Color::Indexed(11));
    }

    #[test]
    fn attributes_toggle() {
        let mut style = Style::default();
        Line::parse("\x1b[1;2;3;4;5;7;9m", &mut style);
        assert!(style.bold && style.dim && style.italic && style.underline);
        assert!(style.blink && style.reverse && style.strike);
        Line::parse("\x1b[22;23;24;25;27;29m", &mut style);
        assert_eq!(style, Style::default());
    }

    #[test]
    fn empty_sgr_resets() {
        let mut style = Style::default();
        Line::parse("\x1b[31m", &mut style);
        assert_eq!(style.fg, Color::Indexed(1));
        Line::parse("\x1b[m", &mut style);
        assert_eq!(style, Style::default());
    }

    #[test]
    fn style_carries_over_to_the_next_line() {
        let mut style = Style::default();
        Line::parse("\x1b[32mgreen", &mut style);
        let next = Line::parse("still green", &mut style);
        assert_eq!(next.spans[0].style.fg, Color::Indexed(2));
    }

    #[test]
    fn non_sgr_sequences_are_dropped() {
        let line = parse("a\x1b[2Jb\x1b[10;20Hc\x1b[?25ld\x1b]0;title\x07e\x1b(Bf\x1b=g");
        assert_eq!(line.text, "abcdefg");
    }

    #[test]
    fn private_sgr_lookalikes_do_not_change_style() {
        let mut style = Style::default();
        Line::parse("\x1b[>4;1m", &mut style);
        assert_eq!(style, Style::default());
    }

    #[test]
    fn truncated_sequences_do_not_panic() {
        for raw in [
            "\x1b",
            "\x1b[",
            "\x1b[1;",
            "\x1b[38;5",
            "\x1b[38;2;1m",
            "\x1b]0;x",
            "\x1b(",
            "\x1b[\u{1F600}m",
        ] {
            let _ = parse(raw);
        }
    }

    #[test]
    fn huge_parameters_saturate() {
        let line = parse("\x1b[99999999999999999999mx");
        assert_eq!(line.text, "x");
    }

    #[test]
    fn tabs_expand_to_tab_stops() {
        assert_eq!(parse("a\tb").text, "a       b");
        assert_eq!(parse("\tb").text, "        b");
    }

    #[test]
    fn control_characters_are_removed() {
        assert_eq!(parse("a\x07b\x08c\rd\x7fe").text, "abcde");
    }

    #[test]
    fn local_lines_and_append() {
        let style = Style::fg(Color::Indexed(3));
        let mut line = parse("> ");
        line.append("look", style);
        assert_eq!(line.text, "> look");
        assert_eq!(line.spans.len(), 2);
        assert_eq!(Line::local("hi", style).spans[0].style, style);
    }
}
