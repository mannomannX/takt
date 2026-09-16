//! Symbole und Sichtbereiche (plan/m1.md 3.3): jede Entitaet mit ihrer
//! Definitionsspanne; innere Bereiche duerfen sichtbare Namen nicht erneut
//! vergeben, nur Namen des Preludes (Warnung, 2.5).

use std::collections::HashMap;

use takt_diag::Span;
use takt_mir::expr::{Builtin, Expr};
use takt_mir::*;

/// Was ein Name bezeichnet.
#[derive(Clone, Debug)]
pub enum Entity {
    /// Gefaltete Konstante (Literal mit Typ).
    Const(Expr),
    /// Parameter mit Typ.
    Param(ParamId, TypeId),
    /// Channel.
    Channel(ChannelId),
    /// Command.
    Command(CommandId),
    /// Nicht generische Funktion.
    Fn(FnId),
    /// Generische Funktionsvorlage (Index in `Templates::fns`).
    FnTemplate(usize),
    /// Native Funktion.
    Native(NativeId),
    /// Nicht generischer Block.
    Block(BlockId),
    /// Generische Blockvorlage.
    BlockTemplate(usize),
    /// Maschine (Singleton oder Instanz).
    Machine(MachineId),
    /// Instanz-Array: erste Instanz und Laenge (5.8).
    MachineArray(MachineId, u32),
    /// Maschinenvorlage.
    MachineTemplate(usize),
    /// Interner Stream.
    Stream(StreamId),
    /// Typalias oder eingebauter Typ.
    Type(TypeId),
    /// Enum.
    Enum(EnumId),
    /// Record.
    Record(RecordId),
    /// Variable mit Typ (Maschine, Zustand, Funktion, gehoben).
    Var(VarId, TypeId),
    /// Signal der aktuellen Maschine.
    Signal(SignalId),
    /// Zustand der aktuellen Maschine.
    State(StateId),
    /// Knoten (v2).
    Node(NodeId),
    /// Trigger (v1.2).
    Trigger(TriggerId),
    /// Profil.
    Profile(ProfileId),
    /// Eingebaute Groesse (`now`, `tick`, `time_in_state`, `last_fault`, `event`).
    Builtin(Builtin),
    /// Primitive (`abs`, `sqrt`, `fma`, …).
    Intrinsic(takt_mir::expr::Intrinsic),
}

/// Ein Symbol mit Herkunft.
#[derive(Clone, Debug)]
pub struct Symbol {
    /// Bedeutung.
    pub entity: Entity,
    /// Definitionsstelle.
    pub span: Span,
    /// Aus dem Prelude (darf verdeckt werden).
    pub prelude: bool,
}

/// Verdeckung beim Deklarieren: was verdeckt wurde und wo es steht.
#[derive(Clone, Copy, Debug)]
pub struct Shadowed {
    /// Definitionsstelle des verdeckten Namens.
    pub span: Span,
    /// Der verdeckte Name kam aus dem Prelude.
    pub prelude: bool,
}

/// Sichtbereiche, innen nach aussen.
#[derive(Debug, Default)]
pub struct Scopes {
    frames: Vec<HashMap<String, Symbol>>,
}

impl Scopes {
    /// Neuer Bereich.
    pub fn push(&mut self) {
        self.frames.push(HashMap::new());
    }

    /// Bereich schliessen.
    pub fn pop(&mut self) {
        self.frames.pop();
    }

    /// Tiefe.
    pub fn depth(&self) -> usize {
        self.frames.len()
    }

    /// Deklariert im innersten Bereich; ein sichtbarer Name ist ein Fehler
    /// (`Err`), ein Prelude-Name wird verdeckt (`Ok(Some)`).
    pub fn declare(&mut self, name: &str, symbol: Symbol) -> Result<Option<Shadowed>, Shadowed> {
        let existing = self.lookup(name).map(|e| Shadowed { span: e.span, prelude: e.prelude });
        let frame = self.frames.last_mut().expect("Sichtbereich");
        match existing {
            Some(e) if !e.prelude || symbol.prelude => Err(e),
            Some(e) => {
                frame.insert(name.to_string(), symbol);
                Ok(Some(e))
            }
            None => {
                frame.insert(name.to_string(), symbol);
                Ok(None)
            }
        }
    }

    /// Ersetzt ein Symbol im Bereich, in dem es steht (Nachtragen von Ids).
    pub fn replace(&mut self, name: &str, entity: Entity) {
        for frame in self.frames.iter_mut().rev() {
            if let Some(s) = frame.get_mut(name) {
                s.entity = entity;
                return;
            }
        }
    }

    /// Loest alle Bereiche ueber den `keep` aeusseren ab: eine Vorlage
    /// sieht nur die Dateiebene, eine aus dem Prelude nur das Prelude
    /// (plan/m1.md 1.2).
    pub fn detach_inner(&mut self, keep: usize) -> Vec<HashMap<String, Symbol>> {
        self.frames.split_off(keep.min(self.frames.len()))
    }

    /// Haengt abgeloeste Bereiche wieder an.
    pub fn attach_inner(&mut self, keep: usize, inner: Vec<HashMap<String, Symbol>>) {
        self.frames.truncate(keep.min(self.frames.len()));
        self.frames.extend(inner);
    }

    /// Innerster Treffer.
    pub fn lookup(&self, name: &str) -> Option<&Symbol> {
        self.frames.iter().rev().find_map(|f| f.get(name))
    }

    /// Alle sichtbaren Namen (fuer Vorschlaege).
    pub fn visible(&self) -> Vec<&str> {
        let mut out: Vec<&str> = self.frames.iter().flat_map(|f| f.keys().map(String::as_str)).collect();
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// Levenshtein-Abstand fuer Vorschlaege („meinst du …").
pub fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur.push((prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Naechstliegender sichtbarer Name mit Abstand hoechstens 2.
pub fn suggestion<'a>(name: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    candidates.into_iter().filter(|c| distance(name, c) <= 2 && !c.is_empty()).min_by_key(|c| distance(name, c))
}
