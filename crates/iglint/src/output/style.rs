//! ANSI styling that is a no-op when colour is off.

use iglint_linter::Severity;

/// Applies ANSI styles when enabled.
#[derive(Clone, Copy, Debug)]
pub struct Style {
    enabled: bool,
}

impl Style {
    pub const fn new(enabled: bool) -> Self {
        Style { enabled }
    }

    fn paint(self, codes: &str, text: &str) -> String {
        if self.enabled && !text.is_empty() {
            format!("\x1b[{codes}m{text}\x1b[0m")
        } else {
            text.to_owned()
        }
    }

    pub fn bold(self, text: &str) -> String {
        self.paint("1", text)
    }

    pub fn green(self, text: &str) -> String {
        self.paint("32", text)
    }

    pub fn cyan(self, text: &str) -> String {
        self.paint("36", text)
    }

    /// Line-number gutter.
    pub fn gutter(self, text: &str) -> String {
        self.paint("1;34", text)
    }

    /// Red for errors, yellow for warnings, blue for info.
    pub fn severity(self, severity: Severity, text: &str) -> String {
        match severity {
            Severity::Error => self.paint("1;31", text),
            Severity::Warning => self.paint("1;33", text),
            Severity::Info => self.paint("1;34", text),
        }
    }
}
