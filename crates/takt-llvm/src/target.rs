//! Zielklassen des Codegens (12.8).
//!
//! **Ein Target ist ein Triple und ein Datenlayout, mehr nicht.** Der
//! Codegen erzeugt fuer x86-64 und aarch64 dieselbe IR — beide sind in
//! 12.8 dieselbe Zielklasse („64-Bit Linux", 64 Register, f32 und f64 mit
//! FMA), und die Unterschiede liegen unterhalb der IR: Registerbelegung,
//! Befehlsauswahl, Aufrufkonvention. Das ist LLVMs Arbeit, nicht unsere.
//!
//! **Und genau darauf beruht Satz 9.4.4.** Waere die IR je Target eine
//! andere, waere die Bit-Gleichheit eine Behauptung ueber zwei Programme;
//! so ist sie eine ueber zwei Uebersetzungen desselben. Der Test in
//! `takt-conformance` prueft, dass die IR tatsaechlich dieselbe ist, und
//! nicht nur, dass beide Ergebnisse gleich aussehen.

/// Ein Ziel, fuer das der Codegen erzeugen kann (12.8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    /// Das LLVM-Triple; es steht im Kopf der IR (11.3).
    pub triple: &'static str,
    /// Der Name in `system: target = …` (12.8).
    pub name: &'static str,
    /// Breite eines Zeigers in Byte.
    ///
    /// Sie geht in `takt size` (11.5) ein und in die Schwelle der
    /// Zeigeruebergabe (11.2).
    pub pointer: u32,
}

impl Target {
    /// x86-64 unter Linux (12.8: Zielklasse „64-Bit Linux").
    pub const X86_64_LINUX: Target = Target { triple: "x86_64-unknown-linux-gnu", name: "x86_64", pointer: 8 };

    /// aarch64 unter Linux, Cortex-A-Klasse (12.8: dieselbe Zielklasse).
    pub const AARCH64_LINUX: Target = Target { triple: "aarch64-unknown-linux-gnu", name: "aarch64", pointer: 8 };

    /// x86-64 unter Windows; fuer die Entwicklung, nicht fuer `linux_rt`.
    pub const X86_64_WINDOWS: Target = Target { triple: "x86_64-pc-windows-msvc", name: "x86_64-windows", pointer: 8 };

    /// Die Ziele, fuer die die Abnahme laeuft (13.8).
    ///
    /// 9.4.4 verlangt Bit-Gleichheit „ueber alle Targets"; diese Liste
    /// ist, was „alle" heute heisst. Sie waechst mit M5 (baremetal).
    pub const ALL: [Target; 2] = [Target::X86_64_LINUX, Target::AARCH64_LINUX];

    /// Das Ziel zu einem Namen.
    pub fn by_name(name: &str) -> Option<Target> {
        [Target::X86_64_LINUX, Target::AARCH64_LINUX, Target::X86_64_WINDOWS].into_iter().find(|t| t.name == name)
    }

    /// Gehoeren zwei Ziele derselben Zielklasse an (12.8)?
    ///
    /// Innerhalb einer Klasse ist die IR dieselbe; zwischen Klassen
    /// unterscheidet sie sich in der Breite und in der Frage, ob es eine
    /// FPU gibt. Heute ist nur die 64-Bit-Klasse gebaut.
    pub fn same_class(self, other: Target) -> bool {
        self.pointer == other.pointer
    }
}
