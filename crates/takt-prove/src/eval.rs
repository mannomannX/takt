//! Konkrete Auswertung der Terme: dieselbe Semantik, die der Drucker nach
//! SMT-LIB2 traegt — i64 wie `bvsdiv`/`bvsrem`, IEEE-754 in der Breite. Damit
//! laesst sich die Kodierung ohne Solver gegen den Interpreter pruefen
//! (plan/m6.md 2.8): Ein Gegenbeispiel des Solvers oder ein Lauf des
//! Modells muss im Interpreter dasselbe tun.

use std::collections::{BTreeMap, HashMap};
use std::hash::{BuildHasherDefault, Hasher};

use crate::term::{Fun, Node, Op, Rounding, Sort, Term};

/// Ein konkreter Wert.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Val {
    /// `Bool`
    Bool(bool),
    /// Ganzzahl.
    Int(i64),
    /// `float` in f32.
    F32(f32),
    /// `float` in f64.
    F64(f64),
}

impl Val {
    /// Der Nullwert einer Sorte.
    pub fn zero(sort: Sort) -> Val {
        match sort {
            Sort::Bool => Val::Bool(false),
            Sort::Int => Val::Int(0),
            Sort::F32 => Val::F32(0.0),
            Sort::F64 => Val::F64(0.0),
        }
    }

    fn as_bool(self) -> bool {
        matches!(self, Val::Bool(true))
    }

    fn as_int(self) -> i64 {
        match self {
            Val::Int(i) => i,
            Val::Bool(b) => i64::from(b),
            Val::F32(f) => f as i64,
            Val::F64(f) => f as i64,
        }
    }

    fn as_f64(self) -> f64 {
        match self {
            Val::F64(f) => f,
            Val::F32(f) => f64::from(f),
            Val::Int(i) => i as f64,
            Val::Bool(b) => f64::from(u8::from(b)),
        }
    }
}

/// Werte der Variablen.
pub type Env = BTreeMap<String, Val>;

/// Eine Tabelle je Knoten, nach seinem Schluessel (`Term::key`, eine
/// Adresse). SipHash kostete hier mehr als die Auswertung selbst.
type Keys<V> = HashMap<usize, V, BuildHasherDefault<KeyHasher>>;

/// Der Hash eines Knotenschluessels: das Produkt mit einer ungeraden
/// Konstante, die hohen Bits auf die niedrigen gefaltet — eine Adresse hat
/// unten nur Nullen.
#[derive(Default)]
struct KeyHasher(u64);

impl Hasher for KeyHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 = (self.0 ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3);
        }
    }

    fn write_usize(&mut self, n: usize) {
        let x = (n as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        self.0 = x ^ (x >> 32);
    }
}

/// Wertet `t` unter `env`; eine fehlende Variable ist ihr Nullwert. Jeder
/// geteilte Knoten wird einmal ausgewertet: Der Term ist ein Graph, und
/// ohne Gedaechtnis waechst die Arbeit mit der Zahl seiner Pfade.
pub fn eval(t: &Term, env: &Env) -> Val {
    eval_in(t, env, &mut Keys::default())
}

/// Wertet mehrere Terme unter derselben Belegung, mit einem Gedaechtnis
/// fuer alle: Die Folgezustaende eines Modells teilen den groessten Teil
/// ihres Graphen.
pub fn eval_all<'a>(ts: impl IntoIterator<Item = &'a Term>, env: &Env) -> Vec<Val> {
    let mut memo = Keys::default();
    ts.into_iter().map(|t| eval_in(t, env, &mut memo)).collect()
}

/// Ein Stapel statt Rekursion: Ein ausgerollter Pfad ist tausende Knoten
/// tief (FB-403). Ein Knoten, dem ein Argument fehlt, legt sich mit ihm
/// zurueck auf den Stapel; `ite` wertet nur den genommenen Zweig, `and`
/// und `or` brechen ab, sobald ihr Wert feststeht.
fn eval_in(t: &Term, env: &Env, memo: &mut Keys<Val>) -> Val {
    let mut stack = vec![(t.clone(), 0)];
    while let Some((n, from)) = stack.pop() {
        if !memo.contains_key(&n.key())
            && let Some(v) = visit(&n, from, env, memo, &mut stack)
        {
            memo.insert(n.key(), v);
        }
    }
    memo[&t.key()]
}

