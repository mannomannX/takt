//! Was der Codegen deckt — als Messung, nicht als Schaetzung.
//!
//! Die Grammatik (`grammar/takt.ebnf`) sagt, was die Sprache kann.
//! Produktionen zu zaehlen waere aber irrefuehrend: Es wiegt eine
//! Randnotiz so schwer wie eine Zuweisung, und ein gutes Drittel der
//! Produktionen ist Deklaration, aus der nie IR entsteht.
//!
//! Diese Messung geht statt dessen ueber die MIR echter Programme und
//! fragt jeden Knoten, ob der Codegen ihn kennt. Das Ergebnis ist eine
//! Aussage ueber den Weg, der noch fehlt — gewichtet danach, wie oft er
//! wirklich vorkommt.
//!
//! Sie ist bewusst *unabhaengig* vom Codegen geschrieben: Sie ruft ihn
//! nicht auf, sondern fuehrt eine eigene Liste. Damit faellt auf, wenn
//! beide auseinanderlaufen — ein Test haelt sie in Deckung.

use std::collections::BTreeMap;

use takt_mir::expr::{Accessor, Expr, ExprKind};
use takt_mir::machine::{Guard, Machine, TransTrigger};
use takt_mir::stmt::{Block, Place, Stmt, StmtKind};

/// Wie oft ein Knoten vorkommt, getrennt nach gedeckt und offen.
#[derive(Debug, Default)]
pub struct Coverage {
    /// Knoten, die der Codegen senkt.
    pub covered: BTreeMap<String, usize>,
    /// Knoten, die er noch meldet.
    pub open: BTreeMap<String, usize>,
}

impl Coverage {
    /// Wie viele Knoten insgesamt gesehen wurden.
    pub fn total(&self) -> usize {
        self.covered.values().sum::<usize>() + self.open.values().sum::<usize>()
    }

    /// Anteil gedeckter Knoten in Prozent.
    pub fn percent(&self) -> f64 {
        let n = self.total();
        if n == 0 {
            return 0.0;
        }
        100.0 * self.covered.values().sum::<usize>() as f64 / n as f64
    }

    fn note(&mut self, name: &str, covered: bool) {
        let map = if covered { &mut self.covered } else { &mut self.open };
        *map.entry(name.to_string()).or_default() += 1;
    }
}

/// Zaehlt die Knoten einer Maschine.
///
/// Jeder Block genau einmal: Der `loop:` einer Zwischenebene laeuft im
/// erzeugten Code zwar je Blatt, steht aber einmal im Programm, und
/// gezaehlt wird, was zu senken ist.
pub fn machine(m: &Machine, c: &mut Coverage) {
    block(&m.loop_block, c);
    for s in &m.states {
        block(&s.enter, c);
        block(&s.loop_block, c);
        block(&s.exit, c);
        if !s.enter.stmts.is_empty() || !s.exit.stmts.is_empty() {
            // Seit den Uebergaengen laufen sie: `exit:` des verlassenen,
            // `enter:` des betretenen Zustands (5.2); der Anfangszustand
            // bekommt sein `enter:` aus `init_function` (9.4).
            c.note("`enter:`/`exit:`", true);
        }
        for t in &s.transitions {
            block(&t.actions, c);
            match &t.trigger {
                TransTrigger::When(Guard::Expr(e)) => {
                    c.note("Uebergang `when`", true);
                    expr(e, c);
                }
                TransTrigger::When(_) => c.note("Uebergang mit Muster-Guard", false),
                TransTrigger::After(d) => {
                    // Eine berechnete Dauer haette einen Wert je Tick; 7.2
                    // koennte sie nicht beschraenken.
                    let literal =
                        matches!(d.kind, takt_mir::expr::ExprKind::Duration(_) | takt_mir::expr::ExprKind::Param(_));
                    c.note("Uebergang `after d`", literal);
                }
            }
        }
        for h in &s.handlers {
            // Ein Catch-all wird gesenkt (8.7); Muster brauchen den
            // Musterabgleich ueber Stromelementen.
            c.note("`on`-Handler", h.pattern.is_none());
            block(&h.body, c);
        }
    }
    for h in &m.handlers {
        c.note("`on`-Handler", h.pattern.is_none());
        block(&h.body, c);
    }
}

/// Zaehlt die Knoten eines Blocks.
pub fn block(b: &Block, c: &mut Coverage) {
    for s in &b.stmts {
        stmt(s, c);
    }
}

