//! Die Konstruktionen eines Programms (13.8, FB-377): welche Knoten der MIR
//! es benutzt, als Kennungen ohne Nutzlast.
//!
//! **Wozu.** Codegen, Rahmen und Beweiser tragen nicht jede Konstruktion;
//! was eine Komponente nicht kann, lehnte sie bisher in einem
//! Platzhalter-Zweig ab, und eine neue Konstruktion fiel dort still hinein.
//! Jede Komponente nennt darum in einer Tabelle ([`Support`]), was sie
//! traegt, mit einem `match` ohne Platzhalter ueber [`Construct`]: Kommt
//! eine Konstruktion dazu, uebersetzt keine Komponente, bis sie entschieden
//! hat. Ob die Tabellen stimmen, prueft ein Test am Korpus: Eine Komponente
//! nimmt ein Programm genau dann an, wenn ihre Tabelle jede seiner
//! Konstruktionen traegt.
//!
//! **Die Kette beginnt hier.** [`census`] liest die Konstruktionen mit
//! Matches ohne Platzhalter ab; eine neue Variante der MIR bricht schon
//! diese Datei.

use std::collections::BTreeSet;

use crate::expr::{Accessor, BinaryOp, Builtin, CheckedKind, ConvertKind, Expr, ExprKind, Intrinsic, MatOp};
use crate::expr::{MatchKind, TProp, TemporalOp, UnaryOp};
use crate::fns::FnParam;
use crate::machine::{Guard, Handler, Machine, MachineKind, SeqItem, State, Target, TransTrigger, Transition};
use crate::program::{Direction, Framing, Meta, Program};
use crate::stmt::{ArmPattern, Block, CheckKind, Place, StmtKind};
use crate::types::{HandleKind, Type};
use crate::{TypeId, visit};

with_all! {
    /// Ein Ausdruck (`ExprKind`).
    #[allow(missing_docs)]
    pub enum ExprTag {
        Bool, Int, Float, Duration, Str, None, Default, Variant, Record, Array, Tuple, BlockInit, Var, Param,
        Command, Input, Output, Published, StateOf, Signal, Builtin, Armed, PortRead, Field, Index, Index2, Slice,
        Accessor, Unary, Binary, Cond, Cast, Convert, Format, JobState, Stream, Matches, Call, NativeCall, MatOp,
        Decode, Checked, Lift, Ok, Err, Intrinsic,
    }
}

with_all! {
    /// Eine Anweisung (`StmtKind`).
    #[allow(missing_docs)]
    pub enum StmtTag {
        Assign, Check, Goto, Abort, If, ForRange, ForEach, Match, Return, Send, At, Cancel, Skip, Raise, Job, Every,
        Break, Observe, Arm, MethodCall, Pass,
    }
}

with_all! {
    /// Ein Typ, den das Programm benutzt (`Type`).
    #[allow(missing_docs)]
    pub enum TypeTag {
        Bool, Int, Float, Duration, Enum, Record, Array, Bytes, Vec, Str, Line, Samples, Table, Mat, Map, Optional,
        Result, Stream, Capture, HandleJob, HandleTrigger, HandleBlock,
    }
}

with_all! {
    /// Eine implizite Pruefung (`CheckedKind`, 4.1).
    #[allow(missing_docs)]
    pub enum CheckTag { DivZero, Overflow, NonFinite, Domain, Index, Range, Convert, Shift, Valid, Missing }
}

with_all! {
    /// Das Ziel einer Zuweisung (`Place`).
    #[allow(missing_docs)]
    pub enum PlaceTag { Var, Output, Port, Field, Index, Index2 }
}

with_all! {
    /// Ein Glied einer Sequenz (`SeqItem`, 6.2).
    #[allow(missing_docs)]
    pub enum SeqTag { Stmt, Wait, Until, Expect, Repeat, Step }
}

