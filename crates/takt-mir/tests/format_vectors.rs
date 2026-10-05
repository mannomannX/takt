//! Die Vektoren aus grammar/mir-format.md (Bloecke ```mir).

use takt_mir::format::wire::{Node, Raw, Reader, Wire, Writer, get_varint, put_varint, unzigzag, zigzag};
use takt_mir::hash::sha256;

fn spec() -> String {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../grammar/mir-format.md");
    std::fs::read_to_string(path).expect("grammar/mir-format.md lesbar")
}

fn vectors() -> Vec<(String, String, String)> {
    let text = spec();
    let mut out = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        if line.starts_with("```mir") {
            inside = true;
            continue;
        }
        if line.starts_with("```") {
            inside = false;
            continue;
        }
        if !inside {
            continue;
        }
        let (kind, rest) = line.split_once(':').expect("art: eingabe -> hex");
        let (input, hex) = rest.split_once("->").expect("eingabe -> hex");
        out.push((kind.trim().to_string(), input.trim().to_string(), hex.trim().to_string()));
    }
    assert!(out.len() >= 25, "Vektoren gefunden: {}", out.len());
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
}

fn unhex(s: &str) -> Vec<u8> {
    s.split_whitespace().map(|h| u8::from_str_radix(h, 16).expect("Hex")).collect()
}

/// `1:varint=5 2:zigzag=-3 1:bytes=0102 3:fixed64=1.0 1:node(1:varint=7)`
fn encode_fields(w: &mut Writer, spec: &str) {
    let mut rest = spec.trim();
    while !rest.is_empty() {
        let (tag, after) = rest.split_once(':').expect("nummer:");
        let tag: u32 = tag.parse().expect("Feldnummer");
        let (kind, value, tail) = if let Some(inner) = after.strip_prefix("node(") {
            let end = inner.find(')').expect(")");
            ("node", &inner[..end], inner[end + 1..].trim_start())
        } else {
            let (kind, tail) = after.split_once('=').expect("art=wert");
            let (value, tail) = match tail.find(' ') {
                Some(i) => (&tail[..i], tail[i..].trim_start()),
                None => (tail, ""),
            };
            (kind, value, tail)
        };
        match kind {
            "varint" => w.varint(tag, value.parse().expect("Zahl")),
            "zigzag" => w.signed(tag, value.parse().expect("Zahl")),
            "fixed64" => w.fixed64(tag, value.parse::<f64>().expect("f64").to_bits()),
            "bytes" => {
                let bytes: Vec<u8> = (0..value.len())
                    .step_by(2)
                    .map(|i| u8::from_str_radix(&value[i..i + 2], 16).expect("Hex"))
                    .collect();
                w.bytes(tag, &bytes);
            }
            "node" => {
                w.begin();
                encode_fields(w, value);
                w.end(tag);
            }
            k => panic!("unbekannte Art {k}"),
        }
        rest = tail;
    }
}

/// Prueft einen gelesenen Knoten gegen die Feldliste eines Vektors, Feld
/// fuer Feld in der Reihenfolge der Liste: Die Art sagt, wie der Wert zu
/// lesen ist.
fn check_fields(node: &Node<'_>, spec: &str) {
    let mut seen: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
    let mut rest = spec.trim();
    while !rest.is_empty() {
        let (tag, after) = rest.split_once(':').expect("nummer:");
        let tag: u32 = tag.parse().expect("Feldnummer");
        let index = seen.entry(tag).or_default();
        let raw = node.all(tag).nth(*index).unwrap_or_else(|| panic!("Feld {tag} fehlt: {spec}"));
        *index += 1;
        if let Some(inner) = after.strip_prefix("node(") {
            let end = inner.find(')').expect(")");
            let Raw::Bytes(b) = raw else { panic!("Feld {tag} ist kein Knoten: {spec}") };
            check_fields(&Node::parse("Kind", b).expect("Knoten"), &inner[..end]);
            rest = inner[end + 1..].trim_start();
            continue;
        }
        let (kind, tail) = after.split_once('=').expect("art=wert");
        let (value, tail) = match tail.find(' ') {
            Some(i) => (&tail[..i], tail[i..].trim_start()),
            None => (tail, ""),
        };
        let ok = match (kind, raw) {
            ("varint", Raw::Varint(v)) => v == value.parse::<u64>().expect("Zahl"),
            ("zigzag", Raw::Varint(v)) => unzigzag(v) == value.parse::<i64>().expect("Zahl"),
            ("fixed64", Raw::Fixed64(b)) => b == value.parse::<f64>().expect("f64").to_bits(),
            ("bytes", Raw::Bytes(b)) => hex(b).replace(' ', "") == value,
            _ => false,
        };
        assert!(ok, "Feld {tag} ({kind}={value}) liest sich als {raw:?}: {spec}");
        rest = tail;
    }
    let total: usize = seen.values().sum();
    let present: usize = seen.keys().map(|t| node.all(*t).count()).sum();
    assert_eq!(total, present, "mehr Felder als die Liste: {spec}");
}

