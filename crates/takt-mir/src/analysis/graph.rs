//! Der Abhaengigkeitsgraph eines Programms (5.4, 7.2, 8.6).
//!
//! Wer schreibt worauf, wer liest von wem, und in welcher Reihenfolge
//! laufen die Maschinen? Die Antworten stehen verstreut in der MIR — im
//! `owner` eines Channels, in den `readers` eines Stroms, in `follows`
//! und in den Cursorn einer Maschine. `takt graph` sammelt sie an einem
//! Ort.
//!
//! **Warum das ein Werkzeug wert ist.** Ein Programm mit zwanzig
//! Maschinen ist im Quelltext nicht mehr ueberschaubar: Welche Maschine
//! haengt an welchem Eingang, wer schreibt diesen Ausgang, und welche
//! Kette laeuft vor welcher? Die Fragen entscheiden, ob eine Aenderung
//! sicher ist — und sie sind mechanisch beantwortbar.
//!
//! Die Ausgabe ist Text, keine Zeichnung. Sie soll in einen Bericht
//! passen, sich mit `diff` vergleichen lassen und ohne Werkzeug lesbar
//! sein; wer ein Bild will, erzeugt es aus den Kanten.

use crate::expr::StreamRef;
use crate::machine::MachineKind;
use crate::program::{Binding, Direction};
use crate::{ChannelId, MachineId, Program, StreamId};

/// Eine Kante des Graphen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edge {
    /// Woher; `None` ist die Aussenwelt (Hardware, Simulation).
    pub from: Node,
    /// Wohin.
    pub to: Node,
    /// Was fliesst.
    pub kind: Kind,
}

/// Ein Knoten des Graphen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    /// Eine Maschine.
    Machine(MachineId),
    /// Ein Channel oder Strom.
    Channel(ChannelId),
    /// Ein interner Strom (8.6).
    Stream(StreamId),
}

/// Die Art einer Kante.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Die Maschine liest den Channel (Unit-Delay, 9.1).
    Reads,
    /// Die Maschine schreibt den Channel; sie ist sein Single-Writer (8.1).
    Writes,
    /// Die Maschine folgt einer anderen im selben Tick (7.2).
    Follows,
    /// Die Maschine liest eine veroeffentlichte Groesse (5.4).
    Published,
    /// Ein `sim`-Output speist einen `hw`-Input derselben Adresse (8.3).
    Simulates,
}

impl Kind {
    /// Das Wort fuer die Ausgabe.
    pub fn word(self) -> &'static str {
        match self {
            Kind::Reads => "liest",
            Kind::Writes => "schreibt",
            Kind::Follows => "folgt",
            Kind::Published => "liest pub",
            Kind::Simulates => "speist",
        }
    }
}

/// Der Graph eines Programms.
#[derive(Clone, Debug, Default)]
pub struct Graph {
    /// Die Kanten in fester Reihenfolge: je Maschine erst ihre Eingaenge,
    /// dann ihre Ausgaenge, dann ihre Abhaengigkeiten.
    pub edges: Vec<Edge>,
}

/// Baut den Graphen eines Programms.
pub fn graph(p: &Program) -> Graph {
    let mut edges = Vec::new();
    for (i, m) in p.machines.iter().enumerate() {
        // Eine Vorlage laeuft nie selbst (5.8); ihre Kanten gehoeren den
        // Instanzen, die aus ihr entstehen.
        if m.kind == MachineKind::Template {
            continue;
        }
        let id = MachineId(i as u32);
        for (j, c) in p.channels.iter().enumerate() {
            let cid = ChannelId(j as u32);
            if c.owner == Some(id) {
                edges.push(Edge { from: Node::Machine(id), to: Node::Channel(cid), kind: Kind::Writes });
            }
        }
        // Die Stroeme, auf die die Maschine einen Cursor haelt (8.6): Sie
        // liest sie, jeder mit eigenem Cursor.
        for s in &m.layout.cursors {
            // Ein Trigger (v1.2) und ein Handle in einer Variablen haben
            // keinen Knoten: Der eine kommt spaeter, der andere nennt
            // seinen Strom erst zur Laufzeit.
            let from_at = match s {
                StreamRef::Channel(c) => Node::Channel(*c),
                StreamRef::Internal(k) => Node::Stream(*k),
                StreamRef::Fired(_) | StreamRef::Var(_) => continue,
            };
            edges.push(Edge { from: from_at, to: Node::Machine(id), kind: Kind::Reads });
        }
        for &f in &m.follows {
            edges.push(Edge { from: Node::Machine(f), to: Node::Machine(id), kind: Kind::Follows });
        }
    }
    edges.extend(sim_edges(p));
    Graph { edges }
}