with_all! {
    /// Was eine Maschine, ein Zustand oder das Programm sonst traegt.
    pub enum Feature {
        /// Eine Maschine mit Periode ueber einem Tick (7.2).
        Multirate,
        /// Eine Maschine mit Phase (7.2).
        Phase,
        /// `follows` (7.2).
        Follows,
        /// Eine `driver machine` (12.10).
        DriverMachine,
        /// Eine Maschinenvorlage oder Instanz (5.7).
        Template,
        /// Eine Instanz einer Vorlage.
        Instance,
        /// Ein Szenario (13.5).
        Scenario,
        /// `persist var` (5.9).
        Persist,
        /// Ein Signal (5.5).
        Signal,
        /// Ein Uebergang aus `FAULTED` (5.3).
        FaultedTransition,
        /// Ein Handler auf Maschinenebene (8.7).
        MachineHandler,
        /// Kindzustaende (5.2).
        ChildStates,
        /// Ein `idle`-Zustand (5.10).
        Idle,
        /// `resume` (5.12).
        Resume,
        /// Ein `exit:`-Block.
        Exit,
        /// Eine Sequenz (6.2).
        Sequence,
        /// Eine gescopte Instanz (5.11).
        ScopedInstance,
        /// Ein Handler eines Zustands (8.7).
        StateHandler,
        /// Ein Fault-Ziel eines Zustands (5.3).
        StateFaultTarget,
        /// Ein Handler mit Muster (8.7).
        HandlerPattern,
        /// Ein Handler mit Bedingung.
        HandlerGuard,
        /// Ein Uebergang `after` (5.2).
        After,
        /// Ein Uebergang mit Bedingung.
        GuardExpr,
        /// Ein Uebergang auf ein Muster.
        GuardMatch,
        /// Ein Uebergang auf das naechste Element eines Stroms.
        GuardNext,
        /// Ein Uebergang in einen Zustand.
        TargetState,
        /// Ein Uebergang nach `FAULTED`.
        TargetFaulted,
        /// Ein Fault einer bestimmten Art als Ziel.
        TargetFault,
        /// `check … for` (6.3).
        CheckConfirm,
        /// `check … within` (6.3).
        CheckWithin,
        /// `expect` (13.5).
        Expect,
        /// Ein `case` auf Varianten.
        ArmVariant,
        /// Ein `case` auf Werte.
        ArmValues,
        /// `case _`.
        ArmWild,
        /// Ein Enum mit Feldern (3.8).
        EnumWithFields,
        /// Eine Funktion (5.7).
        Function,
        /// Ein Block (5.7).
        Block,
        /// Eine native Funktion (4.5).
        Native,
        /// Ein Eingabestrom (8.6).
        InputStream,
        /// Ein Ausgabestrom (8.8).
        OutputStream,
        /// Ein interner Strom (8.6).
        InternalStream,
        /// Ein Command (8.5).
        Command,
        /// Ein Parameter (8.4).
        Param,
        /// Ein Tunable (8.4).
        Tunable,
        /// Ein Parameterprofil (8.4).
        Profile,
        /// Eine Eigenschaft (13.3).
        Property,
        /// Eine Annahme (13.3).
        Assumption,
        /// Ein Laufzeitmonitor (13.3).
        Monitor,
        /// Ein Trigger (7.5).
        Trigger,
        /// Ein Registerport (12.10).
        Port,
        /// Ein aufgezeichneter Input (8.2).
        Recorded,
        /// Eine Kampagne (13.7).
        Campaign,
        /// Ein Knoten (v2).
        Node,
        /// `framing = raw` eines Stroms (8.6).
        FramingRaw,
        /// `framing = lines`.
        FramingLines,
        /// `framing = cobs`.
        FramingCobs,
        /// `framing = length_prefixed(…)`.
        FramingLengthPrefixed,
        /// `framing = fixed(n)`.
        FramingFixed,
        /// Das Metadatum `label` (2.5).
        Label,
        /// Das Metadatum `display = U` (2.5).
        Display,
        /// Das Metadatum `group` (2.5).
        Group,
    }
}

/// Die Kennungen der Zugriffe und ihre Abbildung, aus einer Liste: Ein
/// neuer [`Accessor`] bricht das `match` in [`accessor_tag`].
macro_rules! accessor_tags {
    ($($v:ident),* $(,)?) => {
        with_all! {
            /// Ein Zugriff (`Accessor`), `wrap_*` ohne Breite: Die Breite
            /// aendert nichts daran, ob eine Komponente ihn traegt.
            #[allow(missing_docs)]
            pub enum AccessorTag { $($v,)* Wrap }
        }

        /// Die Kennung eines Zugriffs.
        pub fn accessor_tag(a: Accessor) -> AccessorTag {
            match a {
                $(Accessor::$v => AccessorTag::$v,)*
                Accessor::Wrap(_) => AccessorTag::Wrap,
            }
        }
    };
}