#[test]
fn vectors_match() {
    for (kind, input, expected) in vectors() {
        match kind.as_str() {
            "varint" => {
                let v: u64 = input.parse().expect("Zahl");
                let mut buf = Vec::new();
                put_varint(&mut buf, v);
                assert_eq!(hex(&buf), expected, "varint {input}");
                let mut pos = 0;
                assert_eq!(get_varint(&unhex(&expected), &mut pos).expect("lesbar"), v);
                assert_eq!(pos, buf.len());
            }
            "zigzag" => {
                let v: i64 = input.parse().expect("Zahl");
                let mut buf = Vec::new();
                put_varint(&mut buf, zigzag(v));
                assert_eq!(hex(&buf), expected, "zigzag {input}");
                // SYN-031: auch die Gegenrichtung aus den erwarteten Bytes.
                let mut pos = 0;
                assert_eq!(unzigzag(get_varint(&unhex(&expected), &mut pos).expect("lesbar")), v, "zigzag {input}");
            }
            "f64" => {
                let v: f64 = input.parse().expect("f64");
                assert_eq!(hex(&v.to_bits().to_le_bytes()), expected, "f64 {input}");
                let bytes: [u8; 8] = unhex(&expected).try_into().expect("8 Bytes");
                assert_eq!(f64::from_le_bytes(bytes).to_bits(), v.to_bits(), "f64 {input}");
            }
            "key" => {
                let (tag, wire) = input.split_once('/').expect("nummer/art");
                let tag: u32 = tag.parse().expect("Nummer");
                let mut w = Writer::new(false);
                match wire {
                    "varint" => w.varint(tag, 0),
                    "fixed64" => w.fixed64(tag, 0),
                    "bytes" => w.bytes(tag, &[]),
                    _ => panic!("Drahtart"),
                }
                let (_, bytes) = w.finish();
                let key_len = bytes.len()
                    - match wire {
                        "varint" => 1,
                        "fixed64" => 8,
                        _ => 1,
                    };
                assert_eq!(hex(&bytes[..key_len]), expected, "key {input}");
            }
            "node" => {
                let mut w = Writer::new(false);
                encode_fields(&mut w, &input);
                let (_, bytes) = w.finish();
                assert_eq!(hex(&bytes), expected, "node {input}");
                // SYN-031: Die Bytes lesen sich zurueck in genau die Felder der Eingabe.
                let raw = unhex(&expected);
                let node = Node::parse("Vektor", &raw).expect("lesbar");
                check_fields(&node, &input);
            }
            "sha256" => {
                assert_eq!(sha256(input.as_bytes()).to_string(), expected, "sha256 {input:?}");
            }
            k => panic!("unbekannte Art {k}"),
        }
    }
}

#[test]
fn reader_rejects_bad_wire_and_truncation() {
    assert!(matches!(Node::parse("x", &[0x03]), Err(takt_mir::FormatError::BadWire(3))));
    assert!(matches!(Node::parse("x", &[0x05, 1, 2]), Err(takt_mir::FormatError::Truncated)));
    assert!(matches!(Node::parse("x", &[0x06, 5, 1]), Err(takt_mir::FormatError::Truncated)));
    assert!(matches!(Node::parse("x", &[0x04]), Err(takt_mir::FormatError::Truncated)));
    let long = [0x04, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01];
    assert!(matches!(Node::parse("x", &long), Err(takt_mir::FormatError::BadVarint)));
    let node = Node::parse("x", &[0x04, 1, 0x04, 2]).expect("zwei Felder");
    assert!(matches!(node.one(1), Err(takt_mir::FormatError::Duplicate("x", 1))));
    assert!(matches!(node.one(2), Err(takt_mir::FormatError::Missing("x", 2))));
    assert_eq!(node.all(1).collect::<Vec<_>>(), vec![Raw::Varint(1), Raw::Varint(2)]);
    let r = Reader::new(vec![]);
    assert!(matches!(r.string(0), Err(takt_mir::FormatError::BadString(0))));
    assert_eq!(Wire::Bytes as u8, 2);
    // Laenge groesser als jede Adresse: kein Ueberlauf, sondern `Truncated`.
    let huge = [0x06, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01];
    assert!(matches!(Node::parse("x", &huge), Err(takt_mir::FormatError::Truncated)));
    assert!(matches!(takt_mir::format::wire::slice(&[1, 2], 1, u64::MAX), Err(takt_mir::FormatError::Truncated)));
}