/// Der Wert von `t`, wenn seine Argumente bekannt sind; sonst legt er sich
/// und das naechste fehlende Argument auf den Stapel. `from` ist bei `and`
/// und `or` das erste noch ungepruefte Argument.
fn visit(t: &Term, from: usize, env: &Env, memo: &Keys<Val>, stack: &mut Vec<(Term, usize)>) -> Option<Val> {
    let Node::App(op, args) = &*t.0 else { return Some(leaf(t, env)) };
    let known = |a: &Term| memo.get(&a.key()).copied();
    let mut wait = |a: &Term, from: usize| {
        stack.push((t.clone(), from));
        stack.push((a.clone(), 0));
        None
    };
    match op {
        Op::Ite => {
            let Some(c) = known(&args[0]) else { return wait(&args[0], 0) };
            let pick = &args[if c.as_bool() { 1 } else { 2 }];
            known(pick).or_else(|| wait(pick, 0))
        }
        Op::And | Op::Or => {
            let decisive = matches!(op, Op::Or);
            for (i, a) in args.iter().enumerate().skip(from) {
                match known(a) {
                    Some(v) if v.as_bool() == decisive => return Some(Val::Bool(decisive)),
                    Some(_) => {}
                    None => return wait(a, i),
                }
            }
            Some(Val::Bool(!decisive))
        }
        _ => match args.iter().find(|a| known(a).is_none()) {
            Some(a) => wait(a, 0),
            None => Some(apply(*op, args.len(), |i| memo[&args[i].key()])),
        },
    }
}

fn leaf(t: &Term, env: &Env) -> Val {
    match &*t.0 {
        Node::Bool(b) => Val::Bool(*b),
        Node::Int(i) => Val::Int(*i),
        Node::F32(f) => Val::F32(*f),
        Node::F64(f) => Val::F64(*f),
        Node::Var(name, sort) => env.get(name).copied().unwrap_or(Val::zero(*sort)),
        Node::App(..) => unreachable!("kein Blatt"),
    }
}

/// Terme, einmal in eine Liste uebersetzt (M11 Schritt 28): Jeder Knoten
/// hat einen Platz hinter seinen Argumenten, jede Variable einen Platz in
/// [`Plan::vars`]. Ein Lauf des Modells wertet je Tick dieselben Terme aus;
/// ueber Indizes geht das ohne Hash je Knoten. `ite`, `and` und `or` werten
/// wie [`eval`] nur, was sie brauchen.
pub struct Plan {
    items: Vec<Item>,
    args: Vec<u32>,
    /// Die Operationen und Konstanten, auf die die Knoten zeigen: Ein `Op`
    /// ist 48 Byte gross, ein Knoten so nur 16.
    ops: Vec<Op>,
    consts: Vec<Val>,
    roots: Vec<u32>,
    vars: Vec<(String, Sort)>,
    /// Werte der laufenden Auswertung; gueltig, wo `stamp` gleich `epoch`
    /// ist. So beginnt jede Auswertung ohne Loeschen.
    vals: Vec<Val>,
    stamp: Vec<u32>,
    epoch: u32,
}

#[derive(Clone, Copy)]
enum Item {
    Leaf(u32),
    Var(u32),
    App { op: u32, start: u32, len: u32 },
}