accessor_tags! {
    Valid, Suspect, Stale, Age, Reason, Or, Ok, Err, T, Seq, Text, Data, Len, Count, Dropped, Malformed, Overflowed,
    Free, Jitter, TimeWarped, Done, Result, Bit, Bits, WithBit, Min, Max, Mean, Rms, Last, Encode, Get, StartsWith,
    Contains, Armed, Pre, Post, Samples, Rate, Remaining, Truncated, Peek, Sent, Idle,
}

/// Eine Konstruktion der MIR.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Construct {
    /// Ein Ausdruck.
    Expr(ExprTag),
    /// Ein einstelliger Operator.
    Unary(UnaryOp),
    /// Ein zweistelliger Operator.
    Binary(BinaryOp),
    /// Eine Umwandlung.
    Convert(ConvertKind),
    /// Eine Matrixoperation.
    Mat(MatOp),
    /// Eine eingebaute Groesse.
    Builtin(Builtin),
    /// Eine implizite Pruefung.
    Check(CheckTag),
    /// Eine Intrinsic.
    Intrinsic(Intrinsic),
    /// Ein Zugriff (`.len`, `.valid`, …).
    Accessor(AccessorTag),
    /// Ein Mustervergleich.
    Match(MatchKind),
    /// Eine Anweisung.
    Stmt(StmtTag),
    /// Das Ziel einer Zuweisung.
    Place(PlaceTag),
    /// Ein Typ.
    Type(TypeTag),
    /// Ein Glied einer Sequenz.
    Seq(SeqTag),
    /// Ein Zeitoperator einer Eigenschaft.
    Temporal(TemporalOp),
    /// Ein Merkmal einer Maschine, eines Zustands oder des Programms.
    Feature(Feature),
}