/// Ein Feld im Modus `dflt` (W3) fehlt in aelteren Dateien und liest sich
/// als null: ein Kostenvektor ohne die Felder 8 bis 15.
#[test]
fn a_cost_vector_without_its_later_fields_reads_as_zero() {
    use takt_mir::format::codec::Field;
    let mut w = Writer::new(false);
    w.begin();
    for tag in 1..=7 {
        w.varint(tag, u64::from(tag));
    }
    w.end(1);
    let (_, bytes) = w.finish();
    let root = Node::parse("Wurzel", &bytes).expect("Knoten");
    let v = takt_mir::fns::CostVec::read(root.one(1).expect("Feld"), &Reader::new(vec![])).expect("lesbar");
    let seven =
        takt_mir::fns::CostVec { i32: 1, i64: 2, f32: 3, f64: 4, mem: 5, call: 6, native: 7, ..Default::default() };
    assert_eq!(v, seven);
}

/// Fehlermeldungen nennen Knoten und Feldnummer, auch fuer Primitive.
#[test]
fn errors_name_node_and_field() {
    use takt_mir::format::codec::Field;
    let r = Reader::new(vec![]);
    let mut w = Writer::new(false);
    w.begin();
    w.fixed64(1, 0);
    w.end(1);
    let (_, bytes) = w.finish();
    let root = Node::parse("Wurzel", &bytes).expect("Knoten");
    let err = takt_mir::types::Rational::read(root.one(1).expect("Feld"), &r).expect_err("i64 aus fixed64");
    assert_eq!(err, takt_mir::FormatError::WrongWire("i64", 1));
    assert_eq!(err.to_string(), "i64: Feld 1 hat die falsche Drahtart");
    let r = Reader::new(vec!["n".to_string()]);
    let mut w = Writer::new(false);
    w.begin();
    w.varint(1, 0);
    w.varint(2, 0);
    w.varint(3, 300);
    w.varint(4, 0);
    w.end(1);
    let (_, bytes) = w.finish();
    let root = Node::parse("Wurzel", &bytes).expect("Knoten");
    let err = takt_mir::types::BitfieldDef::read(root.one(1).expect("Feld"), &r).expect_err("u8 > 255");
    assert!(matches!(err, takt_mir::FormatError::OutOfRange("u8", 3)), "{err:?}");
}

/// Schreibt `v` als Feld 1 und gibt die Bytes des umgebenden Knotens.
fn field_bytes(f: impl FnOnce(&mut Writer)) -> Vec<u8> {
    let mut w = Writer::new(false);
    f(&mut w);
    w.finish().1
}

/// SYN-031: je Fehlerart des Lesers ein Negativvektor mit erwartetem Fehler.
#[test]
fn every_reader_error_has_a_negative_vector() {
    use takt_mir::FormatError;
    use takt_mir::format::codec::Field;
    let r = Reader::new(vec!["s".to_string()]);
    fn read_root(bytes: &[u8]) -> Node<'_> {
        Node::parse("Wurzel", bytes).expect("Knoten")
    }
    // Unbekannte Variante: ohne Nutzlast ein Varint, mit Nutzlast Feld 0.
    let bytes = field_bytes(|w| w.varint(1, 99));
    let err = takt_mir::expr::UnaryOp::read(read_root(&bytes).one(1).expect("Feld"), &r).expect_err("Variante 99");
    assert_eq!(err, FormatError::BadVariant("UnaryOp", 99));
    let bytes = field_bytes(|w| {
        w.begin();
        w.varint(0, 999);
        w.end(1);
    });
    let err = takt_mir::expr::ExprKind::read(read_root(&bytes).one(1).expect("Feld"), &r).expect_err("Variante 999");
    assert_eq!(err, FormatError::BadVariant("ExprKind", 999));
    // W1: bool ist 0 oder 1; 2 ist ein Fehler, mit der Feldnummer.
    let bytes = field_bytes(|w| {
        w.begin();
        w.varint(0, 0);
        w.varint(1, 2);
        w.end(1);
    });
    let err = takt_mir::expr::ExprKind::read(read_root(&bytes).one(1).expect("Feld"), &r).expect_err("bool 2");
    assert_eq!(err, FormatError::OutOfRange("bool", 1));
    // Ein `opt`-Feld doppelt: `Expr.range` (Feld 3).
    let bytes = field_bytes(|w| {
        w.begin();
        w.begin();
        w.varint(0, 0);
        w.varint(1, 1);
        w.end(1);
        w.varint(2, 0);
        for _ in 0..2 {
            w.begin();
            w.end(3);
        }
        w.end(1);
    });
    let err = takt_mir::expr::Expr::read(read_root(&bytes).one(1).expect("Feld"), &r).expect_err("range doppelt");
    assert_eq!(err, FormatError::Duplicate("Expr", 3));
    // Eine Stringnummer ausserhalb der Tabelle.
    assert_eq!(r.string(1), Err(FormatError::BadString(1)));
}

