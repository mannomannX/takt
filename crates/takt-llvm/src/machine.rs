//! Maschinen nach LLVM-IR: Zustands-Struct und Tickschritt (11.2).
//!
//! 11.2 gibt die Form woertlich vor:
//!
//! ```text
//! struct hotfire_state { conf: [u8; DEPTH], t_in_state: [u64; DEPTH], vars…,
//!                        state_vars…, blocks…, every_next…, cur: [u64; STREAMS],
//!                        pending: Option<Fault>, last_fault: Fault, pc: u32 }
//! fn hotfire_step(s: &mut hotfire_state, i: &Inputs, psi: &Published, o: &mut OutLatch)
//! ```
//!
//! **Die Konfiguration ist ein Pfad-Array fester Tiefe**, kein Zeiger und
//! keine Rekursion: `conf[d]` ist der aktive Zustand auf Ebene `d`. Damit
//! ist der Zustandsraum statisch bekannt — die Bedingung dafuer, dass
//! `takt size` (11.5) rechnen kann und dass kein Heap noetig ist (13.4,
//! Power of Ten).
//!
//! **Der Fault-Trampolin ist ein eigener Block je Maschine.** Jeder
//! `Checked`-Knoten springt dorthin (4.1); 11.2 verlangt, dass die
//! Fault-Pfade `cold` sind und keine Spekulation ueber sie hinweg
//! stattfindet.

use core::fmt::Write as _;

use takt_mir::machine::{Machine, State};
use takt_mir::program::Program;
use takt_mir::{MachineId, StateId};

use crate::emit::Module;
use crate::ty::{self, LlvmType};

/// Der Bauplan des Zustands-Structs einer Maschine (11.2).
///
/// Er entsteht einmal und ist danach die einzige Quelle fuer die Indizes
/// im erzeugten Code — wie der `wire_size`-Plan eines Records im Sema.
/// Zwei Stellen, die die Reihenfolge unabhaengig voneinander ausrechnen,
/// waeren zwei Gelegenheiten, sie verschieden auszurechnen.
#[derive(Clone, Debug, PartialEq)]
pub struct StateStruct {
    /// Die Felder in ihrer Reihenfolge im Struct.
    pub fields: Vec<Field>,
    /// Tiefe des Zustandsbaums; die Laenge von `conf` und `t_in_state`.
    pub depth: u32,
}

/// Ein Feld des Zustands-Structs.
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    /// Name, wie er im Kommentar der IR steht.
    pub name: String,
    /// Sein LLVM-Typ.
    pub ty: LlvmType,
    /// Wofuer es steht.
    pub role: Role,
}

/// Wofuer ein Feld steht (11.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum Role {
    /// `conf[d]`: aktiver Zustand je Ebene.
    Conf,
    /// `t_in_state[d]`: Zeit in diesem Zustand, in Ticks.
    TimeInState,
    /// Eine Variable der Maschine oder eines Zustands.
    Var,
    /// `every_next[c]`: naechster Tick, an dem `every` feuert.
    EveryNext,
    /// `viol[site]`: Bestaetigungszaehler eines `check … for d` (5.6).
    Viol,
    /// `cur[s]`: Cursor eines gelesenen Stroms (9.6).
    Cursor,
    /// `examined[s] + 1`: hinter dem hoechsten untersuchten Element (9.6).
    Examined,
    /// `pending`: vorgemerkter Fault (5.4).
    Pending,
    /// `last_fault`.
    LastFault,
    /// `pc`: die Zeile der letzten Anweisung, fuer die Instrumentierung.
    Pc,
    /// `saved[i]`: zuletzt aktives Blatt eines `resume`-Zustands (5.12);
    /// -1 vor dem ersten Austritt.
    Saved,
}

/// Die Tiefe des Zustandsbaums einer Maschine.
///
/// Mindestens 1: Auch eine Maschine mit einer einzigen Ebene hat eine
/// Konfiguration.
pub fn depth(m: &Machine) -> u32 {
    fn from(m: &Machine, id: StateId, seen: u32) -> u32 {
        let s = &m.states[id.index()];
        s.children.iter().map(|c| from(m, *c, seen + 1)).max().unwrap_or(seen)
    }
    m.roots.iter().map(|r| from(m, *r, 1)).max().unwrap_or(1)
}