impl Plan {
    /// Uebersetzt die Terme `roots`.
    pub fn new<'a>(roots: impl IntoIterator<Item = &'a Term>) -> Plan {
        let mut items = Vec::new();
        let mut args: Vec<u32> = Vec::new();
        let mut ops: Vec<Op> = Vec::new();
        let mut op_index: HashMap<Op, u32> = HashMap::new();
        let mut consts: Vec<Val> = Vec::new();
        let mut vars: Vec<(String, Sort)> = Vec::new();
        let mut index: Keys<u32> = Keys::default();
        let mut slots: HashMap<String, u32> = HashMap::new();
        let mut tops = Vec::new();
        for root in roots {
            // Ein Knoten kommt mit `ready` zurueck, wenn seine Argumente
            // ihren Platz haben: Sie liegen ueber ihm auf dem Stapel.
            let mut stack = vec![(root.clone(), false)];
            while let Some((t, ready)) = stack.pop() {
                if index.contains_key(&t.key()) {
                    continue;
                }
                let item = match &*t.0 {
                    Node::App(_, xs) if !ready => {
                        stack.push((t.clone(), true));
                        stack.extend(xs.iter().filter(|a| !index.contains_key(&a.key())).map(|a| (a.clone(), false)));
                        continue;
                    }
                    Node::App(op, xs) => {
                        let start = args.len() as u32;
                        args.extend(xs.iter().map(|a| index[&a.key()]));
                        let next = ops.len() as u32;
                        let code = *op_index.entry(*op).or_insert(next);
                        if code == next {
                            ops.push(*op);
                        }
                        Item::App { op: code, start, len: xs.len() as u32 }
                    }
                    Node::Var(name, sort) => {
                        let next = vars.len() as u32;
                        let slot = *slots.entry(name.clone()).or_insert(next);
                        if slot == next {
                            vars.push((name.clone(), *sort));
                        }
                        Item::Var(slot)
                    }
                    Node::Bool(_) | Node::Int(_) | Node::F32(_) | Node::F64(_) => {
                        consts.push(leaf(&t, &Env::new()));
                        Item::Leaf(consts.len() as u32 - 1)
                    }
                };
                index.insert(t.key(), items.len() as u32);
                items.push(item);
            }
            tops.push(index[&root.key()]);
        }
        let n = items.len();
        Plan {
            items,
            args,
            ops,
            consts,
            roots: tops,
            vars,
            vals: vec![Val::Bool(false); n],
            stamp: vec![0; n],
            epoch: 0,
        }
    }

    /// Die Variablen der Terme in der Folge ihrer Plaetze.
    pub fn vars(&self) -> &[(String, Sort)] {
        &self.vars
    }

    /// Wertet alle Wurzeln; `vars` nennt je Variable ihren Wert in der Folge
    /// von [`Plan::vars`].
    pub fn eval(&mut self, vars: &[Val]) -> Vec<Val> {
        self.epoch = self.epoch.wrapping_add(1);
        if self.epoch == 0 {
            self.stamp.fill(0);
            self.epoch = 1;
        }
        let mut stack = Vec::new();
        for i in 0..self.roots.len() {
            self.force(self.roots[i], vars, &mut stack);
        }
        self.roots.iter().map(|&r| self.vals[r as usize]).collect()
    }

    /// Ein Stapel statt Rekursion, wie [`eval`]: Ein Knoten, dem Argumente
    /// fehlen, legt sich unter sie zurueck auf den Stapel.
    fn force(&mut self, root: u32, vars: &[Val], stack: &mut Vec<(u32, u32)>) {
        stack.push((root, 0));
        while let Some((n, from)) = stack.pop() {
            let i = n as usize;
            if self.stamp[i] == self.epoch {
                continue;
            }
            let (stamp, vals, epoch) = (&self.stamp, &self.vals, self.epoch);
            let known = |a: u32| (stamp[a as usize] == epoch).then(|| vals[a as usize]);
            let v = match self.items[i] {
                Item::Leaf(c) => self.consts[c as usize],
                Item::Var(slot) => vars[slot as usize],
                Item::App { op, start, len } => {
                    let op = self.ops[op as usize];
                    let args = &self.args[start as usize..(start + len) as usize];
                    match op {
                        Op::Ite => {
                            let Some(c) = known(args[0]) else {
                                stack.extend([(n, 0), (args[0], 0)]);
                                continue;
                            };
                            let pick = args[if c.as_bool() { 1 } else { 2 }];
                            match known(pick) {
                                Some(v) => v,
                                None => {
                                    stack.extend([(n, 0), (pick, 0)]);
                                    continue;
                                }
                            }
                        }
                        Op::And | Op::Or => {
                            let decisive = matches!(op, Op::Or);
                            let mut open = None;
                            let mut v = Val::Bool(!decisive);
                            for (j, &a) in args.iter().enumerate().skip(from as usize) {
                                match known(a) {
                                    Some(x) if x.as_bool() == decisive => {
                                        v = Val::Bool(decisive);
                                        break;
                                    }
                                    Some(_) => {}
                                    None => {
                                        open = Some((j as u32, a));
                                        break;
                                    }
                                }
                            }
                            if let Some((j, a)) = open {
                                stack.extend([(n, j), (a, 0)]);
                                continue;
                            }
                            v
                        }
                        _ if args.iter().any(|&a| known(a).is_none()) => {
                            stack.push((n, 0));
                            stack.extend(args.iter().filter(|&&a| known(a).is_none()).map(|&a| (a, 0)));
                            continue;
                        }
                        _ => apply(op, args.len(), |k| vals[args[k] as usize]),
                    }
                }
            };
            self.vals[i] = v;
            self.stamp[i] = self.epoch;
        }
    }
}