/// SYN-031: Kopf und Stringtabelle — kein UTF-8 in der Compiler-Version
/// oder in einem String, und eine Stringanzahl ueber das Dateiende hinaus.
#[test]
fn the_header_and_the_string_table_are_checked() {
    use takt_mir::FormatError;
    use takt_mir::format::{read_header, read_program};
    let head = |version: &[u8]| {
        let mut b = b"TAKT-MIR".to_vec();
        b.extend_from_slice(&takt_mir::format::FORMAT_VERSION.to_le_bytes());
        b.extend_from_slice(&1u32.to_le_bytes());
        put_varint(&mut b, version.len() as u64);
        b.extend_from_slice(version);
        b
    };
    assert_eq!(read_header(&head(&[0xff, 0xfe])).map(|_| ()), Err(FormatError::BadUtf8));
    let mut file = head(b"takt");
    put_varint(&mut file, 1);
    put_varint(&mut file, 2);
    file.extend_from_slice(&[0xc3, 0x28]);
    assert_eq!(read_program(&file).map(|_| ()), Err(FormatError::BadUtf8));
    let mut file = head(b"takt");
    put_varint(&mut file, 1 << 40);
    put_varint(&mut file, 1);
    file.push(b'a');
    assert_eq!(read_program(&file).map(|_| ()), Err(FormatError::Truncated));
}

/// Ein Feld mit einer Nummer jenseits von u32 ist unbekannt und wird
/// ueberlesen (W2) — es darf nicht als Feld `nummer mod 2^32` gelten.
#[test]
fn a_field_number_beyond_u32_is_unknown() {
    let mut bytes = Vec::new();
    put_varint(&mut bytes, ((1u64 << 32) + 1) << 2);
    put_varint(&mut bytes, 7);
    let node = Node::parse("x", &bytes).expect("lesbar");
    assert!(matches!(node.one(1), Err(takt_mir::FormatError::Missing("x", 1))));
}

/// Ein Ausdruck aus `depth` geschachtelten Negationen um `true`, als
/// Bytes eines `Expr`-Knotens; von aussen nach innen geschrieben, damit
/// die Laengen nicht wiederholt kopiert werden.
fn nested(depth: usize) -> Vec<u8> {
    let varint = |v: u64| {
        let mut b = Vec::new();
        put_varint(&mut b, v);
        b
    };
    // expr_0 = {1: {0: 0, 1: 1}, 2: 0}
    let inner: Vec<u8> = vec![0x06, 0x04, 0x00, 0x00, 0x04, 0x01, 0x08, 0x00];
    // expr_k = {1: kind_k, 2: 0}, kind_k = {0: 26, 1: 0, 2: expr_(k-1)}
    let mut sizes = vec![inner.len() as u64];
    let mut kinds = Vec::new();
    for k in 1..=depth {
        let below = sizes[k - 1];
        let kind = 5 + varint(below).len() as u64 + below;
        kinds.push(kind);
        sizes.push(1 + varint(kind).len() as u64 + kind + 2);
    }
    let mut out = Vec::with_capacity(sizes[depth] as usize);
    for k in (1..=depth).rev() {
        out.push(0x06);
        out.extend(varint(kinds[k - 1]));
        out.extend([0x00, 26, 0x04, 0x00, 0x0a]);
        out.extend(varint(sizes[k - 1]));
    }
    out.extend(&inner);
    for _ in 0..depth {
        out.extend([0x08, 0x00]);
    }
    out
}

/// SYN-031: Tief geschachtelte Knoten. Ein Leser fremder Dateien darf nicht
/// am Stapel scheitern; jenseits der Tiefe, die ein uebersetzbares Programm
/// erreicht (Ausdruecke hoechstens 256 Knoten tief, 2.2), ist die Datei ein
/// Fehler.
#[test]
fn a_nesting_deeper_than_any_program_is_an_error() {
    use takt_mir::format::codec::Field;
    let r = Reader::new(vec![]);
    let read = |depth: usize| {
        let mut bytes = Vec::new();
        put_varint(&mut bytes, (1 << 2) | 2);
        let body = nested(depth);
        put_varint(&mut bytes, body.len() as u64);
        bytes.extend(body);
        let root = Node::parse("Wurzel", &bytes).expect("Knoten");
        takt_mir::expr::Expr::read(root.one(1).expect("Feld"), &r).map(|_| ())
    };
    assert_eq!(read(300), Ok(()), "300 Ebenen liest jeder Leser");
    assert_eq!(read(100_000), Err(takt_mir::FormatError::TooDeep("Expr")));
}