/// Baut den Zustands-Struct einer Maschine (11.2).
///
/// `None` heisst: Eine Variable hat einen Typ, den der Codegen noch nicht
/// abbildet. Die Auskunft ist ehrlicher als ein Struct, in dem ein Feld
/// fehlt — der Versatz aller folgenden waere falsch, und niemand saehe es.
pub fn state_struct(m: &Machine, p: &Program) -> Option<StateStruct> {
    let d = depth(m);
    let mut fields = Vec::new();
    // 11.2: `conf: [u8; DEPTH]`. Ein `u8` je Ebene reicht, solange eine
    // Ebene nicht mehr als 256 Geschwister hat; darueber waere die
    // Maschine ohnehin nicht mehr lesbar (Prinzip: Struktur sichtbar).
    fields.push(Field { name: "conf".into(), ty: LlvmType::Array(Box::new(LlvmType::Int(8)), d), role: Role::Conf });
    fields.push(Field {
        name: "t_in_state".into(),
        ty: LlvmType::Array(Box::new(LlvmType::Int(64)), d),
        role: Role::TimeInState,
    });
    for (i, v) in m.vars.iter().enumerate() {
        // Eine Blockinstanz traegt ihren Zustand im Struct der Maschine
        // (5.7); ihr Typ steht nicht im Typsystem, sondern in
        // `Layout::block_instances`.
        let ty = match instance_block(m, takt_mir::VarId(i as u32)) {
            Some(b) => crate::block::instance_of(p.blocks.get(b.index())?, p)?.llvm(),
            None => ty::lower(v.ty, p)?,
        };
        fields.push(Field { name: format!("var{i}_{}", v.name), ty, role: Role::Var });
    }
    for (i, _) in m.layout.every_counters.iter().enumerate() {
        fields.push(Field { name: format!("every_next{i}"), ty: LlvmType::Int(64), role: Role::EveryNext });
    }
    for (i, _) in m.layout.viol_sites.iter().enumerate() {
        fields.push(Field { name: format!("viol{i}"), ty: LlvmType::Int(32), role: Role::Viol });
    }
    for (i, _) in m.layout.cursors.iter().enumerate() {
        fields.push(Field { name: format!("cur{i}"), ty: LlvmType::Int(64), role: Role::Cursor });
    }
    for (i, _) in m.layout.cursors.iter().enumerate() {
        fields.push(Field { name: format!("examined{i}"), ty: LlvmType::Int(64), role: Role::Examined });
    }
    // `pending` ist ein Fault mit Gueltigkeitsflag; der Fault selbst ist
    // seine Art und sein Ursprung (5.3). Als Struct, damit 5.4 ihn im
    // selben Tick weiterreichen kann.
    fields.push(Field {
        name: "pending".into(),
        ty: LlvmType::Struct(vec![LlvmType::Int(1), LlvmType::Int(32), LlvmType::Int(32)]),
        role: Role::Pending,
    });
    fields.push(Field {
        name: "last_fault".into(),
        ty: LlvmType::Struct(vec![LlvmType::Int(1), LlvmType::Int(32), LlvmType::Int(32)]),
        role: Role::LastFault,
    });
    fields.push(Field { name: "pc".into(), ty: LlvmType::Int(32), role: Role::Pc });
    for (i, _) in m.layout.saved_paths.iter().enumerate() {
        fields.push(Field { name: format!("saved{i}"), ty: LlvmType::Int(32), role: Role::Saved });
    }
    Some(StateStruct { fields, depth: d })
}

impl StateStruct {
    /// Der Struct als LLVM-Typ.
    pub fn llvm(&self) -> LlvmType {
        LlvmType::Struct(self.fields.iter().map(|f| f.ty.clone()).collect())
    }

    /// Der Index eines Felds, fuer `getelementptr`.
    pub fn index_of(&self, role: Role, nth: usize) -> Option<u32> {
        self.fields
            .iter()
            .enumerate()
            .filter(|(_, f)| f.role == role)
            .nth(nth)
            .map(|(i, _)| u32::try_from(i).unwrap_or(u32::MAX))
    }

    /// Der Byteversatz eines Felds, wie das Datenlayout es legt — fuer
    /// Rahmen, die den Zustand als Bytepuffer halten (12.1).
    pub fn byte_offset(&self, role: Role, nth: usize) -> Option<u64> {
        let index = self.index_of(role, nth)? as usize;
        let mut at = 0u64;
        for f in &self.fields[..index] {
            at = at.div_ceil(f.ty.align()) * f.ty.align() + f.ty.aligned_size();
        }
        let align = self.fields[index].ty.align();
        Some(at.div_ceil(align) * align)
    }

    /// Die Groesse in Bytes, ohne Ausrichtung (11.5 rechnet genauer).
    pub fn size(&self) -> u64 {
        self.fields.iter().map(|f| f.ty.size()).sum()
    }

    /// Die Groesse mit Ausrichtung: so viel belegt der Struct wirklich,
    /// und so viel muss ein Rahmen reservieren (FB-177).
    pub fn aligned_size(&self) -> u64 {
        self.llvm().aligned_size()
    }
}

