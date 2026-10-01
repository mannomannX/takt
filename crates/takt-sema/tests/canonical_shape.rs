//! Der Leser der TCB (`takt_native::bytes::decodes`) urteilt wie der
//! Leser des Interpreters (`takt_interp::bytes::decode`): Er bekommt statt
//! des Typs nur dessen Gestalt (`takt_mir::bytes::shape`), und Satz 9.4.4
//! gilt fuer Zeile 5 aus 12.6 nur, wenn beide jede Eingabe gleich
//! beurteilen — gueltige, verdorbene und zufaellige.

use takt_diag::Policy;
use takt_interp::bytes::{decode, decode_slot};
use takt_mir::bytes::shape;
use takt_mir::{Program, TypeId};
use takt_native::bytes::{decodes, shape as op};
use takt_sema::{Build, Options};

const SRC: &str = "system:
    language = 1
    tick = 1 ms

enum Mode: OFF, ON(level: u8), FAULT(code: u16, hold: bool)

record Inner:
    good : bool
    hold : Duration
    gain : f32

record Big:
    mode  : Mode
    xs    : [3] i16
    name  : str<5>
    data  : bytes<4>
    small : vec<u8, 3>
    inner : Inner
    scale : f64

output n : int in 0..9 @ hw(\"o/n\") with safe = 0

machine m:
    persist var big   : Big = default
    persist var table : map<u8, i32, 3> = default
    persist var modes : vec<Mode, 2> = default
    initial RUN
    state RUN:
        loop:
            n = 1
";

fn program() -> Program {
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
    let out = takt_sema::compile(SRC, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "unerwartete Fehler:\n{}", errors.join("\n"));
    out.program.expect("Programm")
}

/// Die Typen der `persist`-Variablen, je mit Namen.
fn types(p: &Program) -> Vec<(String, TypeId)> {
    let m = p.machines.iter().find(|m| !m.persist.is_empty()).expect("Maschine");
    m.persist.iter().map(|v| (m.vars[v.var.index()].name.clone(), m.vars[v.var.index()].ty)).collect()
}

/// xorshift64*, deterministisch.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

/// Ein Lesezeiger in einer Gestalt.
struct Shape<'a> {
    s: &'a [u8],
    at: usize,
}

impl Shape<'_> {
    fn u8(&mut self) -> u8 {
        self.at += 1;
        self.s[self.at - 1]
    }

    fn u16(&mut self) -> u16 {
        self.at += 2;
        u16::from_le_bytes([self.s[self.at - 2], self.s[self.at - 1]])
    }

    fn u32(&mut self) -> u32 {
        self.at += 4;
        u32::from_le_bytes(self.s[self.at - 4..self.at].try_into().expect("vier"))
    }

    fn i64(&mut self) -> i64 {
        self.at += 8;
        i64::from_le_bytes(self.s[self.at - 8..self.at].try_into().expect("acht"))
    }
}

/// Ein gueltiger Wert der Gestalt als Bytes — eine Erzeugung nur fuer den
/// Test, damit der Vergleich viele gueltige Eingaben sieht.
fn valid(s: &mut Shape<'_>, rng: &mut Rng, out: &mut Vec<u8>) {
    match s.u8() {
        op::BOOL => out.push(rng.below(2) as u8),
        op::INT => {
            let w = s.u8() as usize;
            out.extend(&rng.next().to_le_bytes()[..w]);
        }
        op::F32 => out.extend(&(rng.next() as u32).to_le_bytes()),
        op::F64 | op::DURATION => out.extend(rng.next().to_le_bytes()),
        op::ENUM => {
            let n = s.u16();
            let pick = rng.below(u64::from(n));
            for i in 0..u64::from(n) {
                let d = s.i64();
                let fields = s.u16();
                if i == pick {
                    out.extend(d.to_le_bytes());
                }
                for _ in 0..fields {
                    let mut sink = Vec::new();
                    valid(s, rng, if i == pick { &mut *out } else { &mut sink });
                }
            }
        }
        op::RECORD => {
            for _ in 0..s.u16() {
                valid(s, rng, out);
            }
        }
        op::ARRAY => {
            let n = s.u32();
            let at = s.at;
            for _ in 0..n {
                s.at = at;
                valid(s, rng, out);
            }
        }
        op::BYTES | op::STR => {
            let cap = s.u32();
            let n = rng.below(u64::from(cap) + 1) as u32;
            out.extend(n.to_le_bytes());
            (0..n).for_each(|_| out.push(b'a' + rng.below(26) as u8));
        }
        op::VEC => {
            let cap = s.u32();
            let n = rng.below(u64::from(cap) + 1) as u32;
            out.extend(n.to_le_bytes());
            let at = s.at;
            let mut sink = Vec::new();
            if n == 0 {
                valid(s, rng, &mut sink);
            }
            for _ in 0..n {
                s.at = at;
                valid(s, rng, out);
            }
        }
        op::MAP => {
            let (cap, klen, vlen) = (s.u32(), s.u32() as usize, s.u32() as usize);
            let key = s.at;
            let mut sink = Vec::new();
            valid(s, rng, &mut sink);
            let value = s.at;
            valid(s, rng, &mut sink);
            let end = s.at;
            for _ in 0..cap {
                if rng.below(2) == 0 {
                    out.push(0);
                    out.extend(std::iter::repeat_n(0, klen + vlen));
                    continue;
                }
                out.push(1);
                for (at, width) in [(key, klen), (value, vlen)] {
                    let from = out.len();
                    s.at = at;
                    valid(s, rng, out);
                    let pad = width.saturating_sub(out.len() - from);
                    out.extend(std::iter::repeat_n(0, pad));
                }
            }
            s.at = end;
        }
        other => panic!("Opcode {other} in der Gestalt"),
    }
}