/// Eine Operation ueber ihren bekannten Argumenten.
fn apply(op: Op, n: usize, a: impl Fn(usize) -> Val) -> Val {
    match op {
        Op::Ite | Op::And | Op::Or => unreachable!("im Stapel ausgewertet"),
        Op::Not => Val::Bool(!a(0).as_bool()),
        Op::Eq => Val::Bool(same(a(0), a(1))),
        Op::Neg => Val::Int(a(0).as_int().wrapping_neg()),
        Op::Add => Val::Int(a(0).as_int().wrapping_add(a(1).as_int())),
        Op::Sub => Val::Int(a(0).as_int().wrapping_sub(a(1).as_int())),
        Op::Mul => Val::Int(a(0).as_int().wrapping_mul(a(1).as_int())),
        Op::Div => Val::Int(sdiv(a(0).as_int(), a(1).as_int())),
        Op::Rem => Val::Int(srem(a(0).as_int(), a(1).as_int())),
        Op::Lt => Val::Bool(a(0).as_int() < a(1).as_int()),
        Op::Le => Val::Bool(a(0).as_int() <= a(1).as_int()),
        Op::Gt => Val::Bool(a(0).as_int() > a(1).as_int()),
        Op::Ge => Val::Bool(a(0).as_int() >= a(1).as_int()),
        Op::BitAnd => Val::Int(a(0).as_int() & a(1).as_int()),
        Op::BitOr => Val::Int(a(0).as_int() | a(1).as_int()),
        Op::BitXor => Val::Int(a(0).as_int() ^ a(1).as_int()),
        // Wie `bvshl`/`bvashr`: Der Betrag zaehlt ohne Vorzeichen, ab
        // 64 ist alles hinausgeschoben.
        Op::Shl => {
            let (x, n) = (a(0).as_int(), a(1).as_int() as u64);
            Val::Int(if n >= 64 { 0 } else { x << n })
        }
        Op::Shr => {
            let (x, n) = (a(0).as_int(), a(1).as_int() as u64);
            Val::Int(if n >= 64 { x >> 63 } else { x >> n })
        }
        Op::ULt => Val::Bool((a(0).as_int() as u64) < a(1).as_int() as u64),
        Op::ULe => Val::Bool(a(0).as_int() as u64 <= a(1).as_int() as u64),
        Op::UGt => Val::Bool(a(0).as_int() as u64 > a(1).as_int() as u64),
        Op::UGe => Val::Bool(a(0).as_int() as u64 >= a(1).as_int() as u64),
        // Wie `bvudiv`/`bvurem`: durch null alle Bits gesetzt, der Rest der Dividend.
        Op::UDiv => {
            let (x, y) = (a(0).as_int() as u64, a(1).as_int() as u64);
            Val::Int(x.checked_div(y).unwrap_or(u64::MAX) as i64)
        }
        Op::URem => {
            let (x, y) = (a(0).as_int() as u64, a(1).as_int() as u64);
            Val::Int(x.checked_rem(y).unwrap_or(x) as i64)
        }
        Op::LShr => {
            let (x, n) = (a(0).as_int() as u64, a(1).as_int() as u64);
            Val::Int(if n >= 64 { 0 } else { (x >> n) as i64 })
        }
        Op::FNeg => fp1(a(0), |x| -x, |x| -x),
        Op::FAbs => fp1(a(0), f64::abs, f32::abs),
        Op::FSqrt => fp1(a(0), f64::sqrt, f32::sqrt),
        Op::FAdd => fp2(a(0), a(1), |x, y| x + y, |x, y| x + y),
        Op::FSub => fp2(a(0), a(1), |x, y| x - y, |x, y| x - y),
        Op::FMul => fp2(a(0), a(1), |x, y| x * y, |x, y| x * y),
        Op::FDiv => fp2(a(0), a(1), |x, y| x / y, |x, y| x / y),
        Op::FFma => match (a(0), a(1), a(2)) {
            (Val::F32(x), Val::F32(y), Val::F32(z)) => Val::F32(x.mul_add(y, z)),
            (x, y, z) => Val::F64(x.as_f64().mul_add(y.as_f64(), z.as_f64())),
        },
        Op::FLt => Val::Bool(a(0).as_f64() < a(1).as_f64()),
        Op::FLe => Val::Bool(a(0).as_f64() <= a(1).as_f64()),
        Op::FGt => Val::Bool(a(0).as_f64() > a(1).as_f64()),
        Op::FGe => Val::Bool(a(0).as_f64() >= a(1).as_f64()),
        Op::FEq => Val::Bool(a(0).as_f64() == a(1).as_f64()),
        Op::ToF32 => Val::F32(match a(0) {
            Val::F64(x) => x as f32,
            Val::F32(x) => x,
            x => x.as_int() as f32,
        }),
        Op::ToF64 => Val::F64(match a(0) {
            Val::F64(x) => x,
            Val::F32(x) => f64::from(x),
            x => x.as_int() as f64,
        }),
        Op::UToF32 => Val::F32(a(0).as_int() as u64 as f32),
        Op::UToF64 => Val::F64(a(0).as_int() as u64 as f64),
        Op::IsFinite => Val::Bool(a(0).as_f64().is_finite()),
        Op::Wrap { bits, signed } => Val::Int(wrap(a(0).as_int(), bits, signed)),
        Op::AddOverflows => Val::Bool(a(0).as_int().checked_add(a(1).as_int()).is_none()),
        Op::SubOverflows => Val::Bool(a(0).as_int().checked_sub(a(1).as_int()).is_none()),
        Op::MulOverflows => Val::Bool(a(0).as_int().checked_mul(a(1).as_int()).is_none()),
        Op::UMulOverflows => Val::Bool((a(0).as_int() as u64).checked_mul(a(1).as_int() as u64).is_none()),
        Op::Scale { num, den } => match a(0) {
            Val::F32(x) => Val::F32(libtaktm::scale_f32(x, num, den)),
            x => Val::F64(libtaktm::scale_f64(x.as_f64(), num, den)),
        },
        Op::Math(f) => {
            let x = a(0);
            let y = if n > 1 { a(1) } else { Val::F64(0.0) };
            math(f, x, y)
        }
        Op::Round(r) => rounded(r, a(0)),
        Op::Native { f, part, .. } => native(f, part, &(0..n).map(a).collect::<Vec<_>>()),
        Op::Mat { f, n: rows, k, part } => matrix(f, rows, k, part, &(0..n).map(a).collect::<Vec<_>>()),
        Op::FloatToInt => Val::Int(a(0).as_f64() as i64),
    }
}

