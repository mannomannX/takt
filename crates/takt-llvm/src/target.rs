//! Zielklassen des Codegens (12.8).
//!
//! **Ein Target ist ein Triple, ein Datenlayout und eine Zielklasse.** Der
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
//!
//! **Ueber Klassengrenzen hinweg gilt das nicht mehr, und das ist der
//! Punkt.** Cortex-M4F rechnet `f32` in Hardware, RV32IMAC in Software
//! (12.8). Die IR ist dieselbe, die erzeugten Befehle sind es nicht — und
//! trotzdem muss das Ergebnis bitgleich sein, weil `libtaktm` korrekt
//! rundet und korrekt gerundete Ergebnisse eindeutig sind (4.2). Das ist
//! die schaerfste Probe von 9.4.4, die der Meilenstein hat.

/// Die Zielklassen aus 12.8.
///
/// Sie bestimmen Numerik und Erwartung an die Geschwindigkeit — nicht die
/// IR. Zwei Ziele derselben Klasse erzeugen identische IR; zwei Ziele
/// verschiedener Klassen unterscheiden sich in der Breite der Darstellung
/// (3.4) und darin, ob Fliesskomma in Hardware laeuft.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// 64-Bit Linux: x86-64, aarch64. FPU mit f32 und f64, FMA.
    Linux64,
    /// 32-Bit mit f32-FPU: Cortex-M4F, Cortex-M7, RV32IMFC. f64 in Software.
    Mcu32F32,
    /// 32-Bit ohne FPU: Cortex-M0+/M3, RV32IMAC. Fliesskomma in Software.
    Mcu32NoFpu,
}

impl Class {
    /// Name im Bericht (13.8: der Konformitaetsbericht geht je Zielklasse).
    pub fn name(self) -> &'static str {
        match self {
            Class::Linux64 => "64-Bit Linux",
            Class::Mcu32F32 => "32-Bit mit f32-FPU",
            Class::Mcu32NoFpu => "32-Bit ohne FPU",
        }
    }

    /// Rechnet die Klasse `f32` in Hardware?
    ///
    /// Auf `false` kommen die Operationen aus `libtaktm`. Bitgleich ist
    /// beides, weil korrekt gerundete Ergebnisse eindeutig sind (4.2) —
    /// der Unterschied liegt allein in den Kosten (9.4.3).
    pub fn has_f32_hardware(self) -> bool {
        matches!(self, Class::Linux64 | Class::Mcu32F32)
    }

    /// Rechnet die Klasse `f64` in Hardware?
    pub fn has_f64_hardware(self) -> bool {
        matches!(self, Class::Linux64)
    }
}

/// Ein Ziel, fuer das der Codegen erzeugen kann (12.8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    /// Das LLVM-Triple; es steht im Kopf der IR (11.3).
    ///
    /// **Nicht immer derselbe Name wie das Rust-Target.** Rust nennt sein
    /// RISC-V-Ziel `riscv32imac-unknown-none-elf` und kodiert die
    /// Erweiterungen im Triple; LLVM kennt dort nur
    /// `riscv32-unknown-none-elf` und nimmt die Erweiterungen ueber
    /// `-march` entgegen. Wer beides gleichsetzt, bekommt „unknown target
    /// triple" — und zwar erst beim Uebersetzen, nicht beim Bauen.
    pub triple: &'static str,
    /// Die Architekturerweiterungen fuer `-march`, wo LLVM sie braucht.
    ///
    /// Leer, wo das Triple sie schon traegt (x86-64, aarch64, thumbv7em).
    pub march: &'static str,
    /// Der Name in `system: target = …` (12.8).
    pub name: &'static str,
    /// Breite eines Zeigers in Byte.
    ///
    /// Sie geht in `takt size` (11.5) ein und in die Schwelle der
    /// Zeigeruebergabe (11.2).
    pub pointer: u32,
    /// Die Zielklasse (12.8).
    pub class: Class,
    /// Praefix der Cross-Werkzeugkette, etwa `arm-none-eabi-`.
    ///
    /// Leer fuer den Wirt. `inspect` liest Sektionen und Stackrahmen
    /// damit; das `nm` des Wirts liest ein fremdes Objekt nicht
    /// zuverlaessig.
    pub prefix: &'static str,
}

