//! Der Textpuffer, der die IR aufbaut.
//!
//! **Keine Fliesskomma-Zeile traegt je ein Flag.** Das ist die ganze
//! strikte FP-Semantik aus 4.2 in der Textform: `fast`, `nnan`, `ninf`,
//! `nsz`, `arcp`, `contract` und `reassoc` kommen hier nicht vor, und
//! `tests/strict_fp.rs` prueft es. Darum gibt es keine Methode, die ein
//! Flag setzen koennte — was nicht existiert, wird nicht versehentlich
//! benutzt.

use core::fmt::Write;

use crate::ty::LlvmType;

/// Ein SSA-Register (`%0`, `%1`, ...).
///
/// LLVM verlangt in einer Funktion fortlaufende Nummern ab 0, wenn sie
/// unbenannt sind; der Zaehler steht darum in [`Module`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reg(pub u32);

impl core::fmt::Display for Reg {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "%{}", self.0)
    }
}

/// Ein Modul im Aufbau.
#[derive(Debug)]
pub struct Module {
    /// Der Kopf: `target`, Datenlayout, Kommentare.
    head: String,
    /// Die Funktionen.
    body: String,
    /// Naechste freie Registernummer der laufenden Funktion.
    next: u32,
    /// Laeuft gerade eine Funktion?
    open: bool,
    /// Ist der laufende Basisblock schon terminiert?
    ///
    /// LLVM verlangt, dass jeder Block mit `br`, `ret`, `switch` oder
    /// `unreachable` endet. Ein Block, der in die naechste Marke faellt,
    /// assembliert nicht — und das faellt ohne diese Buchfuehrung erst
    /// auf, wenn LLVM die Datei liest.
    terminated: bool,
}

impl Module {
    /// Ein leeres Modul mit Kopf.
    ///
    /// `triple` gehoert in den Kopf, weil 11.3 reproduzierbare Builds
    /// verlangt: Dieselbe Quelle plus dieselbe Toolchain ergibt dieselbe
    /// Datei, und das Target ist Teil davon. Ein Zeitstempel oder ein
    /// Pfad steht *nicht* darin — 11.3 schliesst beides aus.
    pub fn new(name: &str, triple: &str) -> Module {
        let mut head = String::new();
        let _ = writeln!(head, "; {name}");
        let _ = writeln!(head, "; erzeugt von takt-llvm; strikte FP nach Referenz 4.2:");
        let _ = writeln!(head, "; keine Fast-Math-Flags, contract=off, keine Reassoziation");
        let _ = writeln!(head, "target triple = \"{triple}\"");
        Module { head, body: String::new(), next: 0, open: false, terminated: false }
    }

    /// Beginnt eine Funktion.
    ///
    /// Die Parameter bekommen die Register 0 bis n-1; der Zaehler steht
    /// danach auf n, wie LLVM es verlangt.
    pub fn begin(&mut self, name: &str, ret: &LlvmType, params: &[LlvmType]) -> Vec<Reg> {
        debug_assert!(!self.open, "Funktion `{name}` beginnt in einer offenen Funktion");
        let regs: Vec<Reg> = (0..params.len() as u32).map(Reg).collect();
        let sig: Vec<String> = params.iter().zip(&regs).map(|(t, r)| format!("{t} {r}")).collect();
        let _ = writeln!(self.body, "\ndefine {ret} @{name}({}) {{", sig.join(", "));
        // Der Eintrittsblock bekommt eine Nummer wie ein Register.
        self.next = params.len() as u32 + 1;
        self.open = true;
        self.terminated = false;
        regs
    }

    /// Schliesst die Funktion mit `ret`.
    ///
    /// Ist der laufende Block schon terminiert, entsteht kein zweiter
    /// `ret` — LLVM liesse ihn nicht zu.
    pub fn end(&mut self, ret: Option<(&LlvmType, String)>) {
        if self.terminated {
            let _ = writeln!(self.body, "}}");
            self.open = false;
            return;
        }
        match ret {
            Some((t, v)) => {
                let _ = writeln!(self.body, "  ret {t} {v}");
            }
            None => {
                let _ = writeln!(self.body, "  ret void");
            }
        }
        let _ = writeln!(self.body, "}}");
        self.open = false;
        self.terminated = true;
    }

    /// Ein frisches Register.
    pub fn reg(&mut self) -> Reg {
        let r = Reg(self.next);
        self.next += 1;
        r
    }

    /// Schreibt eine Anweisung, die ein Register belegt, und liefert es.
    ///
    /// `rest` ist alles nach dem `=`, etwa `fadd double %1, %2`. Es gibt
    /// bewusst keine Variante, die Flags anhaengt (4.2).
    pub fn inst(&mut self, rest: &str) -> Reg {
        let r = self.reg();
        let _ = writeln!(self.body, "  {r} = {rest}");
        r
    }

    /// Terminiert der Text einen Basisblock?
    fn is_terminator(text: &str) -> bool {
        let head = text.split_whitespace().next().unwrap_or("");
        matches!(head, "br" | "ret" | "switch" | "unreachable" | "indirectbr" | "resume")
    }

    /// Schreibt eine Anweisung ohne Ergebnis (`store`, `br`).
    pub fn void_inst(&mut self, text: &str) {
        let _ = writeln!(self.body, "  {text}");
        if Module::is_terminator(text) {
            self.terminated = true;
        }
    }

    /// Setzt eine Marke (Basisblock).
    ///
    /// Faellt der laufende Block in die Marke, wird der Sprung dorthin
    /// ergaenzt: LLVM verlangt einen Terminator, und ein Durchfallen ist
    /// in der Textform ohnehin nicht ausdrueckbar. Das ist keine Reparatur
    /// eines Fehlers, sondern die Uebersetzung von „weiter mit" in die
    /// Form, die LLVM dafuer hat.
    pub fn label(&mut self, name: &str) {
        if self.open && !self.terminated {
            let _ = writeln!(self.body, "  br label %{name}");
        }
        let _ = writeln!(self.body, "{name}:");
        self.terminated = false;
    }

    /// Ist der laufende Basisblock terminiert?
    pub fn terminated(&self) -> bool {
        self.terminated
    }

    /// Eine Zeile in den Kopf, etwa eine Deklaration.
    pub fn declare(&mut self, text: &str) {
        let _ = writeln!(self.head, "{text}");
    }

    /// Das fertige Modul als Text.
    pub fn finish(self) -> String {
        debug_assert!(!self.open, "eine Funktion ist noch offen");
        format!("{}{}", self.head, self.body)
    }
}

/// Ein Fliesskommaliteral, wie LLVM es liest.
///
/// LLVM verlangt fuer `double` entweder eine Dezimalzahl, die sich exakt
/// darstellen laesst, oder die hexadezimale Form `0x...` mit dem
/// Bitmuster. Die Dezimalform ist die Quelle stiller Abweichungen — `0.1`
/// ist nicht `0.1` —, darum immer das Bitmuster: Es ist exakt und
/// dasselbe auf jedem Target (4.2, Satz 9.4.4).
pub fn float_literal(v: f64, ty: &LlvmType) -> String {
    match ty {
        // LLVM erwartet auch fuer `float` das Muster eines `double`; die
        // Umrechnung ist verlustfrei, weil jeder `f32` ein `f64` ist.
        LlvmType::F32 => format!("0x{:016X}", f64::from(v as f32).to_bits()),
        _ => format!("0x{:016X}", v.to_bits()),
    }
}
