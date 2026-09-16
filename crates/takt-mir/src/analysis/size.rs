//! Speicherbudget (Referenz 11.5, 11.2).
//!
//! Jeder Posten traegt seine **Herkunft**: `exakt` (aus der MIR gerechnet),
//! `Vertrag` (aus einer Deklaration uebernommen) oder `offen` (die Eingabe
//! fehlt noch). Summiert wird nur Belastbares — eine Summe, die
//! Geschaetztes einrechnet, ist schlechter als keine, weil ihr niemand
//! ansieht, welchem Teil er trauen kann (11.5).

use crate::machine::{Guard, Machine, TransTrigger};
use crate::pattern::Pattern;
use crate::types::{FloatWidth, IntWidth, Type};
use crate::{Program, TypeId};

/// Woher die Zahl eines Postens stammt (11.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// Aus der MIR gerechnet.
    Exact,
    /// Aus einer Deklaration uebernommen (etwa ein `stack`-Vertrag, 4.5).
    Contract,
    /// Aus dem erzeugten Objekt gelesen (Sektionsgroessen nach dem Link).
    ///
    /// Eine eigene Herkunft, weil der Posten weder aus der MIR rechenbar
    /// noch deklariert ist — nach dem Link aber exakt. Er unterscheidet
    /// sich damit von `Open`: Eine Profilreserve braucht Hardware und
    /// einen Lauf unter Last (13.8), eine Sektionsgroesse nur den Linker.
    Measured,
    /// Die Eingabe fehlt noch (Profilreserve ohne Kalibrierung).
    Open,
}

impl Origin {
    /// Name im Report.
    pub fn name(self) -> &'static str {
        match self {
            Origin::Exact => "exakt",
            Origin::Contract => "Vertrag",
            Origin::Measured => "gemessen",
            Origin::Open => "offen",
        }
    }
}

/// Ein Posten des Speicherbudgets.
#[derive(Clone, Debug)]
pub struct Item {
    /// Bezeichnung.
    pub name: String,
    /// Bytes.
    pub bytes: u64,
    /// Herkunft.
    pub origin: Origin,
}

/// Das Budget eines Programms.
#[derive(Clone, Debug, Default)]
pub struct Size {
    /// Die Posten in Ausgabereihenfolge.
    pub items: Vec<Item>,
}

impl Size {
    /// Summe der belastbaren Posten (`exakt`, `Vertrag` und `gemessen`).
    pub fn total(&self) -> u64 {
        self.items.iter().filter(|i| i.origin != Origin::Open).map(|i| i.bytes).sum()
    }

    /// Gibt es Posten, deren Eingabe fehlt?
    pub fn has_open(&self) -> bool {
        self.items.iter().any(|i| i.origin == Origin::Open)
    }

    /// Der Report als Zeilen.
    pub fn lines(&self) -> Vec<String> {
        let w = self.items.iter().map(|i| i.name.len()).max().unwrap_or(0);
        let mut out: Vec<String> =
            self.items.iter().map(|i| format!("  {:w$}  {:>9}  {}", i.name, i.bytes, i.origin.name())).collect();
        out.push(format!("  {:w$}  {:>9}  belastbar", "Summe", self.total()));
        if self.has_open() {
            out.push("  (Posten mit `offen` fehlen in der Summe: die Eingabe kommt mit 8.10 und 13.8)".into());
        }
        out
    }
}

