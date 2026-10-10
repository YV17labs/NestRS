//! What nestrs writes as a line — a console event, the panic hook's stderr
//! line — escaped so that no value in it can forge another line or hide one.

use std::fmt::{self, Write as _};

/// A value with every control, bidirectional override and invisible formatting
/// character written as its Rust debug escape (CWE-117).
///
/// It guarantees one line per event, not unambiguous fields: a value can still
/// read as `x trace_id=…`; JSON is the output a machine parses.
pub(crate) struct LineSafe<'a, T: ?Sized>(pub(crate) &'a T);

impl<T: fmt::Debug + ?Sized> fmt::Debug for LineSafe<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(LineSafeWriter(f), "{:?}", self.0)
    }
}

impl<T: fmt::Display + ?Sized> fmt::Display for LineSafe<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(LineSafeWriter(f), "{}", self.0)
    }
}

pub(crate) fn forges(ch: char) -> bool {
    ch.is_control()
        || matches!(
            ch,
            '\u{061C}' | '\u{200B}'..='\u{200F}' | '\u{2028}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}'
        )
}

struct LineSafeWriter<'a, 'f>(&'a mut fmt::Formatter<'f>);

impl fmt::Write for LineSafeWriter<'_, '_> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let mut run = 0;
        for (at, ch) in text.char_indices() {
            if forges(ch) {
                self.0.write_str(&text[run..at])?;
                for escaped in ch.escape_debug() {
                    self.0.write_char(escaped)?;
                }
                run = at + ch.len_utf8();
            }
        }
        self.0.write_str(&text[run..])
    }
}
