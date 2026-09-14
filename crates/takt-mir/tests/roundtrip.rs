//! Format `TAKT-MIR`: schreiben, lesen, gleich — fuer jede Knotenart; Kopf;
//! Vorwaertskompatibilitaet (unbekannte Felder, fehlende Listen); Hashes.

use takt_mir::format::codec::Field;
use takt_mir::format::wire::{Node, Reader, Writer};
use takt_mir::format::{FORMAT_VERSION, decode_body, encode_body, read_header, read_program, write_program};
use takt_mir::hash::{logic_hash, program_hash};
use takt_mir::program::{Binding, Meta};
use takt_mir::sample::full_program;
use takt_mir::stmt::Block;
use takt_mir::types::TypeTable;
use takt_mir::*;

#[test]
fn full_program_round_trips() {
    let p = full_program();
    let bytes = write_program(&p, "takt 0.1.0");
    let (header, back) = read_program(&bytes).expect("lesbar");
    assert_eq!(header.format_version, FORMAT_VERSION);
    assert_eq!(header.edition, 1);
    assert_eq!(header.compiler_version, "takt 0.1.0");
    assert_eq!(back, p);
    let again = write_program(&back, "takt 0.1.0");
    assert_eq!(again, bytes, "Schreiben ist deterministisch");
}

#[test]
fn sample_covers_every_node_kind() {
    let p = full_program();
    let text = format!("{p:#?}");
    let expected = [
        "Bool",
        "Int",
        "Float",
        "Duration",
        "Enum",
        "Record",
        "Array",
        "Bytes",
        "Vec",
        "Str",
        "Line",
        "Samples",
        "Table",
        "Mat",
        "Map",
        "Optional",
        "Result",
        "Stream",
        "Capture",
        "Handle",
        "Variant",
        "BlockInit",
        "Published",
        "StateOf",
        "Signal",
        "Builtin",
        "Field",
        "Index",
        "Index2",
        "Slice",
        "Accessor",
        "Unary",
        "Binary",
        "Cond",
        "Cast",
        "Convert",
        "Matches",
        "Call",
        "NativeCall",
        "MatOp",
        "Decode",
        "Checked",
        "Assign",
        "Check",
        "Goto",
        "Abort",
        "If",
        "ForRange",
        "ForEach",
        "Match",
        "Return",
        "Send",
        "At",
        "Cancel",
        "Raise",
        "Job",
        "Every",
        "Break",
        "Observe",
        "Arm",
        "MethodCall",
        "Pass",
        "Alert",
        "Log",
        "Measure",
        "Verify",
        "Verdict",
        "Wait",
        "Until",
        "Expect",
        "Repeat",
        "Step",
        "Temporal",
        "Implies",
        "Hw",
        "Sim",
        "Template",
        "Instance",
        "Scenario",
        "Dimensioned",
        "Uniform",
        "LengthPrefixed",
        "Fixed",
        "DropOldest",
        "Drop",
        "sweeps: [",
        "Trigger",
        "Property",
        "Campaign",
        "Node",
        "Profile",
        "Command",
        "PersistVar",
        "ScopedInstance",
        "Dfa",
        "Fired",
        "Internal",
        "Wrap",
        "Runtime",
        "Arithmetic",
        "Lift(",
        "Ok(",
        "Err(",
        "Intrinsic {",
        "Sqrt",
        "Round",
        "Fma",
        "WrappingAdd",
    ];
    for name in expected {
        assert!(text.contains(name), "Knoten {name} fehlt im Beispielprogramm");
    }
}

#[test]
fn header_is_checked() {
    let p = full_program();
    let mut bytes = write_program(&p, "x");
    assert!(read_header(&bytes).is_ok());
    bytes[9] = 9;
    assert!(matches!(read_program(&bytes), Err(FormatError::UnsupportedVersion(_))));
    bytes[0] = b'X';
    assert!(matches!(read_program(&bytes), Err(FormatError::BadMagic)));
    let short = &write_program(&p, "x")[..40];
    assert!(matches!(read_program(short), Err(FormatError::Truncated)));
}

/// 11.3: „Leser akzeptieren aeltere Versionen ihres Formats, Schreiber
/// schreiben die neueste."
///
/// Die Ablehnung einer *neueren* Version steht in `header_is_checked`;
/// hier steht die andere Haelfte. Sie ist die wichtigere: Eine
/// Aufzeichnung von gestern muss sich heute lesen lassen, sonst ist die
/// Versionierung nur Zierde.
///
/// Geprueft wird an einer Datei, deren Kopf eine aeltere Version nennt.
/// Der Rumpf ist der heutige — das ist der Fall, den die Regel deckt:
/// Der Leser darf an der Version nicht scheitern, und die Felder, die er
/// nicht kennt, ueberspringt er ohnehin (der Test darunter).
#[test]
fn a_reader_accepts_an_older_format_version() {
    let p = full_program();
    let mut bytes = write_program(&p, "takt 0.1.0");
    for older in 1..FORMAT_VERSION {
        bytes[8..10].copy_from_slice(&older.to_le_bytes());
        let (header, back) = read_program(&bytes).expect("eine aeltere Version ist lesbar");
        assert_eq!(header.format_version, older);
        assert_eq!(back, p, "Version {older}: der Rumpf kommt unveraendert zurueck");
    }
}

