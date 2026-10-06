//! Stapel fuer die rekursiven Durchlaeufe (2.1, FB-433).
//!
//! Mit den Grenzen aus 2.1 (64 Ebenen, 256 Knoten je Ausdruck) ist die
//! Rekursion jedes Werkzeugs beschraenkt, aber nicht klein. An der Grenze,
//! unter 55 Anweisungsebenen, brauchen Sema, Interpreter und Codegen im
//! Debug-Build 8, 8 und 2 MiB, im Release-Build 2 MiB, 512 KiB und 512 KiB
//! (Windows x86-64, gemessen 2026-10-06). Ein Hauptthread hat unter Windows
//! 1 MiB, ein Rust-Thread 2 MiB: Darum laufen die Einstiege auf einem
//! eigenen Thread, und nicht der Aufrufer entscheidet, was ein Werkzeug
//! annimmt.

/// Der Stapel eines Einstiegs: das Achtfache des groessten gemessenen
/// Bedarfs. Reserviert, nicht belegt; ein Lauf belegt nur, was er braucht.
pub const DEEP_STACK: usize = 64 << 20;

/// Fuehrt `f` auf einem Thread mit [`DEEP_STACK`] aus.
pub fn with_deep_stack<R: Send>(f: impl FnOnce() -> R + Send) -> R {
    with_stack(DEEP_STACK, f)
}

/// Fuehrt `f` auf einem Thread mit `bytes` Stapel aus und wartet auf ihn.
/// Eine Panik in `f` setzt sich im Aufrufer fort.
pub fn with_stack<R: Send>(bytes: usize, f: impl FnOnce() -> R + Send) -> R {
    std::thread::scope(|scope| {
        let thread = std::thread::Builder::new().stack_size(bytes).spawn_scoped(scope, f).expect("Thread mit Stapel");
        thread.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    })
}