/// Die Kanten der Simulationsbindung (8.3).
///
/// Ein Plant-Modell schreibt `@ sim("daq1/ai0")`, das Programm liest
/// `@ hw("daq1/ai0")` — dieselbe Adresse, dieselbe Groesse. Im Quelltext
/// stehen die beiden Deklarationen oft weit auseinander, und wer die
/// Naht nicht kennt, sucht den Treiber, den es nicht gibt.
fn sim_edges(p: &Program) -> Vec<Edge> {
    let addr = |b: &Binding| match b {
        Binding::Hw(a) | Binding::Sim(a) => Some(a.clone()),
        Binding::None => None,
    };
    let mut out = Vec::new();
    for (i, o) in p.channels.iter().enumerate() {
        if o.dir != Direction::Output || !matches!(o.binding, Binding::Sim(_)) {
            continue;
        }
        let Some(a) = addr(&o.binding) else { continue };
        for (j, c) in p.channels.iter().enumerate() {
            if c.dir == Direction::Input && matches!(&c.binding, Binding::Hw(b) if *b == a) {
                out.push(Edge {
                    from: Node::Channel(ChannelId(i as u32)),
                    to: Node::Channel(ChannelId(j as u32)),
                    kind: Kind::Simulates,
                });
            }
        }
    }
    out
}

impl Graph {
    /// Der Graph als Text, je Maschine ein Absatz.
    ///
    /// Die Form folgt `takt size` und `takt latency`: eine Zeile je
    /// Aussage, der Name links, die Erlaeuterung rechts. Was fehlt, wird
    /// genannt statt weggelassen — eine Maschine ohne Eingaenge ist eine
    /// Aussage, keine Leerstelle.
    pub fn lines(&self, p: &Program) -> Vec<String> {
        let mut out = Vec::new();
        for (i, m) in p.machines.iter().enumerate() {
            if m.kind == MachineKind::Template {
                continue;
            }
            let id = MachineId(i as u32);
            let takt = if m.period == 1 { String::new() } else { format!("  (jeder {}. Tick)", m.period) };
            out.push(format!("  {}{takt}", m.name));
            let mut lines = Vec::new();
            for e in &self.edges {
                match (&e.from, &e.to) {
                    (Node::Machine(f), Node::Machine(t)) if *t == id => {
                        lines.push(format!("      {} {}", e.kind.word(), p.machines[f.index()].name));
                    }
                    (from_at, Node::Machine(t)) if *t == id => {
                        lines.push(format!("      {} {}", e.kind.word(), name_of(from_at, p)));
                    }
                    (Node::Machine(f), zu) if *f == id => {
                        lines.push(format!("      {} {}", e.kind.word(), name_of(zu, p)));
                    }
                    _ => {}
                }
            }
            if lines.is_empty() {
                out.push("      (ohne Kanten)".into());
            } else {
                out.extend(lines);
            }
        }
        let sim: Vec<String> = self
            .edges
            .iter()
            .filter(|e| e.kind == Kind::Simulates)
            .map(|e| format!("      {} -> {}", name_of(&e.from, p), name_of(&e.to, p)))
            .collect();
        if !sim.is_empty() {
            out.push("  Simulationsbindung (8.3)".into());
            out.extend(sim);
        }
        // Die Aussenwelt: Eingaenge ohne Quelle im Programm sind der Rand
        // (12.6), und ein Ausgang ohne Schreiber ist ein Befund.
        let ohne: Vec<&str> = p
            .channels
            .iter()
            .filter(|c| c.dir == Direction::Output && c.owner.is_none() && !matches!(c.binding, Binding::None))
            .map(|c| c.name.as_str())
            .collect();
        if !ohne.is_empty() {
            out.push("  ohne Schreiber".into());
            out.extend(ohne.iter().map(|n| format!("      {n}")));
        }
        out
    }
}

fn name_of(n: &Node, p: &Program) -> String {
    match n {
        Node::Machine(m) => p.machines[m.index()].name.clone(),
        Node::Channel(c) => p.channels[c.index()].name.clone(),
        Node::Stream(s) => p.streams.get(s.index()).map_or_else(|| format!("stream{}", s.0), |d| d.name.clone()),
    }
}