/// Ein aelterer Leser ueberspringt Felder, die er nicht kennt; ein neuerer
/// Leser liest Listenfelder, die eine alte Datei nicht hat, als leer.
#[test]
fn unknown_fields_are_skipped_and_missing_lists_are_empty() {
    let r = Reader::new(vec!["x".to_string()]);
    let mut w = Writer::new(false);
    w.begin();
    w.varint(99, 7);
    w.bytes(98, b"zukunft");
    w.fixed64(97, 1);
    w.end(1);
    let (_, bytes) = w.finish();
    let root = Node::parse("Wurzel", &bytes).expect("Knoten");
    let block = Block::read(root.one(1).expect("Feld 1"), &r).expect("Block ohne Felder");
    assert!(block.stmts.is_empty());
    let table = TypeTable::read(root.one(1).expect("Feld 1"), &r).expect("Tabelle ohne Felder");
    assert!(table.list.is_empty());
}

#[test]
fn logic_hash_ignores_bindings_metadata_and_positions() {
    let p = full_program();
    let mut q = p.clone();
    for c in &mut q.channels {
        c.binding = Binding::None;
        c.meta = Meta::default();
        c.span = takt_diag::Span::new(1000, 1001);
    }
    q.machines[0].meta = Meta::default();
    q.machines[0].states[0].span = takt_diag::Span::new(5, 6);
    q.config.tick_source = None;
    q.config.target = None;
    assert_eq!(logic_hash(&p), logic_hash(&q));
    assert_ne!(program_hash(&p), program_hash(&q));

    let mut r = p.clone();
    r.config.float_width = takt_mir::types::FloatWidth::F64;
    assert_ne!(logic_hash(&p), logic_hash(&r), "float-Breite ist Teil des Logik-Hashs");
    let mut s = p.clone();
    s.config.edition = 2;
    assert_ne!(logic_hash(&p), logic_hash(&s), "Edition ist Teil des Logik-Hashs");
    let mut t = p.clone();
    t.machines[0].period = 11;
    assert_ne!(logic_hash(&p), logic_hash(&t));
    assert_eq!(logic_hash(&p).to_string().len(), 64);
}

/// Das Beispielprogramm laesst sich entzuckern; auch die Kern-MIR ist
/// roundtrip-fest.
#[test]
fn sample_desugars_and_round_trips_as_core() {
    let mut p = full_program();
    desugar(&mut p).expect("Desugaring");
    assert!(p.is_core());
    let bytes = write_program(&p, "x");
    let (_, back) = read_program(&bytes).expect("lesbar");
    assert_eq!(back, p);
    let text = takt_mir::dump::dump_machine(&p, MachineId(0));
    assert!(text.contains("state RUN.S0:"), "{text}");
    assert!(text.contains("after 150 ms: -> RUN.S1"), "{text}");
    assert!(text.contains("after 1 s: verdict fail \"no completion\"; -> SAFE"), "{text}");
}

/// Die reinen Zugriffe und mutierenden Methoden tragen genau die reservierten
/// Membernamen aus Referenz 2.5.
#[test]
fn accessor_names_are_reserved_members() {
    use takt_mir::expr::Accessor;
    use takt_mir::stmt::Method;
    use takt_mir::types::IntWidth;
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../plan/definition.md");
    let text = std::fs::read_to_string(path).expect("plan/definition.md lesbar");
    let start = text.find("Die eingebauten Zugriffe — `").expect("Liste") + "Die eingebauten Zugriffe — `".len();
    let end = text[start..].find('`').expect("Listenende") + start;
    let reserved: Vec<&str> = text[start..end].split_whitespace().collect();
    for a in Accessor::ALL {
        assert!(reserved.contains(&a.name().as_str()), "{} steht nicht in 2.5", a.name());
    }
    assert_eq!(Accessor::Wrap(IntWidth::U16).name(), "wrap_u16");
    assert!(reserved.contains(&"wrap_*"));
    for m in [Method::Step, Method::Reset, Method::Push, Method::Insert, Method::Remove, Method::Clear] {
        let name = m.name().expect("Name");
        assert!(name == "step" || reserved.contains(&name), "{name} steht nicht in 2.5");
    }
    let known: std::collections::HashSet<String> = Accessor::ALL
        .iter()
        .map(|a| a.name())
        .chain(
            [
                "wrap_*",
                "state",
                "to",
                "to_float",
                "as",
                "transpose",
                "inv",
                "det",
                "solve",
                "cholesky",
                "decode",
                "default",
                "push",
                "append",
                "insert",
                "remove",
                "clear",
                "skip",
                "fired",
                "reset",
            ]
            .into_iter()
            .map(String::from),
        )
        .collect();
    for name in &reserved {
        assert!(known.contains(*name), "{name} aus 2.5 hat keinen MIR-Knoten");
    }
}