/// Teil `part` des Ergebnisses einer Native, wie der Interpreter sie ruft
/// (`call::call_native`): die Bloecke `[Kapazitaet, Laenge, Byte …]` als
/// Bytes der Grenze.
fn native(f: takt_native::Native, part: u16, args: &[Val]) -> Val {
    use takt_native::Native;
    use takt_native::sha256::Ctx;
    if f == Native::Fft256 {
        return fft_part(part, args);
    }
    let mut blocks: Vec<Vec<u8>> = Vec::new();
    let mut rest = args;
    while let [cap, len, tail @ ..] = rest {
        let cap = usize::try_from(cap.as_int()).unwrap_or(0).min(tail.len());
        let len = usize::try_from(len.as_int()).unwrap_or(0).min(cap);
        blocks.push(tail[..len].iter().map(|b| b.as_int() as u8).collect());
        rest = &tail[cap..];
    }
    let inputs: Vec<&[u8]> = blocks.iter().map(Vec::as_slice).collect();
    let block = |i: usize| inputs.get(i).copied().unwrap_or(&[]);
    // Jeder Wert des Records ist ein Kontext (`Ctx::from_bytes`).
    let ctx = || Ctx::from_bytes(block(0)).unwrap_or_default();
    let byte = |d: [u8; 32]| d.get(usize::from(part)).copied().map_or(0, i64::from);
    Val::Int(match f {
        Native::Sha256Init => ctx_leaf(&Ctx::new(), part),
        Native::Sha256Update => {
            let mut c = ctx();
            c.update(block(1));
            ctx_leaf(&c, part)
        }
        Native::Sha256Final => byte(ctx().finish()),
        Native::EcdsaP256Verify => {
            let (Ok(key), Ok(digest), Ok(sig)) =
                (<[u8; 64]>::try_from(block(0)), <[u8; 32]>::try_from(block(1)), <[u8; 64]>::try_from(block(2)))
            else {
                return Val::Int(0);
            };
            i64::from(takt_crypto::ecdsa_p256_verify(&key, &digest, &sig).unwrap_or(false))
        }
        Native::Rsa3072Verify => i64::from(takt_crypto::rsa3072_verify(block(0), block(1), block(2)).unwrap_or(false)),
        // Teil 0: ob der Tag nicht passt (`Err(FAILED)`), dann der Klartext.
        Native::AesGcmDecrypt => {
            let data = block(3);
            let mut out = vec![0u8; data.len()];
            let opened = takt_crypto::aes_gcm_decrypt(block(0), block(1), block(2), data, block(4), &mut out);
            match (usize::from(part), opened.ok().flatten()) {
                (0, opened) => i64::from(opened.is_none()),
                (1, Some(n)) => n as i64,
                (k, Some(n)) if k >= 2 && k - 2 < n => i64::from(out[k - 2]),
                _ => 0,
            }
        }
        _ => match takt_native::call(f, &inputs) {
            Some(takt_native::Output::Scalar(raw)) => raw as i64,
            Some(takt_native::Output::Digest(d)) => byte(d),
            _ => 0,
        },
    })
}