/// Eine verdorbene Fassung: ein Byte gekippt, gekuerzt oder verlaengert.
fn spoil(bytes: &[u8], rng: &mut Rng) -> Vec<u8> {
    let mut b = bytes.to_vec();
    match rng.below(4) {
        0 if !b.is_empty() => {
            let i = rng.below(b.len() as u64) as usize;
            b[i] ^= 1 << rng.below(8);
        }
        1 if !b.is_empty() => {
            let i = rng.below(b.len() as u64) as usize;
            b[i] = rng.next() as u8;
        }
        2 => b.truncate(rng.below(b.len() as u64 + 1) as usize),
        _ => b.push(rng.next() as u8),
    }
    b
}

#[test]
fn the_tcb_reader_judges_like_the_interpreter() {
    let p = program();
    let mut rng = Rng(0x2026_1001_0290);
    let mut checked = 0;
    for (name, ty) in types(&p) {
        let shape = shape(&p, ty).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        for _ in 0..3000 {
            let mut good = Vec::new();
            valid(&mut Shape { s: &shape, at: 0 }, &mut rng, &mut good);
            assert!(decode(&p, &good, ty).is_ok(), "{name}: die Erzeugung ist gueltig: {good:02x?}");
            assert!(decodes(&shape, &good, true), "{name}: gueltig, aber abgelehnt: {good:02x?}");
            for bytes in [spoil(&good, &mut rng), spoil(&good, &mut rng)] {
                assert_eq!(
                    decodes(&shape, &bytes, true),
                    decode(&p, &bytes, ty).is_ok(),
                    "{name}, genau: {bytes:02x?}"
                );
                assert_eq!(
                    decodes(&shape, &bytes, false),
                    decode_slot(&p, &bytes, ty).is_ok(),
                    "{name}, im Slot: {bytes:02x?}"
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 10_000, "{checked} Vergleiche");
}

/// FB-358: `str<N>` fasst N Bytes; ein Text aus N Zeichen mit mehr Bytes
/// ist keiner.
#[test]
fn a_string_holds_n_bytes_not_n_characters() {
    let p = program();
    let ty = types(&p).into_iter().find(|(n, _)| n == "big").expect("big").1;
    let shape = shape(&p, ty).expect("Gestalt");
    let mut rng = Rng(7);
    let mut good = Vec::new();
    valid(&mut Shape { s: &shape, at: 0 }, &mut rng, &mut good);
    // `name` steht hinter `mode` und `xs`; ein Text aus fuenf `ä` hat zehn Bytes.
    let value = takt_interp::bytes::decode(&p, &good, ty).expect("gueltig");
    let takt_interp::Value::Record(mut fields) = value else { panic!("Record") };
    fields[2] = takt_interp::Value::Str("ääääa".into());
    assert!(takt_interp::bytes::encode(&p, &takt_interp::Value::Record(fields.clone()), ty).is_err());
    fields[2] = takt_interp::Value::Str("ääa".into());
    let bytes = takt_interp::bytes::encode(&p, &takt_interp::Value::Record(fields), ty).expect("fuenf Bytes");
    assert!(decodes(&shape, &bytes, true));
}