/// Ob eine Komponente eine Konstruktion traegt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Support {
    /// Sie traegt sie.
    Yes,
    /// Sie traegt sie in manchen Zusammenhaengen, in anderen lehnt sie ab
    /// (`Rem` nur auf Ganzzahlen); der Grund nennt die Grenze.
    Partial(&'static str),
    /// Sie lehnt sie ab, mit Grund.
    No(&'static str),
}

impl Support {
    /// Die Zelle der Faehigkeitsmatrix.
    pub fn cell(self) -> String {
        match self {
            Support::Yes => "ja".to_string(),
            Support::Partial(why) => format!("teilweise: {why}"),
            Support::No(why) => format!("nein: {why}"),
        }
    }
}

/// Alle Konstruktionen, die es gibt, in der Ordnung von [`Construct`].
pub fn all() -> Vec<Construct> {
    let mut out = Vec::new();
    out.extend(ExprTag::ALL.iter().map(|&t| Construct::Expr(t)));
    out.extend(UnaryOp::ALL.iter().map(|&t| Construct::Unary(t)));
    out.extend(BinaryOp::ALL.iter().map(|&t| Construct::Binary(t)));
    out.extend(ConvertKind::ALL.iter().map(|&t| Construct::Convert(t)));
    out.extend(MatOp::ALL.iter().map(|&t| Construct::Mat(t)));
    out.extend(Builtin::ALL.iter().map(|&t| Construct::Builtin(t)));
    out.extend(CheckTag::ALL.iter().map(|&t| Construct::Check(t)));
    out.extend(Intrinsic::ALL.iter().map(|&t| Construct::Intrinsic(t)));
    out.extend(AccessorTag::ALL.iter().map(|&t| Construct::Accessor(t)));
    out.extend(MatchKind::ALL.iter().map(|&t| Construct::Match(t)));
    out.extend(StmtTag::ALL.iter().map(|&t| Construct::Stmt(t)));
    out.extend(PlaceTag::ALL.iter().map(|&t| Construct::Place(t)));
    out.extend(TypeTag::ALL.iter().map(|&t| Construct::Type(t)));
    out.extend(SeqTag::ALL.iter().map(|&t| Construct::Seq(t)));
    out.extend(TemporalOp::ALL.iter().map(|&t| Construct::Temporal(t)));
    out.extend(Feature::ALL.iter().map(|&t| Construct::Feature(t)));
    out
}

/// Die Konstruktionen, die ein Programm benutzt: seine Maschinen, die
/// Funktionen, die sie erreichen, Kanaele, Stroeme, Parameter,
/// Eigenschaften und Trigger.
///
/// Die Strukturen stehen ohne `..` da: Ein neues Feld uebersetzt erst, wenn
/// hier entschieden ist, ob es eine Konstruktion traegt (`_` heisst: nein).
pub fn census(p: &Program) -> BTreeSet<Construct> {
    let Program {
        config: _,
        types,
        units: _,
        enums: _,
        records: _,
        fns,
        natives,
        blocks,
        machines,
        channels,
        streams,
        params,
        profiles,
        commands,
        nodes,
        properties,
        campaigns,
        triggers,
        ports,
        sources: _,
        recorded,
    } = p;
    let mut c = Census { p, out: BTreeSet::new() };
    for m in machines {
        c.machine(m);
    }
    for id in crate::analysis::reachable_fns(p) {
        let f = &fns[id.index()];
        c.feature(Feature::Function);
        c.params(&f.params);
        if let Some(ret) = f.ret {
            c.ty(ret);
        }
        for v in &f.locals {
            c.ty(v.ty);
        }
        c.block(&f.body);
    }
    for b in blocks {
        c.feature(Feature::Block);
        c.params(&b.params);
        for v in &b.state_vars {
            c.ty(v.ty);
        }
        for e in b.requires.iter().chain(&b.ensures) {
            c.expr(e);
        }
    }
    for ch in channels {
        c.ty(ch.ty);
        c.meta(&ch.meta);
        if let Some(framing) = ch.attrs.framing {
            c.feature(match framing {
                Framing::Raw => Feature::FramingRaw,
                Framing::Lines => Feature::FramingLines,
                Framing::Cobs => Feature::FramingCobs,
                Framing::LengthPrefixed(_) => Feature::FramingLengthPrefixed,
                Framing::Fixed(_) => Feature::FramingFixed,
            });
        }
        if matches!(types.get(ch.ty), Type::Stream(_)) {
            c.feature(match ch.dir {
                Direction::Input => Feature::InputStream,
                Direction::Output => Feature::OutputStream,
            });
        }
    }
    for s in streams {
        c.feature(Feature::InternalStream);
        c.ty(s.elem);
    }
    for prm in params {
        c.feature(if prm.tunable { Feature::Tunable } else { Feature::Param });
        c.meta(&prm.meta);
        c.ty(prm.ty);
        c.expr(&prm.default);
    }
    for prop in properties {
        c.feature(if prop.assumption { Feature::Assumption } else { Feature::Property });
        if prop.monitor {
            c.feature(Feature::Monitor);
        }
        c.prop(&prop.formula);
    }
    for cmd in commands {
        c.meta(&cmd.meta);
    }
    for t in triggers {
        c.feature(Feature::Trigger);
        c.guard(&t.guard);
        c.expr(&t.time);
    }
    let flags = [
        (!natives.is_empty(), Feature::Native),
        (!commands.is_empty(), Feature::Command),
        (!profiles.is_empty(), Feature::Profile),
        (!ports.is_empty(), Feature::Port),
        (!recorded.is_empty(), Feature::Recorded),
        (!campaigns.is_empty(), Feature::Campaign),
        (!nodes.is_empty(), Feature::Node),
    ];
    for (present, f) in flags {
        if present {
            c.feature(f);
        }
    }
    c.out
}

struct Census<'a> {
    p: &'a Program,
    out: BTreeSet<Construct>,
}

