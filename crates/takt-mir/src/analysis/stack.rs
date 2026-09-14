//! Die Stacktiefe als laengster Pfad im Aufrufgraphen (12.3).
//!
//! 12.3 gibt vier Summanden vor: „Programmanteil exakt aus LLVM-Stack-Usage
//! und azyklischem Aufrufgraphen; native Funktionen mit ihrem
//! `stack`-Vertrag (4.5) als Blattkosten; Runtime und Treiber, ISRs … als
//! Reserven je Profil aus der Hardware-Konfiguration, gemessen in 13.8;
//! Gesamt = Programm + Σ Reserven + Marge."
//!
//! Hier entsteht der **erste** Summand. Die Reserven kommen aus der Messung
//! (13.8), die Marge aus der Hardware-Konfiguration (8.10, 12.3); beide
//! gehoeren neben die Rechnung, nicht hinein.
//!
//! **Warum gerechnet und nicht gemessen.** Ein Stack-Painting-Lauf liefert
//! schnell eine Zahl, aber die falsche: Gemessen wird, was ein Lauf
//! beruehrt *hat*, nicht was er beruehren *kann*. Fuer eine Schranke taugt
//! nur die Rechnung — und weil der Aufrufgraph azyklisch ist (Pruefung 11),
//! ist der laengste Pfad endlich und in einem Durchlauf zu haben. Die
//! Messung bleibt die Gegenprobe: Liegt die gemessene Spitze ueber der
//! gerechneten Schranke, ist die Rechnung falsch — nicht die Messung zu
//! klein (plan/m5.md 2.3).
//!
//! **Woher die Rahmengroesse kommt.** Aus dem erzeugten Objekt, nicht aus
//! der MIR: Wie viele Bytes eine Funktion auf dem Stack nimmt, entscheidet
//! der Codegen. `takt_llvm::inspect::Binutils::stack_frame` liest sie je
//! Funktion aus dem Prolog; hier werden sie zusammengesetzt. Ohne Objekt
//! bleibt die Tiefe unbekannt — wie der Flash-Posten in 11.5, und aus
//! demselben Grund.

use crate::expr::{Expr, ExprKind};
use crate::machine::{Machine, SeqItem};
use crate::stmt::{Block, Place, Stmt, StmtKind};
use crate::{FnId, Program};

/// Der Programmanteil des Stacks (12.3), in Byte.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Depth {
    /// Der laengste Pfad ab dem teuersten Einstiegspunkt.
    pub bytes: u64,
    /// Die Funktionen dieses Pfades, von der Wurzel zum Blatt.
    ///
    /// Sie stehen im Bericht, weil eine Zahl allein nicht sagt, wo man
    /// ansetzt: Wer den Stack verkleinern will, braucht die Kette.
    pub path: Vec<String>,
}

/// Rahmengroessen je Funktion, indiziert wie [`Program::fns`].
///
/// `None` heisst „nicht gemessen". Eine erreichbare Funktion ohne Messung
/// macht die Rechnung unbekannt: Eine Schranke mit einer Luecke waere
/// keine.
pub type Frames = [Option<u32>];

/// Rechnet den Programmanteil des Stacks (12.3).
///
/// `None`, wenn eine erreichbare Funktion keinen gemessenen Rahmen hat
/// oder das Programm keine Funktion ruft.
pub fn depth(p: &Program, frames: &Frames) -> Option<Depth> {
    let n = p.fns.len();
    if frames.len() < n {
        return None;
    }
    // Memoisierung ueber den azyklischen Graphen (Pruefung 11): Jede
    // Funktion wird einmal gerechnet, auch wenn viele Pfade zu ihr fuehren.
    let mut best: Vec<Option<(u64, Vec<FnId>)>> = vec![None; n];
    for i in 0..n {
        resolve(p, frames, FnId(i as u32), &mut best, 0)?;
    }

    // Einstieg ist jede Funktion, die eine Maschine ruft. Nicht „was
    // niemand ruft": Eine Hilfsfunktion kann von einer Maschine *und* von
    // einer anderen Funktion gerufen werden, und dann zaehlt der laengere
    // der beiden Pfade.
    let (bytes, path) = entry_points(p)
        .iter()
        .filter_map(|f| best[f.index()].as_ref())
        .max_by_key(|(b, _)| *b)
        .map(|(b, path)| (*b, path.clone()))?;

    Some(Depth { bytes, path: path.iter().map(|f| p.fns[f.index()].name.clone()).collect() })
}