/// Rechnet das Speicherbudget eines Programms (11.5).
pub fn size(p: &Program) -> Size {
    let mut items = Vec::new();

    let states: u64 = p.machines.iter().map(|m| machine_bytes(p, m)).sum();
    items.push(Item { name: "Maschinenzustaende (Overlay)".into(), bytes: states, origin: Origin::Exact });

    let streams: u64 = p
        .streams
        .iter()
        .map(|s| {
            let elem = u64::from(s.capacity) * u64::from(elem_bytes(p, s.elem));
            let ring = u64::from(s.capacity_bytes.unwrap_or(0));
            // Deskriptorring: seq, t und Laenge je Element.
            elem + ring + u64::from(s.capacity) * 20
        })
        .sum();
    items.push(Item { name: "Stroeme (Byte- und Deskriptorringe)".into(), bytes: streams, origin: Origin::Exact });

    let image: u64 = p.channels.iter().map(|c| u64::from(type_bytes(p, c.ty)) * 2).sum();
    items.push(Item { name: "Prozessabbild und Psi".into(), bytes: image, origin: Origin::Exact });

    let sched: u64 = p
        .machines
        .iter()
        // K_o: hoechstens vier geplante Schreibvorgaenge je Output (9.8).
        .map(|m| m.layout.output_queues.len() as u64 * 4 * 16)
        .sum();
    items.push(Item { name: "sched-Warteschlangen".into(), bytes: sched, origin: Origin::Exact });

    let scratch: u64 = p.machines.iter().map(|m| u64::from(m.layout.scratch_bytes.unwrap_or(0))).sum();
    items.push(Item { name: "Scratch je Maschine".into(), bytes: scratch, origin: Origin::Exact });

    // 11.5: die vorkompilierten Automaten der Muster (8.7). Die Rechnung
    // steht; gefuellt sind die Tabellen erst, wenn der Codegen sie erzeugt
    // — der Interpreter gleicht direkt ab und braucht sie nicht
    // (plan/m2.md 1.1). Bis dahin ist der Posten `offen`, nicht `exakt`:
    // eine Null, die noch niemand gerechnet hat, ist kein Messwert.
    let dfa = dfa_bytes(p);
    let origin = if dfa > 0 { Origin::Exact } else { Origin::Open };
    items.push(Item { name: "DFA-Tabellen der Muster".into(), bytes: dfa, origin });

    // 4.5: der Stack einer nativen Funktion steht in ihrem Vertrag.
    let native_stack: u64 = p.natives.iter().map(|n| u64::from(n.stack)).sum();
    items.push(Item { name: "Stack nativer Funktionen".into(), bytes: native_stack, origin: Origin::Contract });

    // 5.9: Das Journal haelt den Schreibpuffer und den Vergleichsstand —
    // zweimal die Nutzlast. Der Flash-Anteil braucht die Sektorgroesse aus
    // 8.10 und steht darum in [`with_hardware`].
    if crate::persist::any(p) {
        let (bytes, origin) = match crate::persist::max_payload(p) {
            Some(n) => (u64::from(n) * 2, Origin::Exact),
            None => (0, Origin::Open),
        };
        items.push(Item { name: "persist-Journal (RAM)".into(), bytes, origin });
        items.push(Item { name: "persist-Journal (Flash)".into(), bytes: 0, origin: Origin::Open });
    }

    // Was ohne Hardware-Konfiguration (8.10) und Kalibrierung (13.8) fehlt.
    items.push(Item { name: "Runtime-Reserven je Profil".into(), bytes: 0, origin: Origin::Open });

    // Flash und Stack braeuchten ein Objekt; ohne eines bleiben sie
    // offen. [`with_object`] fuellt sie.
    items.push(Item { name: "Flash (Code, Konstanten)".into(), bytes: 0, origin: Origin::Open });
    items.push(Item { name: "Stack (Programmanteil)".into(), bytes: 0, origin: Origin::Open });

    Size { items }
}

/// Was sich erst an einem erzeugten Objekt ablesen laesst (11.5, 12.3).
///
/// Zwei Posten stehen ohne Objekt auf `offen`, und zwar nicht aus
/// Unfertigkeit: Wie viele Bytes der Code belegt und wie tief er den Stack
/// nimmt, entscheidet der Codegen, nicht die MIR. `takt size --object`
/// reicht die Messung nach.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Measured {
    /// Code und Konstanten aus den Sektionsgroessen.
    pub flash: Option<u64>,
    /// Der Programmanteil des Stacks als laengster Pfad (12.3).
    pub stack: Option<super::stack::Depth>,
}

