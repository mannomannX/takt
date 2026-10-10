//! Wie oft ein Strom je Aktivierung seines Schreibers hoechstens ein `send`
//! bekommt (8.6, Pruefung 43): MAXPT eines internen Stroms in der Sema, die
//! Elemente, die ein Sendepuffer mit Grenzen in einem Tick vollendet, im
//! Beweiser.

use crate::StreamId;
use crate::expr::{Expr, ExprKind, StreamRef};
use crate::machine::{MachineKind, SeqItem, TimeoutAction};
use crate::program::Program;
use crate::stmt::{Block, Stmt, StmtKind};
use crate::types::Type;
use crate::visit::for_each_block_in;

/// Statische Hoechstzahl von `send` auf einen Stream je Aktivierung des
/// Schreibers (8.6, Pruefung 43): eine Schleife mit ihrer Schranke, ein
/// Handler je Element seines Fensters (8.7), von zwei Zweigen der
/// teurere.
pub fn max_sends(p: &Program, target: StreamRef) -> u64 {
    max_sends_of(p, target, &mut Vec::new())
}

/// `max_sends` mit den Stroemen, deren Hoechstzahl gerade entsteht: Ein
/// Zyklus ueber Handler rechnet mit der Kapazitaet.
fn max_sends_of(p: &Program, target: StreamRef, open: &mut Vec<StreamRef>) -> u64 {
    open.push(target);
    let mut worst = 0u64;
    for m in &p.machines {
        if matches!(m.kind, MachineKind::Template) {
            continue;
        }
        let period = u64::from(m.period.max(1));
        let mut n = 0u64;
        for_each_block_in(m, &mut |b, handler| {
            if !handler {
                n = n.saturating_add(sends_in(p, &b.stmts, target, period, open));
            }
        });
        for h in m.handlers.iter().chain(m.states.iter().flat_map(|s| &s.handlers)) {
            let each = sends_in(p, &h.body.stmts, target, period, open);
            if each > 0 {
                n = n.saturating_add(stream_window(p, h.stream, period, open).saturating_mul(each));
            }
        }
        // 6.2: Eine Sequenz laeuft je Tick ein Segment; ihre `send`
        // zaehlen je Segment, nicht in der Summe (FB-186).
        for s in &m.states {
            if let Some(seq) = &s.sequence {
                let mut count = |stmts: &[Stmt]| sends_in(p, stmts, target, period, open);
                n = n.saturating_add(segment_sends(&seq.items, &mut count));
            }
        }
        worst = worst.max(n);
    }
    open.pop();
    worst.max(1)
}

/// `send` auf `target` in einem Durchlauf von `stmts` einer Maschine
/// der Periode `period`.
fn sends_in(p: &Program, stmts: &[Stmt], target: StreamRef, period: u64, open: &mut Vec<StreamRef>) -> u64 {
    let mut n = 0u64;
    for s in stmts {
        let here = match &s.kind {
            StmtKind::Send { stream, .. } => u64::from(*stream == target),
            StmtKind::If { then, otherwise, .. } => {
                let a = sends_in(p, &then.stmts, target, period, open);
                a.max(sends_in(p, &otherwise.stmts, target, period, open))
            }
            StmtKind::Match { arms, .. } => {
                let mut most = 0u64;
                for a in arms {
                    most = most.max(sends_in(p, &a.body.stmts, target, period, open));
                }
                most
            }
            // Die Sema senkt `range(N)` immer auf ein Literal (stmt.rs).
            StmtKind::ForRange { count, body, .. } => {
                let trips = match count.kind {
                    ExprKind::Int(n) => u64::try_from(n).unwrap_or(0),
                    _ => u64::MAX,
                };
                single_pass(body, trips).saturating_mul(sends_in(p, &body.stmts, target, period, open))
            }
            StmtKind::ForEach { iter, body, .. } => match sends_in(p, &body.stmts, target, period, open) {
                0 => 0,
                each => single_pass(body, iterations(p, iter, period, open)).saturating_mul(each),
            },
            StmtKind::Every { body, .. } | StmtKind::At { body, .. } => sends_in(p, &body.stmts, target, period, open),
            StmtKind::Assign { .. }
            | StmtKind::Check { .. }
            | StmtKind::Goto(_)
            | StmtKind::Abort { .. }
            | StmtKind::Return(_)
            | StmtKind::Cancel(_)
            | StmtKind::Skip(_)
            | StmtKind::Raise(_)
            | StmtKind::Job { .. }
            | StmtKind::Break
            | StmtKind::Observe(_)
            | StmtKind::Arm { .. }
            | StmtKind::MethodCall { .. }
            | StmtKind::Pass => 0,
        };
        n = n.saturating_add(here);
    }
    n
}

