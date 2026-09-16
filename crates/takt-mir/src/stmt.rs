//! Anweisungen: genau die Formen von Referenz 9.2 (plan/mir.md 2.5).
//!
//! `pulse` ist kein Knoten (Lowering: `Assign; At{now + d, Assign(latch)}`),
//! `var x = e` ist ein `Assign` auf einen deklarierten Slot, `block.step`,
//! `push` und Geschwister sind `MethodCall` (Ausdruecke bleiben rein, 4.4).

use takt_diag::Span;

use crate::expr::{Expr, StreamRef};
use crate::ids::*;
use crate::machine::Target;
use crate::pattern::Format;

/// Folge von Anweisungen.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Block {
    /// Anweisungen.
    pub stmts: Vec<Stmt>,
    /// Position.
    pub span: Span,
}

impl Block {
    /// Ruft `f` fuer jede Anweisung, auch in geschachtelten Bloecken.
    pub fn walk(&self, f: &mut impl FnMut(&Stmt)) {
        for s in &self.stmts {
            f(s);
            match &s.kind {
                StmtKind::If { then, otherwise, .. } => {
                    then.walk(f);
                    otherwise.walk(f);
                }
                StmtKind::ForRange { body, .. }
                | StmtKind::ForEach { body, .. }
                | StmtKind::At { body, .. }
                | StmtKind::Every { body, .. } => body.walk(f),
                StmtKind::Match { arms, .. } => arms.iter().for_each(|a| a.body.walk(f)),
                _ => {}
            }
        }
    }

    /// Block aus Anweisungen.
    pub fn new(stmts: Vec<Stmt>) -> Self {
        Block { stmts, span: Span::default() }
    }
}

/// Eine Anweisung mit Position (Telemetrie `pc = LINE`, 11.2).
#[derive(Clone, Debug, PartialEq)]
pub struct Stmt {
    /// Inhalt.
    pub kind: StmtKind,
    /// Position.
    pub span: Span,
}

impl Stmt {
    /// Anweisung mit Position.
    pub fn new(kind: StmtKind, span: Span) -> Self {
        Stmt { kind, span }
    }
}

/// Zuweisungsziel.
#[derive(Clone, Debug, PartialEq)]
pub enum Place {
    /// Variable, Blockinstanz oder Handle.
    Var(VarId),
    /// Eigener Output.
    Output(ChannelId),
    /// Feld.
    Field(Box<Place>, u32),
    /// Element.
    Index(Box<Place>, Expr),
    /// Matrixelement.
    Index2(Box<Place>, Expr, Expr),
}

/// `check` oder `expect` (5.6, 6.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckKind {
    /// Kontinuierlich, Fault-Art `CheckFailed`.
    Check,
    /// Einmalig, Fault-Art `Expect`.
    Expect,
}

/// Bestaetigungszeit `for d` (5.6): Zaehler `viol[site]`.
#[derive(Clone, Debug, PartialEq)]
pub struct Confirm {
    /// Frist `d`.
    pub duration: Expr,
    /// Zaehlerstelle.
    pub site: SiteId,
}

/// Laufvariablen einer `for`-Schleife.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ForVars {
    /// `for x in …`
    One(VarId),
    /// `for (k, v) in m` (3.9).
    Pair(VarId, VarId),
}

/// Wert oder Bereich in einem `case`.
#[derive(Clone, Debug, PartialEq)]
pub struct CaseValue {
    /// Wert oder Untergrenze.
    pub lo: Expr,
    /// Obergrenze bei `a..b`.
    pub hi: Option<Expr>,
}

/// Muster eines `case`.
#[derive(Clone, Debug, PartialEq)]
pub enum ArmPattern {
    /// Variante mit Feldbindungen (gehobene Variablen).
    Variant {
        /// Index der Variante.
        variant: u32,
        /// Gebundene Felder in Deklarationsreihenfolge.
        fields: Vec<VarId>,
    },
    /// Werte und Bereiche.
    Values(Vec<CaseValue>),
    /// `case _`
    Wild,
}

impl ForVars {
    /// Die gebundenen Variablen in Deklarationsreihenfolge.
    pub fn ids(&self) -> Vec<VarId> {
        match self {
            ForVars::One(v) => vec![*v],
            ForVars::Pair(k, v) => vec![*k, *v],
        }
    }
}

impl ArmPattern {
    /// Die Variablen, die dieser Zweig bindet (leer ausser bei `Variant`).
    pub fn bound(&self) -> Vec<VarId> {
        match self {
            ArmPattern::Variant { fields, .. } => fields.clone(),
            _ => Vec::new(),
        }
    }
}

/// Zweig eines `match`.
#[derive(Clone, Debug, PartialEq)]
pub struct Arm {
    /// Muster.
    pub pattern: ArmPattern,
    /// Rumpf.
    pub body: Block,
    /// Position.
    pub span: Span,
}