/// Die Logikform laesst sich lesen; alles, was ihr fehlt, sind `meta`-Felder,
/// und der Logik-Hash des Gelesenen ist derselbe.
#[test]
fn logic_form_decodes_without_metadata() {
    let p = full_program();
    let (strings, body) = encode_body(&p, true);
    let q = decode_body(strings, &body).expect("Logikform lesbar");
    assert_eq!(logic_hash(&q), logic_hash(&p));
    assert_ne!(program_hash(&q), program_hash(&p));
    assert!(q.channels.iter().all(|c| c.binding == Binding::None && c.meta == Meta::default()));
    assert!(q.config.tick_source.is_none() && q.config.target.is_none());
    assert_eq!(q.machines[0].states[0].span, takt_diag::Span::default());
    assert_eq!(q.machines[0].states[0].enter.stmts[0].span, takt_diag::Span::default());
    assert_eq!(q.machines[0].vars[0].span, takt_diag::Span::default());
    assert_eq!(q.fns[0].body.stmts[1].span, takt_diag::Span::default());
    let (again, body_again) = encode_body(&q, true);
    assert_eq!(body_again, body);
    assert_eq!(again.len(), decode_body(again.clone(), &body_again).map(|_| again.len()).expect("lesbar"));
}

/// Positionen an jeder Knotenart sind `meta`: sie aendern den Logik-Hash nicht.
#[test]
fn every_span_is_outside_the_logic_hash() {
    use takt_diag::Span;
    let p = full_program();
    let moved = Span::new(4242, 4243);
    type Case = Box<dyn Fn(&mut Program)>;
    let cases: Vec<Case> = vec![
        Box::new(move |q| q.units[1].span = moved),
        Box::new(move |q| q.enums[0].span = moved),
        Box::new(move |q| q.enums[0].variants[0].span = moved),
        Box::new(move |q| q.enums[2].variants[0].fields[0].span = moved),
        Box::new(move |q| q.records[0].span = moved),
        Box::new(move |q| q.records[0].fields[1].bits[0].span = moved),
        Box::new(move |q| q.fns[0].span = moved),
        Box::new(move |q| q.fns[0].params[0].span = moved),
        Box::new(move |q| q.fns[0].locals[2].span = moved),
        Box::new(move |q| q.fns[0].body.span = moved),
        Box::new(move |q| q.fns[0].body.stmts[0].span = moved),
        Box::new(move |q| q.natives[0].span = moved),
        Box::new(move |q| q.blocks[0].span = moved),
        Box::new(move |q| q.channels[0].span = moved),
        Box::new(move |q| q.streams[0].span = moved),
        Box::new(move |q| q.params[0].span = moved),
        Box::new(move |q| q.profiles[0].span = moved),
        Box::new(move |q| q.commands[0].span = moved),
        Box::new(move |q| q.nodes[0].span = moved),
        Box::new(move |q| q.properties[0].span = moved),
        Box::new(move |q| q.campaigns[0].span = moved),
        Box::new(move |q| q.triggers[0].span = moved),
        Box::new(move |q| q.triggers[0].time.span = moved),
        Box::new(move |q| q.machines[0].span = moved),
        Box::new(move |q| q.machines[0].params[0].span = moved),
        Box::new(move |q| q.machines[0].vars[0].span = moved),
        Box::new(move |q| q.machines[0].signals[0].span = moved),
        Box::new(move |q| q.machines[0].faulted.span = moved),
        Box::new(move |q| q.machines[0].faulted.transitions[0].span = moved),
        Box::new(move |q| q.machines[0].handlers[0].span = moved),
        Box::new(move |q| q.machines[0].layout.every_counters[0].span = moved),
        Box::new(move |q| q.machines[0].layout.viol_sites[0].span = moved),
        Box::new(move |q| q.machines[0].states[0].span = moved),
        Box::new(move |q| q.machines[0].states[0].enter.span = moved),
        Box::new(move |q| q.machines[0].states[0].enter.stmts[0].span = moved),
        Box::new(move |q| q.machines[0].states[0].transitions[0].span = moved),
        Box::new(move |q| q.machines[0].states[0].transitions[0].actions.span = moved),
        Box::new(move |q| q.machines[0].states[0].handlers[0].span = moved),
        Box::new(move |q| {
            if let Some(seq) = &mut q.machines[0].states[0].sequence {
                seq.span = moved;
            }
        }),
        Box::new(move |q| {
            if let Some(seq) = &mut q.machines[0].states[0].sequence {
                if let takt_mir::machine::SeqItem::Until { span, .. } = &mut seq.items[2] {
                    *span = moved;
                }
            }
        }),
        Box::new(move |q| {
            if let takt_mir::stmt::StmtKind::Match { arms, .. } = &mut q.machines[0].states[0].loop_block.stmts[5].kind
            {
                arms[0].span = moved;
            }
        }),
        Box::new(move |q| {
            if let takt_mir::stmt::StmtKind::Assign { value, .. } = &mut q.machines[0].states[0].enter.stmts[0].kind {
                value.span = moved;
            }
        }),
    ];
    let base = logic_hash(&p);
    for (i, case) in cases.iter().enumerate() {
        let mut q = p.clone();
        case(&mut q);
        assert_ne!(q, p, "Fall {i} aendert nichts");
        assert_eq!(logic_hash(&q), base, "Fall {i}: Position im Logik-Hash");
        assert_ne!(program_hash(&q), program_hash(&p), "Fall {i}: Position nicht im Programm-Hash");
    }
    // Eine zusaetzliche gescopte Instanz aendert die Logik, ihre Position nicht.
    let instance =
        |span| takt_mir::machine::ScopedInstance { machine: MachineId(1), scope: StateId(0), resume: false, span };
    let mut q = p.clone();
    q.machines[0].states[0].instances.push(instance(moved));
    let mut r = p.clone();
    r.machines[0].states[0].instances.push(instance(Span::default()));
    assert_ne!(logic_hash(&q), base);
    assert_eq!(logic_hash(&q), logic_hash(&r));
}

