//! Turning scrollback lines into rows on the screen.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color as TuiColor, Modifier, Style as TuiStyle};
use struppi_core::{Color, Line, Scrollback, Style};
use unicode_width::UnicodeWidthChar;

/// One screen row: runs of text with their styles.
pub type Row = Vec<(String, Style)>;

/// Breaks a line into rows of at most `width` columns.
///
/// Every line yields at least one row, so blank lines stay visible.
pub fn wrap(line: &Line, width: usize) -> Vec<Row> {
    let width = width.max(1);
    let mut rows: Vec<Row> = vec![Vec::new()];
    let mut column = 0;

    for span in &line.spans {
        let mut run = String::new();
        for c in span.text.chars() {
            let w = c.width().unwrap_or(0);
            if column + w > width && column > 0 {
                if !run.is_empty() {
                    push_run(&mut rows, std::mem::take(&mut run), span.style);
                }
                rows.push(Vec::new());
                column = 0;
            }
            run.push(c);
            column += w;
        }
        if !run.is_empty() {
            push_run(&mut rows, run, span.style);
        }
    }
    rows
}

fn push_run(rows: &mut [Row], text: String, style: Style) {
    if let Some(row) = rows.last_mut() {
        row.push((text, style));
    }
}

/// Collects the last `height` rows of the lines before `end` (a scrollback
/// sequence number), followed by `tail` if given.
pub fn rows_before(
    scrollback: &Scrollback,
    end: u64,
    tail: Option<&Line>,
    width: usize,
    height: usize,
) -> Vec<Row> {
    let mut reversed: Vec<Row> = Vec::with_capacity(height);
    if let Some(line) = tail {
        reversed.extend(wrap(line, width).into_iter().rev());
    }
    let mut seq = end.min(scrollback.end_seq());
    while reversed.len() < height && seq > scrollback.first_seq() {
        seq -= 1;
        if let Some(line) = scrollback.get(seq) {
            reversed.extend(wrap(line, width).into_iter().rev());
        }
    }
    reversed.truncate(height);
    reversed.reverse();
    reversed
}

/// Draws rows bottom-aligned into `area`.
pub fn render_rows(rows: &[Row], area: Rect, buffer: &mut Buffer) {
    let height = usize::from(area.height);
    let rows = &rows[rows.len().saturating_sub(height)..];
    let offset = height - rows.len();
    for (index, row) in rows.iter().enumerate() {
        let Ok(dy) = u16::try_from(offset + index) else {
            break;
        };
        let y = area.y + dy;
        let mut x = area.x;
        for (text, style) in row {
            let remaining = usize::from(area.right().saturating_sub(x));
            if remaining == 0 {
                break;
            }
            (x, _) = buffer.set_stringn(x, y, text, remaining, convert_style(*style));
        }
    }
}

fn convert_color(color: Color) -> TuiColor {
    match color {
        Color::Default => TuiColor::Reset,
        Color::Indexed(index) => TuiColor::Indexed(index),
        Color::Rgb(r, g, b) => TuiColor::Rgb(r, g, b),
    }
}

pub fn convert_style(style: Style) -> TuiStyle {
    let mut modifier = Modifier::empty();
    modifier.set(Modifier::BOLD, style.bold);
    modifier.set(Modifier::DIM, style.dim);
    modifier.set(Modifier::ITALIC, style.italic);
    modifier.set(Modifier::UNDERLINED, style.underline);
    modifier.set(Modifier::SLOW_BLINK, style.blink);
    modifier.set(Modifier::REVERSED, style.reverse);
    modifier.set(Modifier::CROSSED_OUT, style.strike);
    TuiStyle::default()
        .fg(convert_color(style.fg))
        .bg(convert_color(style.bg))
        .add_modifier(modifier)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(text: &str) -> Line {
        Line::local(text, Style::default())
    }

    fn row_text(row: &Row) -> String {
        row.iter().map(|(text, _)| text.as_str()).collect()
    }

    fn texts(rows: &[Row]) -> Vec<String> {
        rows.iter().map(row_text).collect()
    }

    #[test]
    fn short_and_empty_lines_are_one_row() {
        assert_eq!(texts(&wrap(&plain("hello"), 10)), ["hello"]);
        assert_eq!(texts(&wrap(&plain(""), 10)), [""]);
        assert_eq!(texts(&wrap(&plain("12345"), 5)), ["12345"]);
    }

    #[test]
    fn long_lines_wrap_at_the_width() {
        assert_eq!(
            texts(&wrap(&plain("abcdefghij"), 4)),
            ["abcd", "efgh", "ij"]
        );
    }

    #[test]
    fn wide_characters_do_not_straddle_the_edge() {
        assert_eq!(texts(&wrap(&plain("a日本語"), 4)), ["a日", "本語"]);
        // Wider than the area: still makes progress, one per row.
        assert_eq!(texts(&wrap(&plain("日本"), 1)), ["日", "本"]);
    }

    #[test]
    fn styles_survive_wrapping() {
        let mut style = Style::default();
        let line = Line::parse("ab\x1b[31mcdef", &mut style);
        let rows = wrap(&line, 3);
        assert_eq!(texts(&rows), ["abc", "def"]);
        assert_eq!(rows[0].len(), 2);
        assert_eq!(rows[0][1].1.fg, Color::Indexed(1));
        assert_eq!(rows[1][0].1.fg, Color::Indexed(1));
    }

    #[test]
    fn zero_width_does_not_loop_forever() {
        assert_eq!(wrap(&plain("abc"), 0).len(), 3);
    }

    fn scrollback(lines: &[&str]) -> Scrollback {
        let mut scrollback = Scrollback::default();
        for line in lines {
            scrollback.push(plain(line));
        }
        scrollback
    }

    #[test]
    fn rows_before_takes_the_newest_rows() {
        let sb = scrollback(&["one", "two", "three-long", "four"]);
        assert_eq!(
            texts(&rows_before(&sb, sb.end_seq(), None, 5, 3)),
            ["three", "-long", "four"]
        );
        assert_eq!(texts(&rows_before(&sb, 2, None, 5, 3)), ["one", "two"]);
        assert_eq!(
            texts(&rows_before(&sb, 0, None, 5, 3)),
            Vec::<String>::new()
        );
        assert_eq!(texts(&rows_before(&sb, 99, None, 5, 1)), ["four"]);
    }

    #[test]
    fn rows_before_appends_the_tail() {
        let sb = scrollback(&["one", "two"]);
        let tail = plain("> ");
        assert_eq!(
            texts(&rows_before(&sb, sb.end_seq(), Some(&tail), 5, 2)),
            ["two", "> "]
        );
    }

    #[test]
    fn rows_render_bottom_aligned_and_clipped() {
        let area = Rect::new(0, 0, 4, 3);
        let mut buffer = Buffer::empty(area);
        let rows: Vec<Row> = vec![vec![("abcdef".to_owned(), Style::default())]];
        render_rows(&rows, area, &mut buffer);
        let line =
            |y: u16| -> String { (0..4).map(|x| buffer[(x, y)].symbol().to_owned()).collect() };
        assert_eq!(line(0), "    ");
        assert_eq!(line(2), "abcd");
    }
}
