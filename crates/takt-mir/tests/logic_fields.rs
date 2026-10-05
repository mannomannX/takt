//! **Jedes Logikfeld geht in den Logik-Hash ein** (11.3, SYN-028).
//!
//! `roundtrip.rs` haelt fest, dass jedes `meta`-Feld des Schemas einen Grund
//! hat. Hier die Gegenrichtung, am Draht statt an einer Liste von Hand:
//! Jedes Blatt der Datei von `full_program` wird einzeln veraendert und die
//! Datei neu gelesen. Liegt das Blatt unter einem `meta`-Feld des Schemas,
//! bleibt der Logik-Hash gleich, sonst aendert er sich. Welcher Knotentyp
//! an einer Stelle steht, sagt der Leser selbst: Ein Knotenfeld, durch einen
//! Varint ersetzt, scheitert mit dem Namen seines Typs.

use std::collections::HashMap;

use takt_mir::Program;
use takt_mir::format::wire::{FormatError, get_varint, put_varint};
use takt_mir::format::{decode_body, encode_body};
use takt_mir::hash::logic_hash;
use takt_mir::sample::full_program;

/// Ein Feld am Draht.
#[derive(Clone, Debug)]
enum Item {
    Varint(u64),
    Fixed(u64),
    /// Bytes, deren Typ (noch) nicht bekannt ist, oder eine Bytefolge.
    Raw(Vec<u8>),
    /// Ein Knoten mit dem Namen seines Typs.
    Node(String, Vec<(u32, Item)>),
}

fn parse(buf: &[u8]) -> Vec<(u32, Item)> {
    let mut pos = 0;
    let mut out = Vec::new();
    while pos < buf.len() {
        let key = get_varint(buf, &mut pos).expect("Schluessel");
        let item = match key & 3 {
            0 => Item::Varint(get_varint(buf, &mut pos).expect("Varint")),
            1 => {
                let v = u64::from_le_bytes(buf[pos..pos + 8].try_into().expect("8 Bytes"));
                pos += 8;
                Item::Fixed(v)
            }
            _ => {
                let len = get_varint(buf, &mut pos).expect("Laenge") as usize;
                pos += len;
                Item::Raw(buf[pos - len..pos].to_vec())
            }
        };
        out.push(((key >> 2) as u32, item));
    }
    out
}

/// Schreibt die Felder; an `at` steht `instead` statt des Felds dort.
fn write(fields: &[(u32, Item)], at: &[usize], instead: &Item, out: &mut Vec<u8>) {
    for (i, (tag, item)) in fields.iter().enumerate() {
        let here = at.first() == Some(&i);
        let item = if here && at.len() == 1 { instead } else { item };
        match item {
            Item::Varint(v) => {
                put_varint(out, u64::from(*tag) << 2);
                put_varint(out, *v);
            }
            Item::Fixed(v) => {
                put_varint(out, (u64::from(*tag) << 2) | 1);
                out.extend_from_slice(&v.to_le_bytes());
            }
            Item::Raw(b) => {
                put_varint(out, (u64::from(*tag) << 2) | 2);
                put_varint(out, b.len() as u64);
                out.extend_from_slice(b);
            }
            Item::Node(_, children) => {
                let mut inner = Vec::new();
                write(children, if here { &at[1..] } else { &[] }, instead, &mut inner);
                put_varint(out, (u64::from(*tag) << 2) | 2);
                put_varint(out, inner.len() as u64);
                out.extend_from_slice(&inner);
            }
        }
    }
}

fn get<'a>(fields: &'a [(u32, Item)], at: &[usize]) -> &'a Item {
    let item = &fields[at[0]].1;
    match (item, at.len()) {
        (_, 1) => item,
        (Item::Node(_, children), _) => get(children, &at[1..]),
        _ => panic!("kein Knoten an {at:?}"),
    }
}

fn get_mut<'a>(fields: &'a mut [(u32, Item)], at: &[usize]) -> &'a mut Item {
    let item = &mut fields[at[0]].1;
    if at.len() == 1 {
        return item;
    }
    match item {
        Item::Node(_, children) => get_mut(children, &at[1..]),
        _ => panic!("kein Knoten an {at:?}"),
    }
}