/// Durchlaeufe von `for x in iter`: das Fenster eines Stroms, sonst die
/// Kapazitaet des Behaelters (9.4.3).
fn iterations(p: &Program, iter: &Expr, period: u64, open: &mut Vec<StreamRef>) -> u64 {
    let stream = match &iter.kind {
        ExprKind::Stream(s) => Some(StreamRef::Internal(*s)),
        ExprKind::Input { channel, .. } => Some(StreamRef::Channel(*channel)),
        ExprKind::Var(v) => Some(StreamRef::Var(*v)),
        _ => None,
    };
    match (p.types.get(iter.ty), stream) {
        (Type::Stream(_), Some(s)) => stream_window(p, s, period, open),
        (Type::Array { len, .. } | Type::Samples { len, .. }, _) => u64::from(*len),
        (
            Type::Vec { cap, .. }
            | Type::Bytes { cap }
            | Type::Map { cap, .. }
            | Type::Str { cap }
            | Type::Line { cap },
            _,
        ) => u64::from(*cap),
        _ => u64::MAX,
    }
}

/// Hoechstens so viele Elemente sieht ein Konsument der Periode
/// `period` in seinem Fenster: die Kapazitaet, und nach Lemma 9.6.1
/// nicht mehr, als zwischen zwei Aktivierungen eintreffen
/// (`MAXPT_s * n_m`) — ein Ueberlauf darueber ist der Fault des Stroms.
pub fn stream_window(p: &Program, s: StreamRef, period: u64, open: &mut Vec<StreamRef>) -> u64 {
    let tick = u64::try_from(p.config.tick).unwrap_or(0);
    let internal = |i: StreamId| p.streams.get(i.index()).map_or(u64::MAX, |d| u64::from(d.capacity));
    let (capacity, maxpt) = match s {
        StreamRef::Channel(c) => {
            let cap = p.channels.get(c.index()).and_then(|c| c.attrs.capacity).map_or(u64::MAX, u64::from);
            let rate = rate_hz(p, c.index()).map(|r| ceil_div(r.saturating_mul(tick), 1_000_000_000).max(1));
            (cap, rate)
        }
        StreamRef::Internal(i) => {
            let sends = (!open.contains(&s)).then(|| max_sends_of(p, s, open));
            (internal(i), sends)
        }
        StreamRef::Fired(t) => (p.triggers.get(t.index()).map_or(u64::MAX, |d| internal(d.fired)), None),
        // Ein Strom als Parameter: hoechstens das groesste Fenster.
        StreamRef::Var(_) => {
            let channels = p.channels.iter().filter_map(|c| c.attrs.capacity).map(u64::from);
            (channels.chain(p.streams.iter().map(|d| u64::from(d.capacity))).max().unwrap_or(u64::MAX), None)
        }
    };
    maxpt.map_or(capacity, |m| capacity.min(m.saturating_mul(period)))
}

/// Das Maximum der `send` eines Segments (6.2): Grenzen sind `wait`,
/// `until`, eine Anweisung mit `->` und das Ende eines `repeat`-Koerpers.
fn segment_sends(items: &[SeqItem], count: &mut dyn FnMut(&[Stmt]) -> u64) -> u64 {
    let (mut best, mut cur) = (0u64, 0u64);
    for item in items {
        match item {
            SeqItem::Stmt(s) => {
                cur += count(std::slice::from_ref(s));
                if Block::new(vec![s.clone()]).has_goto() {
                    best = best.max(cur);
                    cur = 0;
                }
            }
            SeqItem::Wait(_) => {
                best = best.max(cur);
                cur = 0;
            }
            SeqItem::Until { timeout, .. } => {
                best = best.max(cur);
                cur = 0;
                if let Some(TimeoutAction::Else(b)) = timeout.as_ref().map(|t| &t.action) {
                    best = best.max(count(&b.stmts));
                }
            }
            SeqItem::Expect { .. } => {}
            SeqItem::Repeat { body, .. } | SeqItem::Step { body, .. } => {
                best = best.max(cur).max(segment_sends(body, count));
                cur = 0;
            }
        }
    }
    best.max(cur)
}

/// Durchlaeufe einer Schleife mit Schranke `bound`: Endet ihr Rumpf mit
/// `break`, laeuft sie hoechstens einmal.
fn single_pass(body: &Block, bound: u64) -> u64 {
    if matches!(body.stmts.last().map(|s| &s.kind), Some(StmtKind::Break)) { bound.min(1) } else { bound }
}

/// `max_rate` eines Channels in Hz, als ganze Zahl.
fn rate_hz(p: &Program, index: usize) -> Option<u64> {
    let e = p.channels[index].attrs.max_rate.as_ref()?;
    match &e.kind {
        ExprKind::Int(n) => u64::try_from(*n).ok(),
        ExprKind::Float(f) if *f >= 0.0 => Some(*f as u64),
        _ => None,
    }
}

/// Aufrundende Division fuer positive Nenner.
fn ceil_div(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { a.div_ceil(b) }
}
