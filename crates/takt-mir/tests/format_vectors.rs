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
                assert_eq!(unzigzag(zigzag(v)), v);
            }
            "f64" => {
                let v: f64 = input.parse().expect("f64");
                assert_eq!(hex(&v.to_bits().to_le_bytes()), expected, "f64 {input}");
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
                let node = Node::parse("Vektor", &bytes).expect("lesbar");
                assert!(node.all(0).count() + node.all(1).count() + node.all(2).count() + node.all(3).count() > 0);
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