impl Size {
    /// Ersetzt die offenen Posten durch Messwerte.
    ///
    /// **Ein fehlender Messwert laesst den Posten offen**, statt ihn auf
    /// null zu setzen: Eine Null, die niemand gemessen hat, waere in der
    /// Summe eine Luege — und die Summe heisst „belastbar".
    pub fn with_object(mut self, m: &Measured) -> Size {
        for item in &mut self.items {
            let value = match item.name.as_str() {
                "Flash (Code, Konstanten)" => m.flash,
                "Stack (Programmanteil)" => m.stack.as_ref().map(|d| d.bytes),
                _ => continue,
            };
            if let Some(bytes) = value {
                item.bytes = bytes;
                item.origin = Origin::Measured;
            }
        }
        self
    }

    /// Fuellt die Posten, die an der Hardware-Konfiguration haengen (8.10).
    ///
    /// Heute ist das der Flash-Anteil des `persist`-Journals: Zwei Slots
    /// belegen ganze Sektoren, und wie gross die sind, weiss nur das Ziel.
    pub fn with_hardware(mut self, t: &crate::hardware::Target) -> Size {
        let Some(nvm) = t.nvm else { return self };
        if nvm.sector_bytes == 0 || nvm.sectors == 0 {
            return self;
        }
        for item in &mut self.items {
            if item.name == "persist-Journal (Flash)" {
                item.bytes = u64::from(nvm.sector_bytes) * u64::from(nvm.sectors);
                item.origin = Origin::Exact;
            }
        }
        self
    }
}

/// Der Speicher einer Maschine mit Overlay (11.2): Geschwisterzustaende
/// teilen sich den Platz ihrer zustandslokalen Variablen, weil nie zwei
/// gleichzeitig aktiv sind.
/// Speicher einer Maschine in Byte: maschinenweite Variablen plus das
/// Overlay der exklusiven Zustaende (11.2). Pruefung 62 vergleicht sie
/// mit dem deklarierten `budget = {ram = …}`.
pub fn machine_bytes(p: &Program, m: &Machine) -> u64 {
    // Maschinenweite Variablen liegen immer.
    let machine_vars: u64 = m
        .vars
        .iter()
        .filter(|v| matches!(v.scope, crate::machine::VarScope::Machine))
        .map(|v| u64::from(type_bytes(p, v.ty)))
        .sum();

    // Zustandslokale Variablen: je Zustand summiert, ueber Geschwister das
    // Maximum. Die Wurzeln schliessen einander aus.
    let state_vars = overlay(p, m, &m.roots);

    // Konfigurationspfad, Timer, Zaehler, Cursor (11.2).
    let depth = m.states.len().max(1) as u64;
    let fixed = depth /* conf */ + depth * 8 /* time_in_state */
        + m.layout.every_counters.len() as u64 * 8
        + m.layout.viol_sites.len() as u64 * 8
        + m.layout.cursors.len() as u64 * 8
        + 24 /* pending, last_fault, pc */;

    machine_vars + state_vars + fixed
}

/// Das Maximum ueber einander ausschliessende Geschwister, rekursiv.
fn overlay(p: &Program, m: &Machine, siblings: &[crate::StateId]) -> u64 {
    siblings
        .iter()
        .map(|id| {
            let s = &m.states[id.index()];
            let own: u64 = m
                .vars
                .iter()
                .filter(|v| matches!(v.scope, crate::machine::VarScope::State(x) | crate::machine::VarScope::Lifted(x) if x == *id))
                .map(|v| u64::from(type_bytes(p, v.ty)))
                .sum();
            own + overlay(p, m, &s.children)
        })
        .max()
        .unwrap_or(0)
}

/// Bytes eines Elementtyps in einem Stream.
fn elem_bytes(p: &Program, ty: TypeId) -> u32 {
    type_bytes(p, ty)
}