/// Die Annotationen aus M3 (3.4) ueberstehen das Dateiformat: bewiesenes
/// Intervall und gewaehlte Darstellung. `full_program_round_trips` vergleicht
/// zwar ganze Programme, sagt aber nicht, *welches* Feld fehlte — dieser Test
/// nennt die beiden neuen beim Namen.
#[test]
fn range_and_repr_survive_the_format() {
    let p = full_program();
    let bytes = write_program(&p, "takt 0.1.0");
    let (_, back) = read_program(&bytes).expect("lesbar");

    let before = annotations(&p);
    assert!(!before.is_empty(), "das Beispielprogramm traegt Annotationen");
    assert_eq!(before, annotations(&back), "Intervall und Darstellung kommen zurueck");
}

/// Intervall und Darstellung jeder annotierten Zuweisung, aus allen drei
/// Bloecken eines Zustands — nicht nur dem `loop:`.
fn annotations(p: &Program) -> Vec<(Option<types::Range>, Option<expr::Repr>)> {
    let mut out = Vec::new();
    for m in &p.machines {
        for s in &m.states {
            for b in [&s.enter, &s.loop_block, &s.exit] {
                for st in &b.stmts {
                    if let stmt::StmtKind::Assign { value, .. } = &st.kind {
                        if value.range.is_some() || value.repr.is_some() {
                            out.push((value.range, value.repr));
                        }
                    }
                }
            }
        }
    }
    out
}

/// Die beiden Felder der Formatversion 6 ueberstehen das Dateiformat:
/// `within` an einem `check` (9.4.5) und das deklarierte Budget einer
/// Maschine (7.2). Ohne diesen Test liefe der Roundtrip leer darueber.
#[test]
fn within_and_declared_budget_survive_the_format() {
    let p = full_program();
    let bytes = write_program(&p, "takt 0.1.0");
    let (_, back) = read_program(&bytes).expect("lesbar");

    let before = withins(&p);
    assert!(!before.is_empty(), "das Beispielprogramm hat ein `within`");
    assert_eq!(before, withins(&back), "`within` kommt zurueck");

    let budgets: Vec<_> = p.machines.iter().map(|m| m.declared_budget).collect();
    let after: Vec<_> = back.machines.iter().map(|m| m.declared_budget).collect();
    assert!(budgets.iter().any(|b| b.is_some_and(|b| b.ram.is_some())), "eine Maschine deklariert ein Budget");
    assert_eq!(budgets, after, "das deklarierte Budget kommt zurueck");
}

/// Die geforderten Latenzen aller `check`-Anweisungen.
fn withins(p: &Program) -> Vec<expr::ExprKind> {
    let mut out = Vec::new();
    for m in &p.machines {
        let states = m.states.iter().flat_map(|s| [&s.enter, &s.loop_block, &s.exit]);
        for b in std::iter::once(&m.loop_block).chain(states) {
            for st in &b.stmts {
                if let stmt::StmtKind::Check { within: Some(w), .. } = &st.kind {
                    out.push(w.kind.clone());
                }
            }
        }
    }
    out
}
