//! Der Abhaengigkeitsgraph (`takt graph`, 11.1).
//!
//! Die Kanten kommen aus der MIR, nicht aus einer eigenen Analyse: Wer
//! einen Output besitzt, schreibt ihn (8.1); wer einen Cursor haelt,
//! liest den Strom (8.6); `follows` ist eine Kante (7.2). Die Tests
//! pruefen, dass keine davon verlorengeht — und dass eine Vorlage keine
//! bekommt, weil sie nie selbst laeuft.

use takt_mir::analysis::graph::{Kind, Node, graph};
use takt_mir::machine::MachineKind;
use takt_mir::sample::full_program;

#[test]
fn every_output_has_exactly_one_writer() {
    let p = full_program();
    let g = graph(&p);
    for (i, c) in p.channels.iter().enumerate() {
        let id = takt_mir::ChannelId(i as u32);
        let schreiber = g.edges.iter().filter(|e| e.kind == Kind::Writes && e.to == Node::Channel(id)).count();
        // 8.1: Genau ein Schreiber je Output — mehr waere ein Fehler des
        // Sema, keiner heisst, dass der Kanal vom Rand kommt.
        assert!(schreiber <= 1, "`{}` hat {schreiber} Schreiber", c.name);
        if c.owner.is_some() {
            assert_eq!(schreiber, 1, "`{}` hat einen Besitzer, aber keine Kante", c.name);
        }
    }
}

#[test]
fn a_template_has_no_edges() {
    let p = full_program();
    let g = graph(&p);
    for (i, m) in p.machines.iter().enumerate() {
        if m.kind != MachineKind::Template {
            continue;
        }
        let id = takt_mir::MachineId(i as u32);
        // 5.8: Eine Vorlage laeuft nie selbst — sie schreibt nichts und
        // liest nichts. Eingehende Kanten gibt es dennoch: Das
        // Beispielprogramm laesst eine Maschine der Vorlage `follows`,
        // und diese Kante gehoert der folgenden Maschine.
        let ausgehend = g.edges.iter().filter(|e| e.from == Node::Machine(id) && e.kind != Kind::Follows).count();
        assert_eq!(ausgehend, 0, "die Vorlage `{}` schreibt {ausgehend} Kanaele", m.name);
        let liest = g.edges.iter().filter(|e| e.kind == Kind::Reads && e.to == Node::Machine(id)).count();
        assert_eq!(liest, 0, "die Vorlage `{}` liest {liest} Stroeme", m.name);
    }
}

#[test]
fn every_cursor_becomes_a_read_edge() {
    let p = full_program();
    let g = graph(&p);
    for (i, m) in p.machines.iter().enumerate() {
        if m.kind == MachineKind::Template {
            continue;
        }
        let id = takt_mir::MachineId(i as u32);
        let gelesen = g.edges.iter().filter(|e| e.kind == Kind::Reads && e.to == Node::Machine(id)).count();
        // Ein Trigger und ein Handle in einer Variablen haben keinen
        // Knoten; die uebrigen Cursor sind je eine Kante (8.6).
        let mit_knoten = m
            .layout
            .cursors
            .iter()
            .filter(|s| matches!(s, takt_mir::expr::StreamRef::Channel(_) | takt_mir::expr::StreamRef::Internal(_)))
            .count();
        assert_eq!(gelesen, mit_knoten, "`{}`: {gelesen} Lesekanten bei {mit_knoten} Cursorn", m.name);
    }
}

#[test]
fn the_text_names_every_machine_that_runs() {
    let p = full_program();
    let zeilen = graph(&p).lines(&p).join("\n");
    for m in &p.machines {
        if m.kind == MachineKind::Template {
            continue;
        }
        assert!(zeilen.contains(&m.name), "`{}` fehlt in der Ausgabe:\n{zeilen}", m.name);
    }
}