/// Grosse eines Typs im Speicher. Bewusst einfach: die MIR kennt keine
/// Ausrichtung, und `takt size` nennt Groessenordnungen, keine Offsets.
pub fn type_bytes(p: &Program, ty: TypeId) -> u32 {
    match p.types.list.get(ty.index()) {
        Some(Type::Bool) => 1,
        Some(Type::Int { width, .. }) => match width {
            IntWidth::I8 | IntWidth::U8 => 1,
            IntWidth::I16 | IntWidth::U16 => 2,
            IntWidth::I32 | IntWidth::U32 => 4,
            _ => 8,
        },
        Some(Type::Float { width, .. }) => match width {
            FloatWidth::F32 => 4,
            FloatWidth::F64 => 8,
        },
        Some(Type::Duration { .. }) => 8,
        Some(Type::Enum(_)) => 1,
        Some(Type::Record(r)) => {
            let def = &p.records[r.index()];
            def.wire_size.unwrap_or_else(|| def.fields.iter().map(|f| type_bytes(p, f.ty)).sum())
        }
        Some(Type::Array { elem, len } | Type::Samples { elem, len }) => type_bytes(p, *elem) * len,
        Some(Type::Vec { elem, cap }) => type_bytes(p, *elem) * cap + 4,
        Some(Type::Bytes { cap } | Type::Str { cap } | Type::Line { cap }) => cap + 4,
        Some(Type::Map { key, value, cap }) => (type_bytes(p, *key) + type_bytes(p, *value) + 1) * cap,
        Some(Type::Optional(inner)) => type_bytes(p, *inner) + 1,
        // Der Fehlerteil ist ein Enum, also eine Diskriminante (3.8).
        Some(Type::Result { ok, .. }) => type_bytes(p, *ok).max(1) + 1,
        Some(Type::Mat { rows, cols, .. }) => rows * cols * 8,
        _ => 8,
    }
}

/// Byte der vorkompilierten Musterautomaten (8.7, 11.5): Klassentabelle
/// (256 Byte) und Uebergangstabelle (`states × class_count`, je 4 Byte).
///
/// **Gezaehlt wird, was der Codegen emittiert** — nicht, was die MIR
/// traegt. Drei Unterschiede, jeder davon gemessen (FB-121):
///
/// - Die akzeptierenden Zustaende stehen *nicht* als Tabelle im Objekt.
///   `dfa::run` macht daraus eine Vergleichskette, weil die Menge zur
///   Uebersetzungszeit feststeht und typisch ein- bis dreielementig ist.
/// - Ein Muster mit Platzhaltern bekommt keinen Automaten: Der Vergleich
///   laeuft dort ueber `captures::walk`, weil der Automat zwar sagt, *ob*
///   ein Muster trifft, nicht aber was in `{n:int}` steht (8.7).
/// - Ein Record-Muster hat ohnehin keinen — es ist eine Konjunktion von
///   Feldgleichheiten.
///
/// Ohne diese drei rechnete der Posten bei `23_patterns` 1696 Byte gegen
/// 424 gemessene. Eine Zahl, die `takt size` `exakt` nennt und auf die
/// sich fuer `baremetal` ein Compile-Fehler stuetzt (11.5), darf nicht
/// vierfach danebenliegen.
fn dfa_bytes(p: &Program) -> u64 {
    fn one(pat: &Pattern) -> u64 {
        match pat {
            Pattern::Text { pieces, dfa: Some(d) } => {
                // Mit Platzhaltern laeuft der Durchlauf, nicht der
                // Automat (`step::handler_chain`).
                if pieces.iter().any(|x| matches!(x, crate::pattern::PatternPiece::Capture { .. })) {
                    return 0;
                }
                d.classes.len() as u64 + d.table.len() as u64 * 4
            }
            _ => 0,
        }
    }
    let mut total = 0;
    for m in &p.machines {
        for h in &m.handlers {
            if let Some((_, pat)) = &h.pattern {
                total += one(pat);
            }
        }
        for st in &m.states {
            for h in &st.handlers {
                if let Some((_, pat)) = &h.pattern {
                    total += one(pat);
                }
            }
            for t in &st.transitions {
                if let TransTrigger::When(Guard::Match { pattern, .. }) = &t.trigger {
                    total += one(pattern);
                }
            }
        }
    }
    total
}