impl Target {
    /// x86-64 unter Linux (12.8: Zielklasse „64-Bit Linux").
    pub const X86_64_LINUX: Target = Target {
        triple: "x86_64-unknown-linux-gnu",
        name: "x86_64",
        march: "",
        pointer: 8,
        class: Class::Linux64,
        prefix: "",
    };

    /// aarch64 unter Linux, Cortex-A-Klasse (12.8: dieselbe Zielklasse).
    pub const AARCH64_LINUX: Target = Target {
        triple: "aarch64-unknown-linux-gnu",
        name: "aarch64",
        march: "",
        pointer: 8,
        class: Class::Linux64,
        prefix: "aarch64-linux-gnu-",
    };

    /// x86-64 unter Windows; fuer die Entwicklung, nicht fuer `linux_rt`.
    pub const X86_64_WINDOWS: Target = Target {
        triple: "x86_64-pc-windows-msvc",
        name: "x86_64-windows",
        march: "",
        pointer: 8,
        class: Class::Linux64,
        prefix: "",
    };

    /// Cortex-M4F, `baremetal` (12.3, M5).
    ///
    /// `eabihf` und nicht `eabi`: Die FPU ist da, und 4.2 verlangt, dass
    /// sie benutzt wird — mit geloeschtem FZ-Bit und nur ueber VFP, nie
    /// ueber NEON, das auf ARMv7 ohne Subnormale rechnet.
    pub const THUMBV7EM: Target = Target {
        triple: "thumbv7em-none-eabihf",
        name: "thumbv7em",
        march: "",
        pointer: 4,
        class: Class::Mcu32F32,
        prefix: "arm-none-eabi-",
    };

    /// RV32IMAC, `baremetal` (12.3, M5).
    ///
    /// Keine FPU: `f32` kommt aus `libtaktm`. Genau deshalb steht dieses
    /// Ziel neben [`Target::THUMBV7EM`] im Meilenstein — dieselbe Rechnung
    /// einmal in Hardware, einmal in Software, und beide muessen bitgleich
    /// sein (4.2, Satz 9.4.4).
    pub const RISCV32IMAC: Target = Target {
        // LLVM kennt nur den Basistriple; die Erweiterungen kommen ueber
        // `-march`. Das Rust-Target heisst anders — siehe `triple`.
        triple: "riscv32-unknown-none-elf",
        name: "riscv32imac",
        march: "rv32imac",
        pointer: 4,
        class: Class::Mcu32NoFpu,
        prefix: "riscv32-unknown-elf-",
    };

    /// Die Ziele, fuer die die Abnahme laeuft (13.8).
    ///
    /// 9.4.4 verlangt Bit-Gleichheit „ueber alle Targets"; diese Liste
    /// ist, was „alle" heute heisst.
    pub const ALL: [Target; 4] = [Target::X86_64_LINUX, Target::AARCH64_LINUX, Target::THUMBV7EM, Target::RISCV32IMAC];

    /// Alle benannten Ziele, auch die, die nicht in der Abnahme stehen.
    const KNOWN: [Target; 5] =
        [Target::X86_64_LINUX, Target::AARCH64_LINUX, Target::X86_64_WINDOWS, Target::THUMBV7EM, Target::RISCV32IMAC];

    /// Das Ziel zu einem Namen.
    pub fn by_name(name: &str) -> Option<Target> {
        Target::KNOWN.into_iter().find(|t| t.name == name)
    }

    /// Gehoeren zwei Ziele derselben Zielklasse an (12.8)?
    ///
    /// Innerhalb einer Klasse ist die IR dieselbe. Zwischen Klassen ist
    /// sie es nicht — die Darstellung ist schmaler (3.4) und Fliesskomma
    /// laeuft je nach Klasse in Hardware oder aus `libtaktm`.
    pub fn same_class(self, other: Target) -> bool {
        self.class == other.class
    }

    /// Laeuft das Ziel ohne Betriebssystem (12.3)?
    ///
    /// Entscheidet ueber das Laufzeitprofil (`baremetal` statt
    /// `linux_rt`) und ueber die Instrumentierung: 12.8 gibt `states`
    /// vor, wo `linux_rt` `statements` nimmt.
    ///
    /// Die Frage haengt an der Klasse, nicht an der Zeigerbreite: 12.8
    /// kennt mit Cortex-A7/A9 auch 32-Bit-Ziele *mit* Betriebssystem.
    pub fn is_bare_metal(self) -> bool {
        matches!(self.class, Class::Mcu32F32 | Class::Mcu32NoFpu)
    }
}