/// Wert `part` von `fft256` (4.5) wie im Interpreter: die Argumente in
/// kanonischer Form, das Ergebnis in der Breite der Argumente.
fn fft_part(part: u16, args: &[Val]) -> Val {
    let mut input = Vec::new();
    for a in args {
        match a {
            Val::F32(x) => input.extend(x.to_le_bytes()),
            Val::F64(x) => input.extend(x.to_le_bytes()),
            _ => {}
        }
    }
    let out = match takt_native::call(takt_native::Native::Fft256, &[&input]) {
        Some(takt_native::Output::Floats { bytes, len }) => bytes[..len].to_vec(),
        _ => Vec::new(),
    };
    let k = usize::from(part);
    match args.first() {
        Some(Val::F32(_)) => Val::F32(
            out.get(4 * k..4 * k + 4).map_or(0.0, |b| f32::from_le_bytes(<[u8; 4]>::try_from(b).unwrap_or_default())),
        ),
        _ => Val::F64(
            out.get(8 * k..8 * k + 8).map_or(0.0, |b| f64::from_le_bytes(<[u8; 8]>::try_from(b).unwrap_or_default())),
        ),
    }
}

/// Blatt `part` eines `Sha256Ctx` (Prelude: `h : [8] u32`, `buf : bytes<64>`,
/// `total : u64`) in der Reihenfolge des Modells, gelesen aus der
/// kanonischen Form: die acht Woerter, die Laenge des Puffers, seine 64
/// Plaetze, hinter der Laenge null, und die Gesamtlaenge.
fn ctx_leaf(ctx: &takt_native::sha256::Ctx, part: u16) -> i64 {
    let mut out = [0u8; takt_native::sha256::CTX_MAX_BYTES];
    let len = ctx.to_bytes(&mut out).unwrap_or(0);
    let b = &out[..len];
    let le = |at: usize, n: usize| {
        b.get(at..at + n).map_or(0, |w| w.iter().rev().fold(0u64, |x, &y| (x << 8) | u64::from(y)))
    };
    let filled = le(32, 4) as usize;
    let k = usize::from(part);
    (match k {
        0..=7 => le(4 * k, 4),
        8 => filled as u64,
        9..=72 if k - 9 < filled => le(36 + k - 9, 1),
        73 => le(36 + filled, 8),
        _ => 0,
    }) as i64
}

