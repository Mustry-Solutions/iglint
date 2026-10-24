//! Line-oriented human formats: text (with code frames), concise, grouped
//! and pylint.

use std::io::{self, Write};

use iglint_linter::Diagnostic;

use super::{FixState, Report, Style, position, span};

/// Display columns a tab advances to the next multiple of.
const TAB_WIDTH: usize = 4;
/// Source lines longer than this (in display columns) are cropped.
const MAX_LINE_WIDTH: usize = 120;
/// Columns kept before the flagged range when a line is cropped.
const CROP_CONTEXT: usize = 40;
/// Ranges spanning more lines show the first and last lines only.
const MAX_FRAME_LINES: usize = 6;

/// Messages are shown on one line in line-oriented formats.
fn one_line(message: &str) -> String {
    message.lines().map(str::trim).collect::<Vec<_>>().join(" ")
}

/// `path:line:col: CODE [severity] message`
fn header(report: &Report<'_>, diagnostic: &Diagnostic, style: Style) -> String {
    let (start, _) = span(diagnostic);
    let path = report.display_path(diagnostic.file.path());
    format!(
        "{}:{}:{}: {} {} {}",
        style.bold(&path),
        start.line,
        start.column,
        style.bold(diagnostic.rule.code()),
        style.severity(diagnostic.severity, &format!("[{}]", diagnostic.severity)),
        one_line(&diagnostic.message)
    )
}

pub(super) fn concise(report: &Report<'_>, out: &mut dyn Write) -> io::Result<()> {
    let style = Style::new(report.color);
    for diagnostic in report.diagnostics {
        writeln!(out, "{}", header(report, diagnostic, style))?;
    }
    Ok(())
}

pub(super) fn full(report: &Report<'_>, out: &mut dyn Write) -> io::Result<()> {
    let style = Style::new(report.color);
    for diagnostic in report.diagnostics {
        writeln!(out, "{}", header(report, diagnostic, style))?;
        code_frame(report, diagnostic, style, out)?;
        writeln!(out)?;
    }
    Ok(())
}

pub(super) fn grouped(report: &Report<'_>, out: &mut dyn Write) -> io::Result<()> {
    let style = Style::new(report.color);
    let mut current: Option<&std::path::Path> = None;
    for diagnostic in report.diagnostics {
        let path = diagnostic.file.path();
        if current != Some(path) {
            if current.is_some() {
                writeln!(out)?;
            }
            writeln!(out, "{}:", style.bold(&report.display_path(path)))?;
            current = Some(path);
        }
        let (start, _) = span(diagnostic);
        writeln!(
            out,
            "  {}:{} {} {} {}",
            start.line,
            start.column,
            style.bold(diagnostic.rule.code()),
            style.severity(diagnostic.severity, &format!("[{}]", diagnostic.severity)),
            one_line(&diagnostic.message)
        )?;
    }
    if current.is_some() {
        writeln!(out)?;
    }
    Ok(())
}

pub(super) fn pylint(report: &Report<'_>, out: &mut dyn Write) -> io::Result<()> {
    for diagnostic in report.diagnostics {
        let (start, _) = span(diagnostic);
        writeln!(
            out,
            "{}:{}: [{}({})] {}",
            report.display_path(diagnostic.file.path()),
            start.line,
            diagnostic.rule.code(),
            diagnostic.rule.name(),
            one_line(&diagnostic.message)
        )?;
    }
    Ok(())
}

/// A source line prepared for display: tabs expanded, cropped, and the
/// display columns of the flagged part.
struct DisplayLine {
    text: String,
    underline_start: usize,
    underline_width: usize,
}

/// Bytes of a long line that are examined after the start of the flagged
/// range (half as many before it): more than a cropped line shows, so that
/// many findings on one huge line (a minified script) cost no more than on
/// short lines.
const SCAN_BYTES: usize = 4 * MAX_LINE_WIDTH;

