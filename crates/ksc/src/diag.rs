//! Source spans and compiler error messages.

use std::fmt;

/// Byte range `start..end` in the source file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Span {
        Span { start, end }
    }

    /// Smallest span covering both.
    pub fn to(self, other: Span) -> Span {
        Span { start: self.start.min(other.start), end: self.end.max(other.end) }
    }
}

/// A compiler error (or warning) pointing at a place in the source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub message: String,
    pub span: Span,
    pub note: Option<String>,
    /// A warning does not stop the compilation (`@deprecated` uses).
    pub warning: bool,
}

impl Diagnostic {
    pub fn new(message: impl Into<String>, span: Span) -> Diagnostic {
        Diagnostic { message: message.into(), span, note: None, warning: false }
    }

    pub fn warning(message: impl Into<String>, span: Span) -> Diagnostic {
        Diagnostic { warning: true, ..Diagnostic::new(message, span) }
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Diagnostic {
        self.note = Some(note.into());
        self
    }

    /// Renders the error with the offending line and a caret underline:
    ///
    /// ```text
    /// error: unexpected character `!`
    ///  --> demo.ks:3:5
    ///   |
    /// 3 |     !x;
    ///   |     ^
    /// ```
    pub fn render(&self, path: &str, src: &str) -> String {
        let start = self.span.start.min(src.len());
        let line_start = src[..start].rfind('\n').map_or(0, |i| i + 1);
        let line_end = src[start..].find('\n').map_or(src.len(), |i| start + i);
        let line_no = src[..start].matches('\n').count() + 1;
        let col = src[line_start..start].chars().count() + 1;
        let text = &src[line_start..line_end];
        let width = src[start..self.span.end.clamp(start, line_end)].chars().count().max(1);

        let gutter = " ".repeat(line_no.to_string().len());
        let level = if self.warning { "warning" } else { "error" };
        let mut out = format!("{level}: {}\n{gutter}--> {path}:{line_no}:{col}\n{gutter} |\n", self.message);
        out += &format!("{line_no} | {text}\n");
        out += &format!("{gutter} | {}{}\n", " ".repeat(col - 1), "^".repeat(width));
        if let Some(note) = &self.note {
            out += &format!("{gutter} = note: {note}\n");
        }
        out
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Diagnostic {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_points_at_the_span() {
        let src = "proc main() {\n    x: int = 1 + ;\n}\n";
        let at = src.find(';').unwrap();
        let d = Diagnostic::new("expected an expression", Span::new(at, at + 1)).with_note("try `1 + 2`");
        assert_eq!(
            d.render("demo.ks", src),
            "error: expected an expression\n --> demo.ks:2:18\n  |\n2 |     x: int = 1 + ;\n  |                  ^\n  = note: try `1 + 2`\n"
        );
    }
}
