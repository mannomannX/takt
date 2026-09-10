//! Schluesselwoerter (Referenz 2.2) und reservierte Woerter (2.5), Edition 1.
//!
//! Beide Listen sind sortiert, damit `binary_search` sie durchsucht. Der Test
//! `tests/keywords.rs` liest Abschnitt 2.2 aus `plan/definition.md` und vergleicht;
//! eine Abweichung bricht den Build.

/// Schluesselwoerter: leiten eine Aussage, Deklaration oder Klausel ein oder wirken
/// in Ausdruecken als Operator oder Literal (Regel in 2.2).
pub const KEYWORDS: &[&str] = &[
    "abort", "after", "alert", "always", "and", "arm", "as", "at", "block", "bound", "break",
    "campaign", "cancel", "case", "check", "command", "const", "default", "disarm", "driver",
    "elif", "else", "enter", "enum", "eventually", "every", "exit", "expect", "false", "fault",
    "fn", "for", "has", "if", "implies", "import", "in", "initial", "input", "instance", "job",
    "log", "loop", "machine", "match", "matches", "measure", "native", "never", "node", "none",
    "not", "on", "once", "or", "output", "param", "pass", "persist", "port", "profile",
    "program", "property", "pub", "pulse", "raise", "range", "record", "repeat", "return",
    "scenario", "send", "sequence", "signal", "stable", "state", "step", "stop_on", "stream",
    "sweep", "system", "then", "trigger", "true", "tunable", "type", "unit", "unitvec", "until",
    "var", "verdict", "verify", "wait", "when", "with",
];

/// Reservierte Woerter ohne Bedeutung; als Bezeichner verboten (2.5).
pub const RESERVED: &[&str] = &[
    "async", "await", "catch", "class", "defer", "export", "extern", "global", "impl", "module",
    "region", "select", "spawn", "static", "struct", "throw", "trait", "try", "union", "unsafe",
    "volatile", "where", "while", "yield",
];

/// Ist `word` ein Schluesselwort?
pub fn is_keyword(word: &str) -> bool {
    KEYWORDS.binary_search(&word).is_ok()
}

/// Ist `word` ein reserviertes Wort?
pub fn is_reserved(word: &str) -> bool {
    RESERVED.binary_search(&word).is_ok()
}