/// Expands tabs in `line` and maps the byte range `start..end` of the line
/// to display columns, cropping long lines around the range.
fn display_line(line: &str, start: usize, end: usize) -> DisplayLine {
    if line.len() <= 2 * SCAN_BYTES {
        return display_window(line, start, end, false, false);
    }
    let mut lo = start.min(line.len()).saturating_sub(SCAN_BYTES / 2);
    while !line.is_char_boundary(lo) {
        lo -= 1;
    }
    let mut hi = (start + SCAN_BYTES).min(line.len());
    while !line.is_char_boundary(hi) {
        hi += 1;
    }
    let start = start.clamp(lo, hi);
    let end = end.clamp(start, hi);
    // `lo` and `hi` were moved to char boundaries of `line` above.
    #[allow(clippy::string_slice)]
    let window = &line[lo..hi];
    display_window(window, start - lo, end - lo, lo > 0, hi < line.len())
}

/// [`display_line`] for a part of a line; `cut_left` and `cut_right` say
/// whether the line continues on that side.
fn display_window(
    line: &str,
    start: usize,
    end: usize,
    cut_left: bool,
    cut_right: bool,
) -> DisplayLine {
    let mut chars: Vec<char> = Vec::with_capacity(line.len());
    let mut start_col = None;
    let mut end_col = None;
    for (byte, c) in line.char_indices() {
        if byte == start {
            start_col = Some(chars.len());
        }
        if byte == end {
            end_col = Some(chars.len());
        }
        if c == '\t' {
            let spaces = TAB_WIDTH - chars.len() % TAB_WIDTH;
            chars.extend(std::iter::repeat_n(' ', spaces));
        } else if c.is_control() {
            chars.push('\u{FFFD}');
        } else {
            chars.push(c);
        }
    }
    let start_col = start_col.unwrap_or(chars.len());
    let end_col = end_col.unwrap_or(chars.len()).max(start_col);
    let mut underline_start = start_col;
    let mut underline_width = end_col - start_col;
    if chars.len() <= MAX_LINE_WIDTH && !cut_left && !cut_right {
        return DisplayLine {
            text: chars.into_iter().collect(),
            underline_start,
            underline_width,
        };
    }
    let window_start = start_col.saturating_sub(CROP_CONTEXT);
    let window_end = (window_start + MAX_LINE_WIDTH).min(chars.len());
    let prefix = window_start > 0 || cut_left;
    let mut text = String::new();
    if prefix {
        text.push_str("...");
        underline_start = underline_start - window_start + 3;
    }
    text.extend(&chars[window_start..window_end]);
    let visible_end = window_end - window_start + usize::from(prefix) * 3;
    underline_width = underline_width.min(visible_end.saturating_sub(underline_start));
    if window_end < chars.len() || cut_right {
        text.push_str("...");
    }
    DisplayLine {
        text,
        underline_start,
        underline_width,
    }
}