/// Schreibt die Typdefinition des Zustands-Structs als benannten Typ.
///
/// Ein benannter Typ (`%maschine_state = type { … }`) statt eines
/// anonymen: Die IR bleibt lesbar, und das ist nach 13.4 kein
/// Schoenheitsargument — sie ist das Artefakt der Qualifikation.
pub fn declare_state(m: &Machine, st: &StateStruct, module: &mut Module) {
    let mut text = String::new();
    let _ = write!(text, "\n; Zustand der Maschine `{}` (11.2)", m.name);
    for f in &st.fields {
        let _ = write!(text, "\n;   {} : {}", f.name, f.ty);
    }
    let _ = write!(text, "\n%{}_state = type {}", m.name, st.llvm());
    module.declare(&text);
}

/// Der Name der Schrittfunktion einer Maschine (11.2: `hotfire_step`).
pub fn step_name(m: &Machine) -> String {
    format!("{}_step", crate::fns::sanitized(&m.name))
}

/// Beginnt die Schrittfunktion einer Maschine.
///
/// Die Signatur folgt 11.2: Zustand, Inputs, Ψ und der Output-Latch, alle
/// als Zeiger. Der Latch wird von der Runtime committet (12.1), nicht von
/// der Maschine — darum ist er ein Ausgabeparameter und kein Rueckgabewert.
pub fn begin_step(m: &Machine, module: &mut Module) -> Vec<crate::emit::Reg> {
    let ptr = LlvmType::Ptr;
    module.begin(&step_name(m), &LlvmType::Void, &[ptr.clone(), ptr.clone(), ptr.clone(), ptr])
}

/// Schreibt den Fault-Trampolin einer Maschine (5.3, 11.2).
///
/// Jeder `Checked`-Knoten springt hierher (4.1). Der Trampolin *merkt den
/// Fault vor*, er fuehrt ihn nicht aus: 5.4 laesst ihn in der Abort-Phase
/// wirken, damit die Reihenfolge der Maschinen keine Rolle spielt
/// (Satz 9.4.1). Danach verlaesst er die Schrittfunktion — der Rest des
/// Ticks dieser Maschine faellt aus, wie 5.3 es verlangt.
///
/// 11.2 verlangt ausserdem, dass die Fault-Pfade `cold` sind. Das steht
/// als Attribut an der Funktion, nicht am Block; hier sorgt die
/// Anordnung ans Ende dafuer, dass der heisse Pfad zusammenhaengt.
pub fn fault_trampoline(m: &Machine, st: &StateStruct, module: &mut Module) {
    module.label(&format!("fault_{}", m.name));
    let Some(pending) = st.index_of(Role::Pending, 0) else {
        // Ohne `pending`-Feld gibt es nichts vorzumerken; das kann nur
        // passieren, wenn der Struct nicht gebaut werden konnte.
        module.void_inst("ret void");
        return;
    };
    let state_ty = format!("%{}_state", crate::fns::sanitized(&m.name));
    // `pending` ist `{ i1 gueltig, i32 art, i32 ursprung }` (5.3). Die
    // beiden Zahlen kommen von der Sprungstelle; hier wird das Flag
    // gesetzt, damit die Abort-Phase den Fault findet.
    let field = module.inst(&format!("getelementptr inbounds {state_ty}, ptr %0, i32 0, i32 {pending}"));
    let flag = module.inst(&format!("getelementptr inbounds {{ i1, i32, i32 }}, ptr {field}, i32 0, i32 0"));
    module.void_inst(&format!("store i1 true, ptr {flag}"));
    // 5.3: Der Schritt endet hier. Die Abort-Phase (5.4) uebernimmt.
    module.void_inst("ret void");
}

/// Der Name eines Zustands, wie er als Marke in der IR erscheint.
pub fn label_of(m: &Machine, id: StateId) -> String {
    format!("{}_{}", crate::fns::sanitized(&m.name), crate::fns::sanitized(&m.states[id.index()].name))
}

/// Die Blattzustaende einer Maschine in der Reihenfolge ihrer Ids.
///
/// 11.2: „Blattzustand = Enum-Diskriminante; `switch` ueber die Blaetter."
/// Nur Blaetter, weil ein zusammengesetzter Zustand nie allein aktiv ist —
/// aktiv ist immer ein vollstaendiger Pfad bis zu einem Blatt.
pub fn leaves(m: &Machine) -> Vec<StateId> {
    m.states.iter().enumerate().filter(|(_, s)| s.children.is_empty()).map(|(i, _)| StateId(i as u32)).collect()
}

