//! Der Textpuffer, der die IR aufbaut.
//!
//! **Keine Fliesskomma-Zeile traegt je ein Flag.** Das ist die ganze
//! strikte FP-Semantik aus 4.2 in der Textform: `fast`, `nnan`, `ninf`,
//! `nsz`, `arcp`, `contract` und `reassoc` kommen hier nicht vor, und
//! `tests/strict_fp.rs` prueft es. Darum gibt es keine Methode, die ein
//! Flag setzen koennte — was nicht existiert, wird nicht versehentlich
//! benutzt.

use core::fmt::Write;

use crate::ty::{INDIRECT_MIN, LlvmType};

/// Ein SSA-Register (`%0`, `%1`, ...).
///
/// LLVM verlangt in einer Funktion fortlaufende Nummern ab 0, wenn sie
/// unbenannt sind; der Zaehler steht darum in [`Module`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reg {
    /// Nummeriert; LLVM verlangt die Nummern streng aufsteigend.
    Num(u32),
    /// Benannt. Ein `alloca`, das nachtraeglich in den Eintrittsblock
    /// wandert, braucht das: Es steht dort vor Nummern, die spaeter
    /// vergeben wurden (FB-214).
    Named(u32),
}

impl core::fmt::Display for Reg {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Reg::Num(n) => write!(f, "%{n}"),
            Reg::Named(n) => write!(f, "%slot{n}"),
        }
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
    /// Name des laufenden Basisblocks; `phi` braucht ihn.
    block: String,
    /// Zaehler fuer Marken, die aus Ausdruecken entstehen.
    labels: u32,
    /// Wo im Rumpf der Eintrittsblock der laufenden Funktion beginnt.
    /// [`Module::alloca`] setzt seine Slots dorthin (FB-214).
    entry_at: usize,
    /// Zaehler der benannten Slots der laufenden Funktion.
    slots: u32,
    /// Instrumentierungsstufe (11.2): was die Schrittfunktionen in `pc`
    /// schreiben.
    pub instrument: crate::target::Instrument,
    /// Gerufene LLVM-Intrinsics mit ihrer Signatur.
    ///
    /// Sie sind je Breite eigene Symbole (`@llvm.smin.i8` ist nicht
    /// `@llvm.smin.i32`), also laesst sich die Liste nicht vorab
    /// schreiben — sie haengt an den Typen, die im Programm vorkommen.
    intrinsics: std::collections::BTreeSet<String>,
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
        Module {
            head,
            body: String::new(),
            next: 0,
            labels: 0,
            entry_at: 0,
            slots: 0,
            instrument: crate::target::Instrument::Off,
            open: false,
            block: String::new(),
            intrinsics: std::collections::BTreeSet::new(),
            terminated: false,
        }
    }

    /// Dasselbe Modul mit dieser Instrumentierungsstufe.
    pub fn with_instrument(mut self, instrument: crate::target::Instrument) -> Module {
        self.instrument = instrument;
        self
    }

    /// Beginnt eine Funktion.
    ///
    /// Die Parameter bekommen die Register 0 bis n-1; der Zaehler steht
    /// danach auf n, wie LLVM es verlangt.
    pub fn begin(&mut self, name: &str, ret: &LlvmType, params: &[LlvmType]) -> Vec<Reg> {
        debug_assert!(!self.open, "Funktion `{name}` beginnt in einer offenen Funktion");
        let regs: Vec<Reg> = (0..params.len() as u32).map(Reg::Num).collect();
        let sig: Vec<String> = params.iter().zip(&regs).map(|(t, r)| format!("{t} {r}")).collect();
        let _ = writeln!(self.body, "\ndefine {ret} @{name}({}) {{", sig.join(", "));
        self.entry_at = self.body.len();
        self.slots = 0;
        // Der Eintrittsblock bekommt eine Nummer wie ein Register.
        self.next = params.len() as u32 + 1;
        self.open = true;
        self.terminated = false;
        // LLVM nummeriert den Eintrittsblock wie ein Register; seine
        // Nummer ist die letzte vergebene.
        self.block = format!("{}", params.len());
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

    /// Wie [`Module::begin`], aber mit `sret`-Attribut fuer die Rueckgabe.
    pub fn begin_sig(&mut self, name: &str, sig: &crate::fns::Signature) -> Vec<Reg> {
        debug_assert!(!self.open, "Funktion `{name}` beginnt in einer offenen Funktion");
        let params = sig.params();
        let regs: Vec<Reg> = (0..params.len() as u32).map(Reg::Num).collect();
        let sig_text: Vec<String> = params
            .iter()
            .zip(&regs)
            .enumerate()
            .map(
                |(i, (t, r))| {
                    if i == 0 && sig.sret { format!("ptr sret({}) {r}", sig.ret) } else { format!("{t} {r}") }
                },
            )
            .collect();
        let _ = writeln!(self.body, "\ndefine {} @{name}({}) {{", sig.llvm_ret(), sig_text.join(", "));
        self.entry_at = self.body.len();
        self.slots = 0;
        self.next = params.len() as u32 + 1;
        self.open = true;
        self.terminated = false;
        self.block = format!("{}", params.len());
        regs
    }

    /// Ein frisches Register.
    pub fn reg(&mut self) -> Reg {
        let r = Reg::Num(self.next);
        self.next += 1;
        r
    }

    /// Schreibt eine Anweisung, die ein Register belegt, und liefert es.
    ///
    /// `rest` ist alles nach dem `=`, etwa `fadd double %1, %2`. Es gibt
    /// bewusst keine Variante, die Flags anhaengt (4.2).
    pub fn inst(&mut self, rest: &str) -> Reg {
        let r = self.reg();
        if self.terminated {
            // Unerreichbar: Der Block endete schon. Die Nummer wird
            // trotzdem vergeben, damit der Aufrufer eine gueltige
            // Referenz bekommt; geschrieben wird nichts.
            return r;
        }
        let _ = writeln!(self.body, "  {r} = {rest}");
        r
    }

    /// Terminiert der Text einen Basisblock?
    fn is_terminator(text: &str) -> bool {
        let head = text.split_whitespace().next().unwrap_or("");
        matches!(head, "br" | "ret" | "switch" | "unreachable" | "indirectbr" | "resume")
    }

    /// Ein Stack-Slot, immer im Eintrittsblock.
    ///
    /// **Warum nicht dort, wo er gebraucht wird.** Ein `alloca` in einem
    /// Schleifenrumpf ist semantisch eine Allokation *je Durchlauf*; LLVM
    /// darf ihn nicht hochziehen und `mem2reg` promotet ihn nicht. Bei
    /// `bytes<1024>` bleibt dann pro Iteration ein 1028-Byte-Slot stehen
    /// (FB-214). Im Eintrittsblock ist er ein Slot fuer die ganze
    /// Funktion — so legt C seine Locals an.
    pub fn alloca(&mut self, ty: &LlvmType) -> Reg {
        let r = Reg::Named(self.slots);
        self.slots += 1;
        let line = format!("  {r} = alloca {ty}\n");
        self.body.insert_str(self.entry_at, &line);
        self.entry_at += line.len();
        // Die Lebensdauer beginnt hier, nicht im Eintrittsblock: So kann
        // LLVM Slots mit getrennten Lebensdauern uebereinanderlegen —
        // sonst braucht jeder seinen eigenen Platz fuer die ganze
        // Funktion, und `uart_link_step` hatte 15 KB Rahmen.
        self.void_inst(&format!("call void @llvm.lifetime.start.p0(ptr {r})"));
        r
    }

    /// Wie viele Slots die laufende Funktion bis jetzt hat.
    pub fn slot_mark(&self) -> u32 {
        self.slots
    }

    /// Beendet die Lebensdauer aller Slots seit `from` — am Ende der
    /// Anweisung, in der sie entstanden. Ein Slot lebt nie ueber eine
    /// Anweisung hinaus: Was daraus weiterverwendet wird, ist geladen.
    pub fn end_slots(&mut self, from: u32) {
        for n in from..self.slots {
            self.void_inst(&format!("call void @llvm.lifetime.end.p0(ptr {})", Reg::Named(n)));
        }
    }

    /// Schreibt eine Anweisung ohne Ergebnis (`store`, `br`).
    pub fn void_inst(&mut self, text: &str) {
        if self.terminated {
            return;
        }
        let _ = writeln!(self.body, "  {text}");
        if Module::is_terminator(text) {
            self.terminated = true;
        }
    }

    /// Schreibt einen Wert an eine Stelle.
    ///
    /// Ein Wert in der Hand ist ein `store`; erst wo eine *Stelle* zu
    /// kopieren ist, lohnt [`Module::copy`].
    pub fn write(&mut self, ty: &LlvmType, value: &str, dst: &str) {
        // `store zeroinitializer` schreibt LLVM Feld fuer Feld; `memset`
        // ist dieselbe Aussage in einem Aufruf (FB-214).
        if value == "zeroinitializer" && ty.size() > INDIRECT_MIN {
            self.void_inst(&format!("call void @llvm.memset.p0.i64(ptr {dst}, i8 0, i64 {}, i1 false)", ty.size()));
            return;
        }
        self.void_inst(&format!("store {ty} {value}, ptr {dst}"));
    }

    /// Kopiert `ty` von `src` nach `dst`, ohne den Wert zu materialisieren.
    ///
    /// **Warum nicht `load` und `store`.** Ein Aggregat als Wert zerlegt
    /// LLVM in Felder und schreibt jedes einzeln; bei `bytes<1024>` sind
    /// das tausend Byte-Zugriffe mit je eigener Adressrechnung (FB-214).
    /// `memmove` ist dieselbe Aussage in einem Aufruf, den das Backend
    /// kennt — und `memmove` statt `memcpy`, weil ein `inout` dieselbe
    /// Stelle als Quelle und Ziel gibt (3.9).
    pub fn copy(&mut self, ty: &LlvmType, src: &str, dst: &str) {
        if ty.size() <= INDIRECT_MIN {
            let v = self.inst(&format!("load {ty}, ptr {src}"));
            self.void_inst(&format!("store {ty} {v}, ptr {dst}"));
            return;
        }
        // `memmove` und nicht `memcpy`: Ein `inout` gibt dieselbe Stelle
        // als Quelle und Ziel (3.9), und `memcpy` verlangt, dass sie sich
        // nicht ueberlappen. Der Unterschied kostete `39_sha256` seinen
        // Hashwert.
        self.void_inst(&format!(
            "call void @llvm.memmove.p0.p0.i64(ptr {dst}, ptr {src}, i64 {}, i1 false)",
            ty.size()
        ));
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
        self.block = name.to_string();
        self.terminated = false;
    }

    /// Ist der laufende Basisblock terminiert?
    pub fn terminated(&self) -> bool {
        self.terminated
    }

    /// Eine frische Nummer fuer eine Marke.
    ///
    /// Ausdruecke koennen Zweige brauchen (`decode`, 3.7), und sie kennen
    /// den Kontext nicht, der sonst die Nummern vergibt. Der Zaehler des
    /// Moduls ist die Stelle, die beide erreichen.
    pub fn next_label(&mut self) -> u32 {
        self.labels += 1;
        self.labels
    }

    /// Merkt ein gerufenes Intrinsic vor; die Deklaration entsteht beim
    /// Abschluss.
    ///
    /// `signature` ist die vollstaendige Zeile ohne `declare`, etwa
    /// `i8 @llvm.smin.i8(i8, i8)`.
    pub fn needs_intrinsic(&mut self, signature: &str) {
        self.intrinsics.insert(signature.to_string());
    }

    /// Der Name des laufenden Basisblocks.
    ///
    /// `phi` nennt seine Vorgaenger beim Namen; wer ihn raet, erzeugt
    /// stillen Unsinn, den erst LLVM findet.
    pub fn block(&self) -> &str {
        &self.block
    }

    /// Bricht die laufende Funktion ab und verwirft sie.
    ///
    /// Wird gebraucht, wenn ein Koerper etwas enthaelt, das der Codegen
    /// nicht senkt: Die halb geschriebene Funktion faellt weg, statt als
    /// ungueltige IR stehen zu bleiben. Eine halbe Schrittfunktion waere
    /// schlimmer als keine, weil sie moeglicherweise uebersetzt.
    pub fn abort(&mut self, from: usize) {
        self.body.truncate(from);
        self.open = false;
        self.terminated = true;
    }

    /// Die Laenge des Rumpfes; Merkzeichen fuer [`Module::abort`].
    pub fn mark(&self) -> usize {
        self.body.len()
    }

    /// Eine Zeile in den Kopf, etwa eine Deklaration.
    pub fn declare(&mut self, text: &str) {
        let _ = writeln!(self.head, "{text}");
    }

    /// Das fertige Modul als Text.
    pub fn finish(self) -> String {
        debug_assert!(!self.open, "eine Funktion ist noch offen");
        let mut decls = String::new();
        for sig in &self.intrinsics {
            let _ = writeln!(decls, "declare {sig}");
        }
        format!("{}{decls}{}", self.head, self.body)
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