fn stmt(s: &Stmt, c: &mut Coverage) {
    match &s.kind {
        StmtKind::Assign { target, value } => {
            let ok = !matches!(target, Place::Index2(..));
            c.note(if ok { "Zuweisung" } else { "Zuweisung an ein Matrixelement" }, ok);
            expr(value, c);
        }
        StmtKind::Check { cond, .. } => {
            c.note("`check`", true);
            expr(cond, c);
        }
        StmtKind::If { cond, then, otherwise } => {
            c.note("`if`", true);
            expr(cond, c);
            block(then, c);
            block(otherwise, c);
        }
        StmtKind::ForRange { body, .. } => {
            c.note("`for i in range(n)`", false);
            block(body, c);
        }
        StmtKind::ForEach { body, .. } => {
            c.note("`for x in W`", false);
            block(body, c);
        }
        StmtKind::Abort { .. } => c.note("`abort`", true),
        StmtKind::MethodCall { method, args, .. } => {
            // `step`/`reset` einer Blockinstanz (5.7) und die
            // Sammlungsmethoden (3.9) senkt der Codegen; `insert` und
            // `remove` gehoeren zu `map` und damit zu v1.1.
            let ok = !matches!(takt_mir::stmt::Method::Insert, m2 if *method == m2)
                && !matches!(takt_mir::stmt::Method::Remove, m2 if *method == m2);
            c.note("Methode", ok);
            for a in args {
                expr(a, c);
            }
        }
        StmtKind::Observe(o) => {
            let ok = matches!(
                o,
                takt_mir::stmt::Observe::Alert { .. }
                    | takt_mir::stmt::Observe::Log(_)
                    | takt_mir::stmt::Observe::Measure { .. }
                    | takt_mir::stmt::Observe::Verify { .. }
            );
            c.note("Beobachtung (`alert`, `log`, `measure`, ...)", ok);
        }
        StmtKind::Match { arms, subject } => {
            // Varianten ohne Feldbindungen und Wertmuster werden gesenkt;
            // Bindungen brauchen den Musterabgleich ueber Summentypen.
            let ok = arms.iter().all(|a| match &a.pattern {
                takt_mir::stmt::ArmPattern::Variant { fields, .. } => fields.len() <= 1,
                _ => true,
            });
            c.note("`match`", ok);
            expr(subject, c);
            for a in arms {
                block(&a.body, c);
            }
        }
        StmtKind::Raise(_) => c.note("`raise`", true),
        other => c.note(stmt_name(other), false),
    }
}

/// Der Name einer eingebauten Groesse fuer die Meldung (3.3, 5.3).
pub fn builtin_name(b: takt_mir::expr::Builtin) -> &'static str {
    use takt_mir::expr::Builtin as B;
    match b {
        B::Now => "`now`",
        B::Tick => "`tick`",
        B::TimeInState => "`time_in_state`",
        B::LastFault => "`last_fault`",
        B::Event => "`event`",
    }
}

/// Der Name einer Anweisung fuer Meldung und Messung.
pub fn stmt_name(s: &StmtKind) -> &'static str {
    match s {
        StmtKind::Goto(_) => "`->` als Anweisung",
        StmtKind::Abort { .. } => "`abort`",
        StmtKind::Return(_) => "`return`",
        StmtKind::Send { .. } => "`send`",
        StmtKind::At { .. } => "`at`/`pulse`",
        StmtKind::Cancel(_) => "`cancel`",
        StmtKind::Skip(_) => "`skip`",
        StmtKind::Raise(_) => "`raise`",
        StmtKind::Job { .. } => "`job`",
        StmtKind::Every { .. } => "`every`",
        // `alert`, `log`, `measure`, `verify` und `verdict` sind
        // Beobachtungen: Sie aendern den Zustand nicht, sondern melden
        // (9.3). Die MIR fasst sie darum zusammen.
        StmtKind::Observe(_) => "Beobachtung (`alert`, `log`, `measure`, ...)",
        StmtKind::Arm { .. } => "`arm`/`disarm`",
        StmtKind::MethodCall { .. } => "Methode",
        _ => "weitere Anweisung",
    }
}