/// Der Pfad von der Wurzel zu einem Zustand (11.2: `conf`).
pub fn path_to(m: &Machine, id: StateId) -> Vec<StateId> {
    let mut out = Vec::new();
    let mut cur = Some(id);
    while let Some(s) = cur {
        out.push(s);
        cur = m.states[s.index()].parent;
    }
    out.reverse();
    out
}

/// Der Zustand zu einer Id.
pub fn state_of(m: &Machine, id: StateId) -> &State {
    &m.states[id.index()]
}

/// Die Maschine zu einer Id.
pub fn machine_of(p: &Program, id: MachineId) -> &Machine {
    &p.machines[id.index()]
}

/// Das Blatt, das beim Betreten eines Zustands aktiv wird (5.2).
///
/// Ein zusammengesetzter Zustand ist nie allein aktiv: Sein `initial`-Kind
/// wird mitbetreten, und dessen `initial`-Kind, bis ein Blatt erreicht
/// ist. `None` heisst, dass ein zusammengesetzter Zustand kein `initial`
/// hat — das faengt Pruefung 10 ab, aber der Codegen verlaesst sich nicht
/// darauf.
pub fn initial_leaf(m: &Machine, from: StateId) -> Option<StateId> {
    let mut cur = from;
    // Eine Vorlage hat keine Zustaende (5.8); `from` zeigt dann ins
    // Leere, und ein Index waere ein Absturz statt einer Meldung.
    if m.states.is_empty() {
        return None;
    }
    // Die Schranke ist die Zahl der Zustaende: Ein Zyklus im `initial`-Pfad
    // waere ein Fehler im Sema, aber eine Endlosschleife im Codegen waere
    // schlimmer als eine Meldung (4.1: beschraenkte Schleifen).
    for _ in 0..=m.states.len() {
        let s = &m.states[cur.index()];
        if s.children.is_empty() {
            return Some(cur);
        }
        cur = s.initial?;
    }
    None
}

/// Die Zustaende, deren `exit:` bei einem Uebergang laeuft (5.2).
///
/// Vom verlassenen Blatt aufwaerts bis unter den gemeinsamen Vorfahren mit
/// dem Ziel. Der Vorfahre selbst bleibt aktiv und wird nicht verlassen —
/// ein Uebergang zwischen zwei Geschwistern raeumt nicht ihren Elternteil
/// ab.
pub fn exiting(m: &Machine, from: StateId, to: StateId) -> Vec<StateId> {
    let common = common_ancestor(m, from, to);
    let mut out = Vec::new();
    let mut cur = Some(from);
    while let Some(id) = cur {
        if Some(id) == common {
            break;
        }
        out.push(id);
        cur = m.states[id.index()].parent;
    }
    out
}

/// Die Zustaende, deren `enter:` bei einem Uebergang laeuft (5.2).
///
/// Von unter dem gemeinsamen Vorfahren abwaerts bis zum neuen Blatt, in
/// dieser Richtung: Ein `enter:` weiter oben stellt her, worauf das
/// darunter sich verlaesst.
pub fn entering(m: &Machine, from: StateId, to: StateId) -> Vec<StateId> {
    let common = common_ancestor(m, from, to);
    let mut out: Vec<StateId> = Vec::new();
    let mut cur = Some(to);
    while let Some(id) = cur {
        if Some(id) == common {
            break;
        }
        out.push(id);
        cur = m.states[id.index()].parent;
    }
    out.reverse();
    out
}

/// Der naechste gemeinsame Vorfahre zweier Zustaende; `None`, wenn sie in
/// verschiedenen Baeumen der obersten Ebene stehen.
fn common_ancestor(m: &Machine, a: StateId, b: StateId) -> Option<StateId> {
    let pa = path_to(m, a);
    let pb = path_to(m, b);
    let mut common = None;
    for (x, y) in pa.iter().zip(&pb) {
        if x != y {
            break;
        }
        common = Some(*x);
    }
    // Ein Zustand ist nicht sein eigener Vorfahre: Ein Uebergang auf sich
    // selbst verlaesst und betritt ihn (5.2, Selbstuebergang).
    if common == Some(a) && a == b { None } else { common }
}

/// Der Block, dessen Instanz in dieser Variablen steht (5.7).
///
/// Die Identitaet steht in `Layout::block_instances`, nicht im Typ: Das
/// Sema gibt jeder Instanz denselben Platzhaltertyp (FB-77).
pub fn instance_block(m: &Machine, var: takt_mir::VarId) -> Option<takt_mir::BlockId> {
    m.layout.block_instances.iter().find(|b| b.var == var).map(|b| b.block)
}