impl Census<'_> {
    fn feature(&mut self, f: Feature) {
        self.out.insert(Construct::Feature(f));
    }

    /// Die Metadaten (2.5); `doc` ist Kommentar und keine Konstruktion.
    fn meta(&mut self, m: &Meta) {
        let Meta { label, display, group, doc: _ } = m;
        let present = [
            (label.is_some(), Feature::Label),
            (display.is_some(), Feature::Display),
            (group.is_some(), Feature::Group),
        ];
        for (on, f) in present {
            if on {
                self.feature(f);
            }
        }
    }

    fn params(&mut self, params: &[FnParam]) {
        for prm in params {
            self.ty(prm.ty);
        }
    }

    fn machine(&mut self, m: &Machine) {
        let Machine {
            name: _,
            state_enum: _,
            kind,
            driver,
            polling_unchecked: _,
            fault_is_fail: _,
            params,
            period,
            phase,
            follows,
            node: _,
            vars,
            persist,
            signals,
            fault_target: _,
            states,
            roots: _,
            initial: _,
            loop_block: _,
            handlers,
            faulted,
            layout: _,
            budget: _,
            declared_budget: _,
            meta,
            span: _,
        } = m;
        self.meta(meta);
        let flags = [
            (*period > 1, Feature::Multirate),
            (*phase > 0, Feature::Phase),
            (!follows.is_empty(), Feature::Follows),
            (*driver, Feature::DriverMachine),
            (!persist.is_empty(), Feature::Persist),
            (!signals.is_empty(), Feature::Signal),
            (!faulted.transitions.is_empty(), Feature::FaultedTransition),
            (!handlers.is_empty(), Feature::MachineHandler),
        ];
        for (present, f) in flags {
            if present {
                self.feature(f);
            }
        }
        match kind {
            MachineKind::Regular => {}
            MachineKind::Template => self.feature(Feature::Template),
            MachineKind::Instance(_) => self.feature(Feature::Instance),
            MachineKind::Scenario => self.feature(Feature::Scenario),
        }
        self.params(params);
        for v in vars {
            self.ty(v.ty);
        }
        for h in handlers {
            self.handler(h);
        }
        for s in states {
            self.state(s);
        }
        for t in &faulted.transitions {
            self.transition(t);
        }
        visit::for_each_stmt(m, &mut |s| self.stmt(&s.kind));
        visit::for_each_expr_machine(m, &mut |e| self.node(e));
    }

    /// Ein Zustand; Anweisungen und Ausdruecke liefert der Durchlauf der Maschine.
    fn state(&mut self, s: &State) {
        let State {
            name: _,
            parent: _,
            children,
            initial: _,
            idle,
            resume,
            vars: _,
            enter: _,
            exit,
            loop_block: _,
            handlers,
            transitions,
            fault_target,
            sequence,
            sequence_ticks: _,
            instances,
            step_name: _,
            meta,
            span: _,
        } = s;
        self.meta(meta);
        let flags = [
            (!children.is_empty(), Feature::ChildStates),
            (*idle, Feature::Idle),
            (*resume, Feature::Resume),
            (!exit.stmts.is_empty(), Feature::Exit),
            (sequence.is_some(), Feature::Sequence),
            (!instances.is_empty(), Feature::ScopedInstance),
            (!handlers.is_empty(), Feature::StateHandler),
            (fault_target.is_some(), Feature::StateFaultTarget),
        ];
        for (present, f) in flags {
            if present {
                self.feature(f);
            }
        }
        for h in handlers {
            self.handler(h);
        }
        for t in transitions {
            self.transition(t);
        }
        if let Some(seq) = sequence {
            self.seq(&seq.items);
        }
    }

    fn handler(&mut self, h: &Handler) {
        let Handler { stream: _, pattern, binding: _, guard, body: _, span: _ } = h;
        if let Some((kind, _)) = pattern {
            self.out.insert(Construct::Match(*kind));
            self.feature(Feature::HandlerPattern);
        }
        if guard.is_some() {
            self.feature(Feature::HandlerGuard);
        }
    }

    fn transition(&mut self, t: &Transition) {
        let Transition { trigger, actions: _, target, kind: _, span: _ } = t;
        self.trigger(trigger);
        self.target(target);
    }

    fn trigger(&mut self, t: &TransTrigger) {
        match t {
            TransTrigger::After(_) => self.feature(Feature::After),
            TransTrigger::When(g) => self.guard(g),
        }
    }

    fn guard(&mut self, g: &Guard) {
        let f = match g {
            Guard::Expr(_) => Feature::GuardExpr,
            Guard::Match { kind, .. } => {
                self.out.insert(Construct::Match(*kind));
                Feature::GuardMatch
            }
            Guard::Next { .. } => Feature::GuardNext,
        };
        self.feature(f);
    }

    fn target(&mut self, t: &Target) {
        self.feature(match t {
            Target::State(_) => Feature::TargetState,
            Target::Faulted => Feature::TargetFaulted,
            Target::Fault(_) => Feature::TargetFault,
        });
    }

    fn seq(&mut self, items: &[SeqItem]) {
        for item in items {
            let tag = match item {
                SeqItem::Stmt(_) => SeqTag::Stmt,
                SeqItem::Wait(_) => SeqTag::Wait,
                SeqItem::Until { guard, .. } => {
                    self.guard(guard);
                    SeqTag::Until
                }
                SeqItem::Expect { .. } => SeqTag::Expect,
                SeqItem::Repeat { body, .. } => {
                    self.seq(body);
                    SeqTag::Repeat
                }
                SeqItem::Step { body, .. } => {
                    self.seq(body);
                    SeqTag::Step
                }
            };
            self.out.insert(Construct::Seq(tag));
        }
    }

    fn block(&mut self, b: &Block) {
        visit::for_each_stmt_block(b, &mut |s| self.stmt(&s.kind));
        visit::for_each_expr_block(b, &mut |e| self.node(e));
    }

    fn expr(&mut self, e: &Expr) {
        visit::walk_expr(e, &mut |x| self.node(x));
    }

    fn prop(&mut self, t: &TProp) {
        match t {
            TProp::Temporal { op, inner, .. } => {
                self.out.insert(Construct::Temporal(*op));
                self.prop(inner);
            }
            TProp::Implies(a, b) | TProp::And(a, b) | TProp::Or(a, b) => {
                self.prop(a);
                self.prop(b);
            }
            TProp::Not(a) => self.prop(a),
            TProp::Atom(e) => self.expr(e),
        }
    }

    fn stmt(&mut self, s: &StmtKind) {
        let tag = match s {
            StmtKind::Assign { target, .. } => {
                self.place(target);
                StmtTag::Assign
            }
            StmtKind::Check { confirm, within, target, kind, .. } => {
                if confirm.is_some() {
                    self.feature(Feature::CheckConfirm);
                }
                if within.is_some() {
                    self.feature(Feature::CheckWithin);
                }
                if let Some(t) = target {
                    self.target(t);
                }
                match kind {
                    CheckKind::Check => {}
                    CheckKind::Expect => self.feature(Feature::Expect),
                }
                StmtTag::Check
            }
            StmtKind::Goto(t) => {
                self.target(t);
                StmtTag::Goto
            }
            StmtKind::Abort { .. } => StmtTag::Abort,
            StmtKind::If { .. } => StmtTag::If,
            StmtKind::ForRange { .. } => StmtTag::ForRange,
            StmtKind::ForEach { .. } => StmtTag::ForEach,
            StmtKind::Match { arms, .. } => {
                for a in arms {
                    self.feature(match a.pattern {
                        ArmPattern::Variant { .. } => Feature::ArmVariant,
                        ArmPattern::Values(_) => Feature::ArmValues,
                        ArmPattern::Wild => Feature::ArmWild,
                    });
                }
                StmtTag::Match
            }
            StmtKind::Return(_) => StmtTag::Return,
            StmtKind::Send { .. } => StmtTag::Send,
            StmtKind::At { .. } => StmtTag::At,
            StmtKind::Cancel(_) => StmtTag::Cancel,
            StmtKind::Skip(_) => StmtTag::Skip,
            StmtKind::Raise(_) => StmtTag::Raise,
            StmtKind::Job { .. } => StmtTag::Job,
            StmtKind::Every { .. } => StmtTag::Every,
            StmtKind::Break => StmtTag::Break,
            StmtKind::Observe(_) => StmtTag::Observe,
            StmtKind::Arm { .. } => StmtTag::Arm,
            StmtKind::MethodCall { target, receiver, .. } => {
                if let Some(t) = target {
                    self.place(t);
                }
                self.place(receiver);
                StmtTag::MethodCall
            }
            StmtKind::Pass => StmtTag::Pass,
        };
        self.out.insert(Construct::Stmt(tag));
    }

    fn place(&mut self, p: &Place) {
        let tag = match p {
            Place::Var(_) => PlaceTag::Var,
            Place::Output(_) => PlaceTag::Output,
            Place::Port(..) => PlaceTag::Port,
            Place::Field(b, _) => {
                self.place(b);
                PlaceTag::Field
            }
            Place::Index(b, _) => {
                self.place(b);
                PlaceTag::Index
            }
            Place::Index2(b, ..) => {
                self.place(b);
                PlaceTag::Index2
            }
        };
        self.out.insert(Construct::Place(tag));
    }

    /// Ein Knoten eines Ausdrucks, ohne seine Teilausdruecke (die liefert
    /// der Durchlauf).
    fn node(&mut self, e: &Expr) {
        let tag = match &e.kind {
            ExprKind::Bool(_) => ExprTag::Bool,
            ExprKind::Int(_) => ExprTag::Int,
            ExprKind::Float(_) => ExprTag::Float,
            ExprKind::Duration(_) => ExprTag::Duration,
            ExprKind::Str(_) => ExprTag::Str,
            ExprKind::None => ExprTag::None,
            ExprKind::Default => ExprTag::Default,
            ExprKind::Variant { .. } => ExprTag::Variant,
            ExprKind::Record { .. } => ExprTag::Record,
            ExprKind::Array(_) => ExprTag::Array,
            ExprKind::Tuple(..) => ExprTag::Tuple,
            ExprKind::BlockInit { .. } => ExprTag::BlockInit,
            ExprKind::Var(_) => ExprTag::Var,
            ExprKind::Param(_) => ExprTag::Param,
            ExprKind::Command(_) => ExprTag::Command,
            ExprKind::Input { .. } => ExprTag::Input,
            ExprKind::Output(_) => ExprTag::Output,
            ExprKind::Published { .. } => ExprTag::Published,
            ExprKind::StateOf(_) => ExprTag::StateOf,
            ExprKind::Signal { .. } => ExprTag::Signal,
            ExprKind::Builtin(b) => {
                self.out.insert(Construct::Builtin(*b));
                ExprTag::Builtin
            }
            ExprKind::Armed(_) => ExprTag::Armed,
            ExprKind::PortRead(_) => ExprTag::PortRead,
            ExprKind::Field { .. } => ExprTag::Field,
            ExprKind::Index { .. } => ExprTag::Index,
            ExprKind::Index2 { .. } => ExprTag::Index2,
            ExprKind::Slice { .. } => ExprTag::Slice,
            ExprKind::Accessor { accessor, .. } => {
                self.out.insert(Construct::Accessor(accessor_tag(*accessor)));
                ExprTag::Accessor
            }
            ExprKind::Unary { op, .. } => {
                self.out.insert(Construct::Unary(*op));
                ExprTag::Unary
            }
            ExprKind::Binary { op, .. } => {
                self.out.insert(Construct::Binary(*op));
                ExprTag::Binary
            }
            ExprKind::Cond { .. } => ExprTag::Cond,
            ExprKind::Cast { .. } => ExprTag::Cast,
            ExprKind::Convert { kind, .. } => {
                self.out.insert(Construct::Convert(*kind));
                ExprTag::Convert
            }
            ExprKind::Format(_) => ExprTag::Format,
            ExprKind::JobState { .. } => ExprTag::JobState,
            ExprKind::Stream(_) => ExprTag::Stream,
            ExprKind::Matches { kind, .. } => {
                self.out.insert(Construct::Match(*kind));
                ExprTag::Matches
            }
            ExprKind::Call { .. } => ExprTag::Call,
            ExprKind::NativeCall { .. } => ExprTag::NativeCall,
            ExprKind::MatOp { op, .. } => {
                self.out.insert(Construct::Mat(*op));
                ExprTag::MatOp
            }
            ExprKind::Decode { .. } => ExprTag::Decode,
            ExprKind::Checked { kind, .. } => {
                self.out.insert(Construct::Check(check_tag(kind)));
                ExprTag::Checked
            }
            ExprKind::Lift(_) => ExprTag::Lift,
            ExprKind::Ok(_) => ExprTag::Ok,
            ExprKind::Err(_) => ExprTag::Err,
            ExprKind::Intrinsic { op, .. } => {
                self.out.insert(Construct::Intrinsic(*op));
                ExprTag::Intrinsic
            }
        };
        self.out.insert(Construct::Expr(tag));
        self.ty(e.ty);
    }

    /// Ein Typ und die Typen in ihm.
    fn ty(&mut self, ty: TypeId) {
        let tag = match self.p.types.get(ty) {
            Type::Bool => TypeTag::Bool,
            Type::Int { .. } => TypeTag::Int,
            Type::Float { .. } => TypeTag::Float,
            Type::Duration { .. } => TypeTag::Duration,
            Type::Enum(id) => {
                let fields: Vec<TypeId> =
                    self.p.enums[id.index()].variants.iter().flat_map(|v| v.fields.iter().map(|f| f.ty)).collect();
                if !fields.is_empty() {
                    self.feature(Feature::EnumWithFields);
                }
                self.tys(&fields);
                TypeTag::Enum
            }
            Type::Record(id) => {
                let fields: Vec<TypeId> = self.p.records[id.index()].fields.iter().map(|f| f.ty).collect();
                self.tys(&fields);
                TypeTag::Record
            }
            Type::Array { elem, .. } => {
                self.ty(*elem);
                TypeTag::Array
            }
            Type::Bytes { .. } => TypeTag::Bytes,
            Type::Vec { elem, .. } => {
                self.ty(*elem);
                TypeTag::Vec
            }
            Type::Str { .. } => TypeTag::Str,
            Type::Line { .. } => TypeTag::Line,
            Type::Samples { elem, .. } => {
                self.ty(*elem);
                TypeTag::Samples
            }
            Type::Table { key, value } => {
                self.tys(&[*key, *value]);
                TypeTag::Table
            }
            Type::Mat { .. } => TypeTag::Mat,
            Type::Map { key, value, .. } => {
                self.tys(&[*key, *value]);
                TypeTag::Map
            }
            Type::Optional(inner) => {
                self.ty(*inner);
                TypeTag::Optional
            }
            Type::Result { ok, .. } => {
                self.ty(*ok);
                TypeTag::Result
            }
            Type::Stream(elem) => {
                self.ty(*elem);
                TypeTag::Stream
            }
            Type::Capture { elem, .. } => {
                self.ty(*elem);
                TypeTag::Capture
            }
            Type::Handle(HandleKind::Job) => TypeTag::HandleJob,
            Type::Handle(HandleKind::Trigger) => TypeTag::HandleTrigger,
            Type::Handle(HandleKind::Block(_)) => TypeTag::HandleBlock,
        };
        self.out.insert(Construct::Type(tag));
    }

    /// Mehrere Typen. Rekursion endet: Ein Typ ohne Indirektion kann sich
    /// nicht selbst enthalten.
    fn tys(&mut self, tys: &[TypeId]) {
        for &t in tys {
            self.ty(t);
        }
    }
}

/// Die Kennung einer impliziten Pruefung.
pub fn check_tag(kind: &CheckedKind) -> CheckTag {
    match kind {
        CheckedKind::DivZero => CheckTag::DivZero,
        CheckedKind::Overflow => CheckTag::Overflow,
        CheckedKind::NonFinite => CheckTag::NonFinite,
        CheckedKind::Domain => CheckTag::Domain,
        CheckedKind::Index { .. } => CheckTag::Index,
        CheckedKind::Range(_) => CheckTag::Range,
        CheckedKind::Convert => CheckTag::Convert,
        CheckedKind::Shift => CheckTag::Shift,
        CheckedKind::Valid => CheckTag::Valid,
        CheckedKind::Missing => CheckTag::Missing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_lists_every_construct_once() {
        let a = all();
        assert_eq!(a.iter().collect::<BTreeSet<_>>().len(), a.len());
    }
}