fn expr(e: &Expr, c: &mut Coverage) {
    match &e.kind {
        ExprKind::Bool(_) | ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Duration(_) => {
            c.note("Literal", true);
        }
        ExprKind::Var(_) => c.note("Variable", true),
        ExprKind::Input { .. } => c.note("Input", true),
        ExprKind::Param(_) => c.note("Parameter", true),
        ExprKind::Output(_) => c.note("Output-Latch", true),
        ExprKind::Command(_) => c.note("Command", true),
        ExprKind::Unary { expr: x, .. } => {
            c.note("einstelliger Operator", true);
            expr(x, c);
        }
        ExprKind::Binary { lhs, rhs, .. } => {
            c.note("zweistelliger Operator", true);
            expr(lhs, c);
            expr(rhs, c);
        }
        ExprKind::Cond { cond, then, otherwise } => {
            c.note("bedingter Ausdruck", true);
            expr(cond, c);
            expr(then, c);
            expr(otherwise, c);
        }
        ExprKind::Checked { expr: x, .. } => {
            c.note("Laufzeitpruefung", true);
            expr(x, c);
        }
        ExprKind::Accessor { base, accessor, .. } => {
            let quality = matches!(
                accessor,
                Accessor::Valid | Accessor::Suspect | Accessor::Stale | Accessor::Age | Accessor::Reason | Accessor::Or
            );
            // Die Qualitaetszugriffe gelten nur auf einem Channel: Sie
            // lesen den Eintrag im Prozessabbild (3.5).
            let on_channel = matches!(base.kind, ExprKind::Input { .. });
            // `.valid` und `.or` gelten auch auf einem Wrapper (3.8),
            // `.age`/`.reason` nur auf einem Channel — ein Wrapper hat
            // keine Herkunft.
            let auf_wrapper = matches!(accessor, Accessor::Valid | Accessor::Or | Accessor::Ok | Accessor::Err);
            let ok = matches!(
                accessor,
                Accessor::Bit | Accessor::Bits | Accessor::WithBit | Accessor::Len | Accessor::Encode
            ) || (quality && on_channel)
                || auf_wrapper;
            c.note(if ok { "Zugriff" } else { "Zugriff (`.len`, `.count`, ...)" }, ok);
            expr(base, c);
        }
        ExprKind::Field { base, .. } => {
            c.note("Feldzugriff", true);
            expr(base, c);
        }
        ExprKind::Index { base, index } => {
            c.note("Index", false);
            expr(base, c);
            expr(index, c);
        }
        ExprKind::Call { args, .. } => {
            c.note("Funktionsaufruf", true);
            for a in args {
                expr(a, c);
            }
        }
        ExprKind::Intrinsic { op, args } => {
            c.note("Primitive", true);
            // `interp` verbraucht seine Tabelle unmittelbar (3.9): Die
            // Stuetzstellen gehen in die Rechnung, nicht durch die
            // Typabbildung. Sie hier mitzuzaehlen wuerde eine Luecke
            // melden, die es nicht gibt.
            let tabelle = *op == takt_mir::expr::Intrinsic::Interp;
            for (i, a) in args.iter().enumerate() {
                if tabelle && i == 0 {
                    continue;
                }
                expr(a, c);
            }
        }
        ExprKind::Convert { expr: x, .. } => {
            c.note("Einheitenkonversion", true);
            expr(x, c);
        }
        ExprKind::Cast { expr: x, .. } => {
            c.note("`as`", true);
            expr(x, c);
        }
        ExprKind::Format(_) => c.note("Format-String", false),
        ExprKind::Builtin(_) => c.note("eingebauter Bezeichner", false),
        ExprKind::Published { .. } => c.note("Psi (`m.x`)", true),
        ExprKind::StateOf(_) => c.note("`m.state`", true),
        ExprKind::Signal { .. } => c.note("Signal", true),
        ExprKind::Record { fields, .. } => {
            c.note("Record-Literal", true);
            for f in fields {
                expr(f, c);
            }
        }
        ExprKind::Variant { fields, .. } => c.note("Variante", fields.is_empty()),
        ExprKind::Array(items) => {
            c.note("Array-Literal", true);
            for i in items {
                expr(i, c);
            }
        }
        ExprKind::Decode { bytes, .. } => {
            c.note("`decode`", true);
            expr(bytes, c);
        }
        ExprKind::Ok(v) | ExprKind::Err(v) | ExprKind::Lift(v) => {
            c.note("`ok`/`err`", true);
            expr(v, c);
        }
        ExprKind::None | ExprKind::Default => c.note("`none`/`default`", true),
        ExprKind::Matches { .. } => c.note("`matches`", false),
        ExprKind::MatOp { .. } => c.note("Matrixoperation", false),
        ExprKind::Slice { base, from, to } => {
            c.note("Teilbereich", true);
            expr(base, c);
            expr(from, c);
            expr(to, c);
        }
        ExprKind::NativeCall { .. } => c.note("native Funktion", false),
        // Eine Stuetzstelle steht nur in einer Tabelle, und `interp`
        // liest sie dort unmittelbar (3.9) — ausserhalb kommt sie nicht
        // vor.
        ExprKind::Tuple(..) => c.note("Stuetzstelle einer Tabelle", true),
        _ => c.note("weiterer Ausdruck", false),
    }
}

/// Der Name eines Zugriffs fuer Meldung und Messung (3.5, 3.9).
pub fn accessor_name(a: Accessor) -> &'static str {
    match a {
        Accessor::Valid => "`.valid`",
        Accessor::Suspect => "`.suspect`",
        Accessor::Stale => "`.stale`",
        Accessor::Age => "`.age`",
        Accessor::Reason => "`.reason`",
        Accessor::Or => "`.or`",
        Accessor::Ok => "`.ok`",
        Accessor::Err => "`.err`",
        Accessor::T => "`.t`",
        Accessor::Seq => "`.seq`",
        Accessor::Text => "`.text`",
        Accessor::Data => "`.data`",
        Accessor::Len => "`.len`",
        Accessor::Count => "`.count`",
        Accessor::Dropped => "`.dropped`",
        Accessor::Malformed => "`.malformed`",
        Accessor::Overflowed => "`.overflowed`",
        Accessor::Free => "`.free`",
        Accessor::Jitter => "`.jitter`",
        Accessor::TimeWarped => "`.time_warped`",
        Accessor::Done => "`.done`",
        Accessor::Result => "`.result`",
        Accessor::Bit => "`.bit`",
        Accessor::Bits => "`.bits`",
        Accessor::WithBit => "`.with_bit`",
        _ => "Zugriff",
    }
}