/// Teil `part` des Ergebnisses einer Matrixfunktion, wie der Interpreter
/// sie ruft (`matrix::op`), in der Breite ihrer Argumente.
fn matrix(f: crate::term::MatFun, n: u8, k: u8, part: u16, args: &[Val]) -> Val {
    if let Some(Val::F32(_)) = args.first() {
        let x: Vec<f32> = args.iter().map(|v| if let Val::F32(f) = v { *f } else { 0.0 }).collect();
        let (items, flag) = matrix_parts(f, usize::from(n), usize::from(k), &x);
        return items.get(usize::from(part)).map_or(Val::Bool(flag), |v| Val::F32(*v));
    }
    let x: Vec<f64> = args.iter().map(|v| v.as_f64()).collect();
    let (items, flag) = matrix_parts(f, usize::from(n), usize::from(k), &x);
    items.get(usize::from(part)).map_or(Val::Bool(flag), |v| Val::F64(*v))
}

/// Die Elemente und der Wahrheitswert einer Matrixfunktion; ein
/// Ergebnis, das es nicht gibt, ist null.
fn matrix_parts<S: libtaktm::mat::Scalar>(f: crate::term::MatFun, n: usize, k: usize, x: &[S]) -> (Vec<S>, bool) {
    use crate::term::MatFun;
    use libtaktm::mat;
    let (a, b) = x.split_at((n * n).min(x.len()));
    let (mut scratch, mut perm) = (vec![S::ZERO; mat::scratch_len(n)], vec![0usize; n]);
    match f {
        MatFun::Det => (vec![mat::det(a, n, &mut scratch, &mut perm)], false),
        MatFun::Inv => {
            let mut out = vec![S::ZERO; n * n];
            let singular = mat::inv(a, n, &mut out, &mut scratch, &mut perm).is_err();
            (if singular { vec![S::ZERO; n * n] } else { out }, singular)
        }
        MatFun::Solve => {
            let mut out = vec![S::ZERO; n * k];
            let singular = mat::solve(a, n, b, k, &mut out, &mut scratch, &mut perm).is_err();
            (if singular { vec![S::ZERO; n * k] } else { out }, singular)
        }
        MatFun::Cholesky => {
            let mut out = vec![S::ZERO; n * n];
            let ok = mat::cholesky(a, n, &mut out).is_some();
            (if ok { out } else { vec![S::ZERO; n * n] }, ok)
        }
    }
}

/// Auf eine ganze Zahl gerundet, in der Breite des Werts.
fn rounded(r: Rounding, v: Val) -> Val {
    match (r, v) {
        (Rounding::HalfAway, Val::F32(x)) => Val::F32(x.round()),
        (Rounding::Down, Val::F32(x)) => Val::F32(x.floor()),
        (Rounding::Up, Val::F32(x)) => Val::F32(x.ceil()),
        (Rounding::TowardZero, Val::F32(x)) => Val::F32(x.trunc()),
        (Rounding::HalfAway, x) => Val::F64(x.as_f64().round()),
        (Rounding::Down, x) => Val::F64(x.as_f64().floor()),
        (Rounding::Up, x) => Val::F64(x.as_f64().ceil()),
        (Rounding::TowardZero, x) => Val::F64(x.as_f64().trunc()),
    }
}