/// Die Felder eines Knotentyps: Nummer und ob sie `meta` sind; bei einem
/// Enum je Variante.
#[derive(Debug)]
enum Shape {
    Struct(HashMap<u32, bool>),
    Enum(HashMap<u64, HashMap<u32, bool>>),
}

const MODES: [&str; 7] = ["one", "dflt", "opt", "rep", "meta", "metaopt", "metarep"];

/// Liest die Knotentypen aus `schema.rs`, dazu die von Hand geschriebenen
/// Codecs (`codec.rs`): `Span`, Paare und die Wurzel.
fn schema() -> HashMap<String, Shape> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/format/schema.rs");
    let text: String = std::fs::read_to_string(path)
        .expect("Schema lesbar")
        .lines()
        .map(|l| l.split("//").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");
    let mut out = HashMap::new();
    let plain = |tags: &[u32]| Shape::Struct(tags.iter().map(|t| (*t, false)).collect());
    out.insert("Span".to_string(), plain(&[1, 2, 3]));
    out.insert("Paar".to_string(), plain(&[1, 2]));
    out.insert("Wurzel".to_string(), plain(&[1]));
    for (kind, rest) in [("struct", "codec_struct!("), ("enum", "codec_enum!(")]
        .into_iter()
        .flat_map(|(k, m)| text.split(m).skip(1).map(move |r| (k, r)))
    {
        let body = &rest[..rest.find(");").expect("Ende des Makros")];
        let tokens: Vec<&str> =
            body.split(|c: char| c.is_whitespace() || "{}(),".contains(c)).filter(|t| !t.is_empty()).collect();
        let (name, tokens) = tokens.split_first().expect("Name");
        let is_mode = |t: &str| MODES.contains(&t);
        let shape = if kind == "struct" {
            Shape::Struct(tokens.chunks(3).map(|c| (c[0].parse().expect("Nummer"), c[1].starts_with("meta"))).collect())
        } else {
            let mut variants: HashMap<u64, HashMap<u32, bool>> = HashMap::new();
            let (mut current, mut i) = (0u64, 0);
            while i < tokens.len() {
                let n = tokens[i].parse().expect("Nummer");
                if tokens.get(i + 1).is_some_and(|t| is_mode(t)) {
                    variants.entry(current).or_default().insert(n as u32, tokens[i + 1].starts_with("meta"));
                    i += 3;
                } else {
                    current = n;
                    variants.entry(current).or_default();
                    i += 2;
                }
            }
            Shape::Enum(variants)
        };
        out.insert((*name).to_string(), shape);
    }
    out
}

/// Typisiert jedes Bytefeld: Durch einen Varint ersetzt, nennt der Leser den
/// Typ, den er dort erwartet. Ein Typ des Schemas wird zum Knoten, alles
/// andere (`Dimension`) bleibt eine Bytefolge.
fn typed(strings: &[String], body: &[u8], shapes: &HashMap<String, Shape>) -> Vec<(u32, Item)> {
    let mut root = parse(body);
    let mut open: Vec<Vec<usize>> = (0..root.len()).map(|i| vec![i]).collect();
    while let Some(at) = open.pop() {
        let Item::Raw(bytes) = get(&root, &at).clone() else { continue };
        let mut probe = Vec::new();
        write(&root, &at, &Item::Varint(0), &mut probe);
        let name = match decode_body(strings.to_vec(), &probe) {
            Err(FormatError::WrongWire(name, _)) => name,
            other => panic!("Typ an {at:?} nicht erkennbar: {other:?}"),
        };
        if shapes.contains_key(name) {
            let children = parse(&bytes);
            open.extend((0..children.len()).map(|i| [at.as_slice(), &[i]].concat()));
            *get_mut(&mut root, &at) = Item::Node(name.to_string(), children);
        }
    }
    root
}