/// Beobachtung (5.6, 13.5): nie ein Fault.
#[derive(Clone, Debug, PartialEq)]
pub enum Observe {
    /// `alert cond, "text" [for d]`
    Alert {
        /// Bedingung.
        cond: Expr,
        /// Meldung.
        message: Format,
        /// Bestaetigungszeit.
        confirm: Option<Confirm>,
    },
    /// `log "text"`
    Log(Format),
    /// `measure name = e`
    Measure {
        /// Name des Messwerts.
        name: String,
        /// Wert.
        value: Expr,
    },
    /// `verify cond, "text" [req "SR-n"]`
    Verify {
        /// Bedingung.
        cond: Expr,
        /// Meldung.
        message: Format,
        /// Anforderungsreferenz (v1.2).
        req: Option<String>,
    },
    /// `verdict pass | fail ["text"]`
    Verdict {
        /// `pass` oder `fail`.
        pass: bool,
        /// Meldung.
        message: Option<Format>,
    },
}

/// Mutierende Methode (5.7, 3.9, 8.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// `step(args)` einer Blockinstanz, hoechstens einmal je Tick (5.7).
    Step,
    /// `reset()` einer Blockinstanz.
    Reset,
    /// Weitere Methode eines Blocks (`start()`, `stop()`).
    Block(FnId),
    /// `push(x) -> bool`
    Push,
    /// `append(src) -> bool`: haengt eine ganze Folge an (3.9). Kosten
    /// O(Kapazitaet der Quelle), damit die Schranke statisch bleibt.
    Append,
    /// `insert(k, v) -> bool`
    Insert,
    /// `remove(k) -> bool`
    Remove,
    /// `clear()`
    Clear,
}

impl Method {
    /// Der reservierte Membername (2.5); Blockmethoden tragen ihren eigenen.
    pub fn name(self) -> Option<&'static str> {
        match self {
            Method::Step => Some("step"),
            Method::Reset => Some("reset"),
            Method::Block(_) => None,
            Method::Push => Some("push"),
            Method::Append => Some("append"),
            Method::Insert => Some("insert"),
            Method::Remove => Some("remove"),
            Method::Clear => Some("clear"),
        }
    }
}

/// Inhalt einer Anweisung.
#[derive(Clone, Debug, PartialEq)]
#[allow(missing_docs)]
pub enum StmtKind {
    Assign {
        target: Place,
        value: Expr,
    },
    /// `check`/`expect` mit Nachricht, Bestaetigung, eigenem Ziel und Anforderung.
    Check {
        cond: Expr,
        message: Option<Format>,
        confirm: Option<Confirm>,
        /// `within d` (9.4.5): geforderte Safe-State-Latenz. Die MIR traegt
        /// sie, weil die Latenzanalyse sie braucht und der Report sie nennt.
        within: Option<Expr>,
        target: Option<Target>,
        req: Option<String>,
        kind: CheckKind,
    },
    /// `-> q`; nur im Modus RUN wirksam.
    Goto(Target),
    Abort {
        message: Option<Format>,
    },
    If {
        cond: Expr,
        then: Block,
        otherwise: Block,
    },
    /// `for i in range(n)`, `n` statisch.
    ForRange {
        var: VarId,
        count: Expr,
        body: Block,
    },
    /// `for x in W` ueber Arrays, Samples, Fenster und Maps (9.2, 9.6).
    ForEach {
        vars: ForVars,
        iter: Expr,
        body: Block,
    },
    Match {
        subject: Expr,
        arms: Vec<Arm>,
    },
    /// Nur in `Fn`.
    Return(Expr),
    /// `send o, e`; `len_max` statisch (8.8).
    Send {
        stream: StreamRef,
        value: Expr,
        len_max: u32,
    },
    /// `at T:`; der Block enthaelt nur Output-Zuweisungen (5.5, 9.8).
    At {
        time: Expr,
        body: Block,
    },
    Cancel(ChannelId),
    /// `s.skip()`: untersucht das ganze Fenster und verwirft es (8.6).
    Skip(StreamRef),
    Raise(SignalId),
    /// `job v = f(args)` (4.5, v1.1).
    Job {
        handle: VarId,
        native: NativeId,
        args: Vec<Expr>,
    },
    /// `every d:` mit Zaehler `next` (5.8).
    Every {
        period: Expr,
        counter: CounterId,
        body: Block,
    },
    Break,
    Observe(Observe),
    /// `arm`/`disarm` (7.5, v1.2).
    Arm {
        trigger: TriggerId,
        on: bool,
    },
    /// `target = receiver.method(args)`; `target` fehlt bei `reset()`, `clear()`, `skip()`.
    MethodCall {
        target: Option<Place>,
        receiver: Place,
        method: Method,
        args: Vec<Expr>,
    },
    Pass,
}