/// Eine Funktion aus `libtaktm` in der Breite ihres Arguments, wie der
/// Interpreter sie ruft (`call::math_f64`, `math_f32`).
fn math(f: Fun, x: Val, y: Val) -> Val {
    match x {
        Val::F32(x) => {
            let y = match y {
                Val::F32(y) => y,
                other => other.as_f64() as f32,
            };
            Val::F32(match f {
                Fun::Sin => libtaktm::sin_f32(x),
                Fun::Cos => libtaktm::cos_f32(x),
                Fun::Tan => libtaktm::tan_f32(x),
                Fun::Asin => libtaktm::asin_f32(x),
                Fun::Acos => libtaktm::acos_f32(x),
                Fun::Atan => libtaktm::atan_f32(x),
                Fun::Atan2 => libtaktm::atan2_f32(x, y),
                Fun::Exp => libtaktm::exp_f32(x),
                Fun::Log => libtaktm::log_f32(x),
                Fun::Pow => libtaktm::pow_f32(x, y),
            })
        }
        x => {
            let (x, y) = (x.as_f64(), y.as_f64());
            Val::F64(match f {
                Fun::Sin => libtaktm::sin_f64(x),
                Fun::Cos => libtaktm::cos_f64(x),
                Fun::Tan => libtaktm::tan_f64(x),
                Fun::Asin => libtaktm::asin_f64(x),
                Fun::Acos => libtaktm::acos_f64(x),
                Fun::Atan => libtaktm::atan_f64(x),
                Fun::Atan2 => libtaktm::atan2_f64(x, y),
                Fun::Exp => libtaktm::exp_f64(x),
                Fun::Log => libtaktm::log_f64(x),
                Fun::Pow => libtaktm::pow_f64(x, y),
            })
        }
    }
}

/// `bvsdiv`: durch null `-1` fuer nichtnegative, `1` fuer negative
/// Dividenden; `MIN / -1` laeuft um.
fn sdiv(a: i64, b: i64) -> i64 {
    match b {
        0 if a < 0 => 1,
        0 => -1,
        _ => a.wrapping_div(b),
    }
}

/// `bvsrem`: durch null der Dividend; `MIN % -1` ist null.
fn srem(a: i64, b: i64) -> i64 {
    if b == 0 { a } else { a.wrapping_rem(b) }
}

/// Die unteren `bits` Bits, erweitert wie `Op::Wrap`.
fn wrap(x: i64, bits: u32, signed: bool) -> i64 {
    if bits >= 64 {
        return x;
    }
    let shift = 64 - bits;
    if signed { (x << shift) >> shift } else { ((x as u64) << shift >> shift) as i64 }
}

/// `=` wie in SMT-LIB: Fliesskomma gleicht aufs Bit, `+0` nicht `-0`, und
/// es gibt nur ein NaN.
fn same(a: Val, b: Val) -> bool {
    match (a, b) {
        (Val::Bool(x), Val::Bool(y)) => x == y,
        (Val::Int(x), Val::Int(y)) => x == y,
        (Val::F32(x), Val::F32(y)) => (x.is_nan() && y.is_nan()) || x.to_bits() == y.to_bits(),
        (x, y) => {
            let (x, y) = (x.as_f64(), y.as_f64());
            (x.is_nan() && y.is_nan()) || x.to_bits() == y.to_bits()
        }
    }
}

fn fp1(x: Val, f64_: fn(f64) -> f64, f32_: fn(f32) -> f32) -> Val {
    match x {
        Val::F32(v) => Val::F32(f32_(v)),
        v => Val::F64(f64_(v.as_f64())),
    }
}

fn fp2(x: Val, y: Val, f64_: fn(f64, f64) -> f64, f32_: fn(f32, f32) -> f32) -> Val {
    match (x, y) {
        (Val::F32(a), Val::F32(b)) => Val::F32(f32_(a, b)),
        (a, b) => Val::F64(f64_(a.as_f64(), b.as_f64())),
    }
}