/// Liegt das Blatt an `at` unter einem `meta`-Feld? Dazu der Weg als Text.
fn classify(fields: &[(u32, Item)], at: &[usize], ty: &str, shapes: &HashMap<String, Shape>) -> (bool, String) {
    let (tag, item) = &fields[at[0]];
    let (meta, here) = match shapes.get(ty).unwrap_or_else(|| panic!("Typ {ty} fehlt im Schema")) {
        Shape::Struct(f) => (*f.get(tag).unwrap_or_else(|| panic!("{ty} hat kein Feld {tag}")), format!("{ty}.{tag}")),
        Shape::Enum(variants) => {
            let variant = fields.iter().find_map(|(t, i)| match (t, i) {
                (0, Item::Varint(v)) => Some(*v),
                _ => None,
            });
            let variant = variant.unwrap_or_else(|| panic!("{ty} ohne Variante"));
            let meta = *tag != 0
                && *variants[&variant].get(tag).unwrap_or_else(|| panic!("{ty}#{variant} hat kein Feld {tag}"));
            (meta, format!("{ty}#{variant}.{tag}"))
        }
    };
    match item {
        Item::Node(name, children) if at.len() > 1 => {
            let (below, path) = classify(children, &at[1..], name, shapes);
            (meta || below, format!("{here}/{path}"))
        }
        _ => (meta, here),
    }
}

/// Alle Blaetter: Wege zu Varints, festen Feldern und Bytefolgen.
fn leaves(fields: &[(u32, Item)], prefix: &[usize], out: &mut Vec<Vec<usize>>) {
    for (i, (_, item)) in fields.iter().enumerate() {
        let at = [prefix, &[i]].concat();
        match item {
            Item::Node(_, children) => leaves(children, &at, out),
            _ => out.push(at),
        }
    }
}

/// Das Blatt, veraendert; `None`, wenn es nichts zu veraendern gibt.
fn mutations(item: &Item) -> Vec<Item> {
    match item {
        Item::Varint(v) => [v.checked_add(1), v.checked_sub(1)].into_iter().flatten().map(Item::Varint).collect(),
        Item::Fixed(v) => vec![Item::Fixed(v ^ 1)],
        Item::Raw(b) if !b.is_empty() => {
            let mut b = b.clone();
            b[0] ^= 1;
            vec![Item::Raw(b)]
        }
        _ => Vec::new(),
    }
}

#[test]
fn every_logic_field_changes_the_logic_hash() {
    let p = full_program();
    let shapes = schema();
    let (strings, body) = encode_body(&p, false);
    let root = typed(&strings, &body, &shapes);
    let hash = logic_hash(&p);
    let mut all = Vec::new();
    leaves(&root, &[], &mut all);
    let (mut logic, mut meta, mut wrong) = (0usize, 0usize, Vec::new());
    // Ein Feld genuegt je Ort im Schema (Typ, Variante, Nummer) und je
    // Antwort auf `meta`: Weitere Vorkommen pruefen dieselbe Zeile des
    // Codecs noch einmal.
    let mut seen = std::collections::HashSet::new();
    for at in &all {
        let (is_meta, path) = classify(&root, at, "Wurzel", &shapes);
        let field = path.rsplit('/').next().unwrap_or(&path).to_string();
        if seen.contains(&(field.clone(), is_meta)) {
            continue;
        }
        // Die erste Veraenderung, die sich lesen laesst und das Programm
        // aendert; ein Wert ausserhalb seines Typs liest sich nicht.
        let changed: Option<Program> = mutations(get(&root, at)).into_iter().find_map(|m| {
            let mut buf = Vec::new();
            write(&root, at, &m, &mut buf);
            decode_body(strings.clone(), &buf).ok().filter(|q| *q != p)
        });
        let Some(q) = changed else { continue };
        seen.insert((field, is_meta));
        let moved = logic_hash(&q) != hash;
        if is_meta {
            meta += 1;
        } else {
            logic += 1;
        }
        if moved == is_meta {
            wrong.push(format!("{path}: meta = {is_meta}, Logik-Hash geaendert = {moved}"));
        }
    }
    assert!(logic > 100 && meta > 10, "{logic} Logik- und {meta} meta-Felder veraendert");
    assert!(wrong.is_empty(), "{} Blaetter widersprechen dem Schema:\n{}", wrong.len(), wrong.join("\n"));
}
