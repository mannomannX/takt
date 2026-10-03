//! Was die Tests aller Crates teilen (13.8).
//!
//! **Ein fehlendes Werkzeug ist ein Fehler, kein Bestehen** (FB-392). Ein
//! Test, der ohne clang, die LLVM-Werkzeuge oder den Solver still
//! zurueckkehrt, sieht aus wie ein bestandener: `cargo test` zeigt seine
//! Meldung nicht. [`require`] laesst ihn darum scheitern, wenn das
//! Werkzeug fehlt — es sei denn, die Umgebung erlaubt das Fehlen
//! ausdruecklich mit `TAKT_ALLOW_MISSING` (durch Kommas getrennt, `all`
//! fuer alle). Was es bewusst nur auf manchen Rechnern gibt — Boards, eine
//! zweite Werkzeugkette —, gehoert nicht hierher, sondern in Tests mit
//! `#[ignore = "…"]`: Die stehen im Ergebnis als `ignored`.

/// Die Umgebungsvariable, die das Fehlen von Werkzeugen erlaubt.
pub const ALLOW_MISSING: &str = "TAKT_ALLOW_MISSING";

/// Erlaubt `allowed`, den Wert von `TAKT_ALLOW_MISSING`, das Fehlen von `tool`?
pub fn missing_allowed(allowed: Option<&str>, tool: &str) -> bool {
    allowed.is_some_and(|list| list.split(',').map(str::trim).any(|t| t == tool || t == "all"))
}

/// Das Werkzeug `tool`, wenn `found` es nennt. Fehlt es, scheitert der
/// Test mit `hint`; erlaubt `TAKT_ALLOW_MISSING` das Fehlen, meldet er sich
/// als uebersprungen und bekommt `None`.
///
/// # Panics
///
/// Wenn das Werkzeug fehlt und `TAKT_ALLOW_MISSING` es nicht nennt.
pub fn require<T>(tool: &str, found: Option<T>, hint: &str) -> Option<T> {
    require_with(std::env::var(ALLOW_MISSING).ok().as_deref(), tool, found, hint)
}

/// [`require`] mit dem Wert von `TAKT_ALLOW_MISSING` als Argument.
///
/// # Panics
///
/// Wenn das Werkzeug fehlt und `allowed` es nicht nennt.
pub fn require_with<T>(allowed: Option<&str>, tool: &str, found: Option<T>, hint: &str) -> Option<T> {
    if found.is_some() {
        return found;
    }
    assert!(
        missing_allowed(allowed, tool),
        "`{tool}` fehlt: {hint}. Ohne `{tool}` ueberspringt nur `{ALLOW_MISSING}={tool}` diesen Test."
    );
    eprintln!("uebersprungen: `{tool}` fehlt ({ALLOW_MISSING})");
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_named_tool_may_be_missing() {
        assert!(missing_allowed(Some("clang"), "clang"));
        assert!(missing_allowed(Some(" solver , clang "), "clang"));
        assert!(missing_allowed(Some("all"), "solver"));
        assert!(!missing_allowed(Some("clangd"), "clang"));
        assert!(!missing_allowed(Some(""), "clang"));
        assert!(!missing_allowed(None, "clang"));
    }

    #[test]
    fn a_found_tool_passes_through() {
        assert_eq!(require_with(None, "clang", Some(7), "egal"), Some(7));
    }

    #[test]
    fn an_allowed_missing_tool_skips() {
        assert_eq!(require_with::<()>(Some("clang"), "clang", None, "egal"), None);
    }

    #[test]
    #[should_panic(expected = "`clang` fehlt")]
    fn a_missing_tool_fails_the_test() {
        let _ = require_with::<()>(Some("solver"), "clang", None, "nur fuer diesen Test");
    }
}