fn code_frame(
    report: &Report<'_>,
    diagnostic: &Diagnostic,
    style: Style,
    out: &mut dyn Write,
) -> io::Result<()> {
    let text = diagnostic.file.text();
    let index = diagnostic.file.line_index();
    let range = diagnostic.range;
    let first = index.line_of(range.start());
    let mut last = index.line_of(range.end());
    if last > first && index.line_start(last) == range.end() {
        // The range ends with a line terminator: do not show the next line.
        last -= 1;
    }
    let lines: Vec<Option<usize>> = if last - first + 1 > MAX_FRAME_LINES {
        vec![
            Some(first),
            Some(first + 1),
            None,
            Some(last - 1),
            Some(last),
        ]
    } else {
        (first..=last).map(Some).collect()
    };
    let width = (last + 1).to_string().len();
    let blank = " ".repeat(width);
    let bar = style.gutter("|");
    writeln!(out, "{blank} {bar}")?;
    for line in lines {
        let Some(line) = line else {
            writeln!(out, "{}", style.gutter("..."))?;
            continue;
        };
        let line_range = index.line_range(line, text);
        let line_text = line_range.slice(text);
        let seg_start = range.start().max(line_range.start()) - line_range.start();
        let seg_end =
            range.end().min(line_range.end()).max(line_range.start()) - line_range.start();
        let shown = display_line(line_text, seg_start.to_usize(), seg_end.to_usize());
        let number = style.gutter(&format!("{:>width$}", line + 1));
        writeln!(
            out,
            "{}",
            format!("{number} {bar} {}", shown.text).trim_end()
        )?;
        let carets = if line == first {
            shown.underline_width.max(1)
        } else {
            shown.underline_width
        };
        if carets > 0 {
            writeln!(
                out,
                "{blank} {bar} {}{}",
                " ".repeat(shown.underline_start),
                style.severity(diagnostic.severity, &"^".repeat(carets))
            )?;
        }
    }
    writeln!(out, "{blank} {bar}")?;
    let eq = style.gutter("=");
    if let Some(node_path) = &diagnostic.node_path {
        writeln!(out, "{blank} {eq} {}: at {node_path}", style.bold("note"))?;
    }
    for label in &diagnostic.labels {
        let at = position(&label.file, label.range.start());
        writeln!(
            out,
            "{blank} {eq} {}: {} ({}:{}:{})",
            style.bold("note"),
            label.message,
            report.display_path(label.file.path()),
            at.line,
            at.column
        )?;
    }
    if let Some(fix) = &diagnostic.fix {
        let help = style.bold("help");
        match report.fix_state(diagnostic) {
            FixState::Applicable => writeln!(
                out,
                "{blank} {eq} {help}: {} {}",
                style.cyan("[*]"),
                fix.message
            )?,
            FixState::HiddenUnsafe => writeln!(
                out,
                "{blank} {eq} {help}: {} (unsafe fix available with `--unsafe-fixes`)",
                fix.message
            )?,
            FixState::DisplayOnly | FixState::None => {
                writeln!(out, "{blank} {eq} {help}: {}", fix.message)?
            }
        }
        if let Some(reason) = &fix.unsafe_reason {
            writeln!(
                out,
                "{blank} {eq} {}: the fix is unsafe because {}",
                style.bold("note"),
                reason.describe(|p| report.display_path(p))
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::display_line;

    #[test]
    fn tabs_expand_and_shift_the_underline() {
        let line = display_line("\tab\tc", 1, 3);
        assert_eq!(line.text, "    ab  c");
        assert_eq!((line.underline_start, line.underline_width), (4, 2));
    }

    #[test]
    fn long_lines_are_cropped_around_the_range() {
        let line = "x".repeat(200) + "TARGET" + &"y".repeat(200);
        let shown = display_line(&line, 200, 206);
        assert!(shown.text.starts_with("..."));
        assert!(shown.text.ends_with("..."));
        assert_eq!(
            shown
                .text
                .get(shown.underline_start..shown.underline_start + 6)
                .unwrap(),
            "TARGET"
        );
        assert_eq!(shown.underline_width, 6);
    }

    #[test]
    fn huge_lines_are_only_scanned_around_the_range() {
        let line = "é".repeat(100_000) + "TARGET" + &"y".repeat(200_000);
        let start = 200_000;
        let shown = display_line(&line, start, start + 6);
        assert!(shown.text.starts_with("..."));
        assert!(shown.text.ends_with("..."));
        let chars: Vec<char> = shown.text.chars().collect();
        let underlined: String = chars
            [shown.underline_start..shown.underline_start + shown.underline_width]
            .iter()
            .collect();
        assert_eq!(underlined, "TARGET");
        let at_start = display_line(&line, 0, 2);
        assert_eq!((at_start.underline_start, at_start.underline_width), (0, 1));
        assert!(!at_start.text.starts_with("..."));
        assert!(at_start.text.ends_with("..."));
    }

    #[test]
    fn unicode_columns_count_characters() {
        let line = display_line("é = x", 4, 5);
        assert_eq!((line.underline_start, line.underline_width), (3, 1));
    }
}