/// Der teuerste Pfad ab `f`, memoisiert.
///
/// `guard` begrenzt die Rekursion auf die Zahl der Funktionen. Ein Zyklus
/// kann nicht auftreten — Pruefung 11 hat ihn abgelehnt —, aber eine
/// verletzte Vorbedingung soll den Compiler nicht in eine Endlosschleife
/// fuehren, sondern die Rechnung als unbekannt melden.
fn resolve(
    p: &Program,
    frames: &Frames,
    f: FnId,
    best: &mut Vec<Option<(u64, Vec<FnId>)>>,
    guard: usize,
) -> Option<()> {
    if best[f.index()].is_some() {
        return Some(());
    }
    if guard > p.fns.len() {
        return None;
    }
    let own = u64::from(frames.get(f.index()).copied().flatten()?);

    let mut calls = Calls::default();
    calls.block(&p.fns[f.index()].body);

    let mut deepest: (u64, Vec<FnId>) = (0, Vec::new());
    for callee in calls.fns {
        resolve(p, frames, callee, best, guard + 1)?;
        if let Some((b, path)) = &best[callee.index()]
            && *b > deepest.0
        {
            deepest = (*b, path.clone());
        }
    }
    // Native Funktionen sind Blaetter mit deklariertem Vertrag (4.5): Sie
    // rufen nichts Sichtbares, und ihr Bedarf steht in der Deklaration
    // statt im Objekt.
    for id in calls.natives {
        let b = p.natives.get(id).map_or(0, |n| u64::from(n.stack));
        if b > deepest.0 {
            deepest = (b, Vec::new());
        }
    }

    let mut path = deepest.1;
    path.insert(0, f);
    best[f.index()] = Some((own + deepest.0, path));
    Some(())
}

/// Funktionen, die von einer Maschine aus gerufen werden.
fn entry_points(p: &Program) -> Vec<FnId> {
    let mut calls = Calls::default();
    for m in &p.machines {
        calls.machine(m);
    }
    calls.fns
}

/// Sammelt die Aufrufe eines Baumstuecks.
///
/// Der Durchlauf ist ausgeschrieben statt generisch — wie in `cost.rs`,
/// und aus demselben Grund: Eine neue `StmtKind`-Variante soll die
/// Kompilierung brechen und nicht still uebersprungen werden.
#[derive(Default)]
struct Calls {
    fns: Vec<FnId>,
    natives: Vec<usize>,
}

impl Calls {
    fn machine(&mut self, m: &Machine) {
        self.block(&m.loop_block);
        for h in &m.handlers {
            self.block(&h.body);
        }
        for t in &m.faulted.transitions {
            self.block(&t.actions);
        }
        for s in &m.states {
            self.block(&s.enter);
            self.block(&s.loop_block);
            self.block(&s.exit);
            for h in &s.handlers {
                self.block(&h.body);
            }
            for t in &s.transitions {
                self.block(&t.actions);
            }
            if let Some(seq) = &s.sequence {
                for item in &seq.items {
                    if let SeqItem::Stmt(st) = item {
                        self.stmt(st);
                    }
                }
            }
        }
        self.dedup();
    }

    fn block(&mut self, b: &Block) {
        for s in &b.stmts {
            self.stmt(s);
        }
    }

    fn stmt(&mut self, s: &Stmt) {
        match &s.kind {
            StmtKind::Assign { target, value } => {
                self.place(target);
                self.expr(value);
            }
            StmtKind::Check { cond, .. } => self.expr(cond),
            StmtKind::If { cond, then, otherwise } => {
                self.expr(cond);
                self.block(then);
                self.block(otherwise);
            }
            StmtKind::ForRange { count, body, .. } => {
                self.expr(count);
                self.block(body);
            }
            StmtKind::ForEach { iter, body, .. } => {
                self.expr(iter);
                self.block(body);
            }
            StmtKind::Match { subject, arms } => {
                self.expr(subject);
                for a in arms {
                    self.block(&a.body);
                }
            }
            StmtKind::Every { period, body, .. } => {
                self.expr(period);
                self.block(body);
            }
            StmtKind::At { time, body } => {
                self.expr(time);
                self.block(body);
            }
            StmtKind::Send { value, .. } | StmtKind::Return(value) => self.expr(value),
            StmtKind::MethodCall { target, args, .. } => {
                if let Some(t) = target {
                    self.place(t);
                }
                for a in args {
                    self.expr(a);
                }
            }
            StmtKind::Job { args, native, .. } => {
                for a in args {
                    self.expr(a);
                }
                self.natives.push(native.index());
            }
            StmtKind::Observe(o) => self.observe(o),
            _ => {}
        }
    }

    fn observe(&mut self, o: &crate::stmt::Observe) {
        use crate::stmt::Observe;
        match o {
            Observe::Alert { cond, .. } | Observe::Verify { cond, .. } => self.expr(cond),
            Observe::Measure { value, .. } => self.expr(value),
            _ => {}
        }
    }

    fn place(&mut self, p: &Place) {
        match p {
            Place::Var(_) | Place::Output(_) => {}
            Place::Field(b, _) => self.place(b),
            Place::Index(b, i) => {
                self.place(b);
                self.expr(i);
            }
            Place::Index2(b, r, c) => {
                self.place(b);
                self.expr(r);
                self.expr(c);
            }
        }
    }

    fn expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Call { callee, .. } => self.fns.push(*callee),
            ExprKind::NativeCall { native, .. } => self.natives.push(native.index()),
            _ => {}
        }
        for child in e.children() {
            self.expr(child);
        }
    }

    fn dedup(&mut self) {
        self.fns.sort_unstable();
        self.fns.dedup();
        self.natives.sort_unstable();
        self.natives.dedup();
    }
}
