//! Der C-Text eines Rahmens in drei Teilen (12.11).
//!
//! C verlangt die Typen vor der Arena und die Arena vor jeder Funktion, die
//! sie liest. Die Bausteine des Rahmens liefern aber jeder alle drei: einen
//! Typ, ihre Felder in der Arena und ihren Code. Jeder schreibt darum in
//! seinen Teil, und [`Text::assemble`] fuegt die Teile in der Reihenfolge
//! zusammen, die C braucht.

use std::fmt::Write as _;

use takt_llvm::arena::Arena;

/// Ein Rahmen in drei Teilen.
#[derive(Clone, Debug, Default)]
pub struct Text {
    /// Typen und Konstanten, die die Felder der Arena brauchen.
    pub types: String,
    /// Die Felder der Runtime in `struct takt_arena`, je mit Einrueckung und
    /// Semikolon.
    pub fields: String,
    /// Funktionen und Tabellen.
    pub code: String,
}

impl Text {
    /// Fuegt den Rahmen zusammen: `head`, die Typen, die Arena mit dem
    /// Programmbereich vorn und den Feldern der Runtime dahinter, die
    /// Pruefungen der Versaetze, dann der Code.
    ///
    /// Mit `pad_to` reicht der Programmbereich bis zu dieser Groesse: So
    /// deckt eine Schutzregion ihn genau und die Runtime dahinter nicht
    /// (12.3, 12.11).
    ///
    /// **Jeder Bereich ist auf acht Byte ausgerichtet.** Der erzeugte Code
    /// sieht ihn als Struktur mit `i64`-Feldern und liest mit `LDRD`; ein
    /// Byte-Array allein haette in C Ausrichtung 1. Auf ARMv7-M ist `LDRD`
    /// ohne Wortausrichtung ein UsageFault, der zum HardFault eskaliert —
    /// gefunden auf dem STM32F401, wo `state_blink` auf 0x2000_0036 lag.
    pub fn assemble(self, head: &str, arena: &Arena, pad_to: Option<u64>) -> String {
        let mut s = String::from(head);
        s.push_str(&self.types);
        let _ = writeln!(s, "\n/* Die Arena (12.11): vorn der Programmbereich, den der erzeugte Code adressiert,");
        let _ = writeln!(s, "   dahinter die Runtime. Der Wirt stellt sie fuer die Lebensdauer des Programms. */");
        let _ = writeln!(s, "struct takt_arena {{");
        for r in arena.regions() {
            let _ = writeln!(s, "    _Alignas(8) unsigned char {}[{}];", r.name, r.bytes);
        }
        if let Some(pad) = pad_to.filter(|pad| *pad > arena.bytes) {
            let _ = writeln!(s, "    unsigned char program_pad[{}];", pad - arena.bytes);
        }
        if !self.fields.is_empty() {
            let _ = writeln!(s, "    /* Runtime */");
            s.push_str(&self.fields);
        }
        let _ = writeln!(s, "}};");
        for r in arena.regions() {
            let _ = writeln!(
                s,
                "_Static_assert(offsetof(struct takt_arena, {0}) == {1}, \"{0} liegt, wo der Codegen ihn sucht\");",
                r.name, r.offset
            );
        }
        s.push('\n');
        s.push_str(&self.code);
        s
    }
}
