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
        for _ in &s.handlers {
            c.note("`on`-Handler", false);
        }
    }
    for _ in &m.handlers {
        c.note("`on`-Handler", false);
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
            c.note("`match`", false);
            expr(subject, c);
            for a in arms {
                block(&a.body, c);
            }
        }
        other => c.note(stmt_name(other), false),
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
        StmtKind::MethodCall { .. } => "Blockmethode",
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
            let ok = matches!(accessor, Accessor::Bit | Accessor::Bits | Accessor::WithBit) || (quality && on_channel);
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
        ExprKind::Call { .. } => c.note("Funktionsaufruf", false),
        ExprKind::Intrinsic { .. } => c.note("Primitive", false),
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
        ExprKind::Published { .. } => c.note("Psi (`m.x`)", false),
        ExprKind::StateOf(_) => c.note("`m.state`", false),
        ExprKind::Record { fields, .. } => {
            c.note("Record-Literal", true);
            for f in fields {
                expr(f, c);
            }
        }
        ExprKind::Variant { fields, .. } => c.note("Variante", fields.is_empty()),
        ExprKind::Array(_) => c.note("Array-Literal", false),
        ExprKind::Decode { .. } => c.note("`decode`", false),
        ExprKind::Matches { .. } => c.note("`matches`", false),
        ExprKind::MatOp { .. } => c.note("Matrixoperation", false),
        ExprKind::Slice { .. } => c.note("Teilbereich", false),
        ExprKind::NativeCall { .. } => c.note("native Funktion", false),
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
