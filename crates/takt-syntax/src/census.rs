//! Die Konstruktionen der Syntax, die die MIR nicht sieht (13.2, KOR-006).
//!
//! **Wozu.** `takt_mir::census` liest ab, welche Knoten der MIR ein Programm
//! benutzt, und haelt so die Abdeckung des Korpus fest. Was die Sema
//! entzuckert, kommt dort nie an: `pulse` wird zu Zuweisung und `at`,
//! `x += 1` zu einer Zuweisung mit Addition, ein `case`-Bereich zu einem
//! Wert, den der Zensus der MIR nicht von einem einzelnen unterscheidet,
//! eine Konstante und eine Einheit zu Zahlen. [`census`] zaehlt genau diese
//! Formen am Baum des Parsers.
//!
//! **Keine Form faellt durch.** Jede Variante der Aufzaehlungen, die Zucker
//! tragen koennen, steht in einem `match` ohne Platzhalter: entweder mit
//! ihrer Kennung oder mit dem Knoten der MIR, der sie sichtbar traegt. Eine
//! neue Variante uebersetzt erst, wenn hier entschieden ist, wohin sie
//! gehoert. Konfiguration (`system:`) und Attribute nach `with` leitet die
//! Inventur ab (`takt-sema/tests/inventory.rs`), sie stehen nicht hier.

use std::collections::BTreeSet;

use crate::ast::*;

macro_rules! tags {
    ($(#[$m:meta])* $vis:vis enum $name:ident { $($(#[$vm:meta])* $v:ident),* $(,)? }) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        $vis enum $name {
            $($(#[$vm])* $v),*
        }

        impl $name {
            /// Alle Kennungen, in der Ordnung der Deklaration.
            pub const ALL: [$name; [$(stringify!($v)),*].len()] = [$($name::$v),*];
        }
    };
}

tags! {
    /// Eine Form, die die Sema entzuckert oder aufloest; die MIR (und ihr
    /// Zensus) sieht sie nicht.
    pub enum Sugar {
        /// `import a.b` (8.2): aufgeloest beim Uebersetzen.
        ImportModule,
        /// `import channels from "…"` (8.2).
        ImportChannels,
        /// `type T = …`: ein Alias, aufgeloest.
        TypeAlias,
        /// `unitvec` (3.11).
        Unitvec,
        /// `unit x = f y` (3.2).
        UnitScaled,
        /// `unit x = affine(…)` (3.2).
        UnitAffine,
        /// `const` (3.1): eingesetzt.
        Const,
        /// Einheitenvariable `[U]` (3.12): monomorphisiert.
        GenericUnit,
        /// Typvariable `[type T]` (3.12).
        GenericType,
        /// Konstantenvariable `[const N]` (3.12).
        GenericConst,
        /// Explizite Instanziierung `f[U](…)`.
        ExplicitGenerics,
        /// Benannte Argumente `f(x = …)`: in die Reihenfolge der Parameter gestellt.
        NamedArgs,
        /// `var` in einem Block: zur Variable der Maschine oder Funktion gehoben.
        LocalVar,
        /// `var` vor `initial` eines Zustands: zur Variable der Maschine gehoben.
        StateVar,
        /// `x += …` (2.3).
        AssignAdd,
        /// `x -= …`.
        AssignSub,
        /// `x *= …`.
        AssignMul,
        /// `x /= …`.
        AssignDiv,
        /// `pulse o = v for d` (7.5): Zuweisung und `at`.
        Pulse,
        /// `alert` (5.6): eine Beobachtung, die der Zensus der MIR nicht unterscheidet.
        Alert,
        /// `log` (13.5).
        Log,
        /// `measure` (13.5).
        Measure,
        /// `verify` (13.5).
        Verify,
        /// `verdict` (13.5).
        Verdict,
        /// `elif`: ein geschachteltes `if`.
        Elif,
        /// `for (k, v) in m` (3.9).
        ForPair,
        /// `case a..b` (2.3): ein Bereich unter den Werten.
        CaseRange,
        /// Klammern um einen Ausdruck: fallen weg.
        Paren,
        /// Zahl mit Einheit (`85 degC`, 3.2): in die Basiseinheit gerechnet.
        UnitLiteral,
        /// `[N] block(…)` (5.7): N Instanzen.
        InstanceArray,
        /// `implies` in einer Eigenschaft (13.3).
        Implies,
        /// Der Segment-Default `sequence with timeout = d` (6.2): auf jedes `until` verteilt.
        SequenceTimeout,
        /// `timeout d else:` (6.2): ein weicher Timeout mit Aktionsblock.
        TimeoutElse,
        /// `mat<…>` mit Einheiten je Zeile und Spalte (3.11).
        MatDim,
        /// `vec<…>` mit Einheiten je Komponente (3.11).
        VecDim,
    }
}

/// Die Formen, die eine Datei benutzt.
pub fn census(file: &File) -> BTreeSet<Sugar> {
    let mut c = Census::default();
    for item in &file.items {
        c.item(item);
    }
    c.out
}

#[derive(Default)]
struct Census {
    out: BTreeSet<Sugar>,
}

impl Census {
    fn add(&mut self, s: Sugar) {
        self.out.insert(s);
    }

    fn item(&mut self, item: &Item) {
        match item {
            Item::Import(Import::Module { .. }) => self.add(Sugar::ImportModule),
            Item::Import(Import::Channels { .. }) => self.add(Sugar::ImportChannels),
            // Konfiguration; die Inventur leitet die Eintraege ab.
            Item::System(s) => {
                for i in &s.items {
                    match i {
                        SystemItem::TickTolerance { value, .. } => self.expr(value),
                        SystemItem::Tick(_)
                        | SystemItem::OutputTiming(_)
                        | SystemItem::FaultIsFail(_)
                        | SystemItem::TickSource(_)
                        | SystemItem::Target(_)
                        | SystemItem::Float(_)
                        | SystemItem::Language(_)
                        | SystemItem::TcbPolicy(_)
                        | SystemItem::Overrun(_) => {}
                    }
                }
            }
            Item::Type(t) => {
                self.add(Sugar::TypeAlias);
                self.ty(&t.ty);
            }
            Item::Unitvec(_) => self.add(Sugar::Unitvec),
            Item::Unit(UnitDecl::Scaled { .. }) => self.add(Sugar::UnitScaled),
            Item::Unit(UnitDecl::Affine { .. }) => self.add(Sugar::UnitAffine),
            // MIR: `Type(Enum)`, `EnumWithFields`.
            Item::Enum(e) => {
                for v in &e.variants {
                    self.fields(&v.fields);
                }
            }
            // MIR: `Type(Record)`.
            Item::Record(r) => {
                for f in &r.fields {
                    match f {
                        RecordField::Plain(f) => self.fields(std::slice::from_ref(f)),
                        RecordField::Bits { .. } => {}
                    }
                }
            }
            // MIR: `InternalStream`.
            Item::Stream(s) => {
                self.elem(&s.elem);
                self.attrs(&s.attrs);
            }
            // MIR: `Port`, `Node`.
            Item::Port(_) | Item::Node(_) => {}
            // MIR: `Property`, `Assumption`, `Temporal`.
            Item::Property(p) => self.expr(&p.prop),
            Item::Const(c) => {
                self.add(Sugar::Const);
                self.opt_ty(c.ty.as_ref());
                self.expr(&c.value);
            }
            // MIR: `Param`, `Tunable`.
            Item::Param(p) => {
                self.ty(&p.ty);
                self.expr(&p.value);
                self.attrs(&p.attrs);
            }
            // MIR: `Profile`.
            Item::Profile(p) => {
                for (_, e) in &p.entries {
                    self.expr(e);
                }
            }
            // MIR: `Input`, `Output`, `InputStream`, `OutputStream`.
            Item::Channel(ch) => {
                self.ty(&ch.ty);
                self.attrs(&ch.attrs);
            }
            // MIR: `Command`.
            Item::Command(c) => self.attrs(&c.attrs),
            // MIR: `Function`.
            Item::Fn(f) => {
                self.generics(&f.generics);
                self.params(&f.params);
                self.opt_ty(f.ret.as_ref());
                self.block(&f.body);
            }
            // MIR: `Native`.
            Item::Native(n) => {
                self.generics(&n.generics);
                self.params(&n.params);
                self.ty(&n.ret);
            }
            // MIR: `Block`.
            Item::Block(b) => {
                self.generics(&b.generics);
                self.params(&b.params);
                for v in &b.vars {
                    self.opt_ty(v.ty.as_ref());
                    self.expr(&v.value);
                }
                if let Some(s) = &b.step {
                    self.params(&s.params);
                    self.ty(&s.ret);
                    self.opt_expr(s.requires.as_ref());
                    self.opt_expr(s.ensures.as_ref());
                    self.block(&s.body);
                }
                for m in &b.methods {
                    self.params(&m.params);
                    self.opt_ty(m.ret.as_ref());
                    self.block(&m.body);
                }
            }
            // MIR: Maschine mit ihren Merkmalen (`Template`, `DriverMachine`, …).
            Item::Machine(m) => {
                self.params(&m.params);
                self.attrs(&m.attrs);
                self.machine_body(&m.body);
            }
            // MIR: `Instance`.
            Item::Instance(i) => self.instance(i),
            // MIR: `Scenario`.
            Item::Scenario(s) => {
                self.attrs(&s.attrs);
                self.machine_body(&s.body);
            }
            // MIR: `Campaign`.
            Item::Campaign(c) => {
                for i in &c.items {
                    match i {
                        CampaignItem::SweepRange { from, to, step, .. } => {
                            self.expr(from);
                            self.expr(to);
                            self.expr(step);
                        }
                        CampaignItem::SweepList { values, .. } => values.iter().for_each(|v| self.expr(v)),
                        CampaignItem::Program(_)
                        | CampaignItem::Profile(_)
                        | CampaignItem::Repeat(_)
                        | CampaignItem::StopOn(_) => {}
                    }
                }
            }
            // MIR: `Trigger`.
            Item::Trigger(t) => {
                self.guard(&t.when);
                self.expr(&t.then.time);
                self.block(&t.then.body);
            }
        }
    }

    fn generics(&mut self, generics: &[GenericVar]) {
        for g in generics {
            match g {
                GenericVar::Unit(_) => self.add(Sugar::GenericUnit),
                GenericVar::Type { .. } => self.add(Sugar::GenericType),
                GenericVar::Const { range, .. } => {
                    self.add(Sugar::GenericConst);
                    self.range(range.as_ref());
                }
            }
        }
    }

    fn params(&mut self, params: &[Param]) {
        for p in params {
            self.ty(&p.ty);
            self.opt_expr(p.default.as_ref());
        }
    }

    fn fields(&mut self, fields: &[Field]) {
        for f in fields {
            self.ty(&f.ty);
            self.opt_expr(f.value.as_ref());
        }
    }

    fn attrs(&mut self, attrs: &[Attr]) {
        for a in attrs {
            // Die Attribute selbst leitet die Inventur ab; hier zaehlen nur
            // die Ausdruecke darin.
            match &a.kind {
                AttrKind::Safe(e) | AttrKind::Rate(e) | AttrKind::MaxRate(e) | AttrKind::MaxSlew(e) => self.expr(e),
                AttrKind::Budget(items) => items.iter().for_each(|b| self.expr(&b.value)),
                AttrKind::MaxAge(_)
                | AttrKind::Capacity(_)
                | AttrKind::Framing(_)
                | AttrKind::Overflow(_)
                | AttrKind::Wake(_)
                | AttrKind::Jitter(_)
                | AttrKind::Debounce(_)
                | AttrKind::CapacityBytes(_)
                | AttrKind::ExpectLen(_)
                | AttrKind::Irreversible
                | AttrKind::Label(_)
                | AttrKind::Display(_)
                | AttrKind::Group(_)
                | AttrKind::Doc(_)
                | AttrKind::PollingUnchecked
                | AttrKind::FaultIsFail(_) => {}
            }
        }
    }

    fn instance(&mut self, i: &InstanceDecl) {
        self.range(i.index.as_ref().map(|(_, r)| r));
        self.args(&i.args);
    }

    fn machine_body(&mut self, body: &MachineBody) {
        for p in &body.prelude {
            match p {
                // MIR: Variable der Maschine.
                MachinePrelude::Var(v) => self.var(v),
                // MIR: `Persist`.
                MachinePrelude::Persist(p) => {
                    self.ty(&p.ty);
                    self.expr(&p.value);
                }
                // MIR: `Signal`, `TargetFault`.
                MachinePrelude::Signal(_) | MachinePrelude::Fault(_) => {}
            }
        }
        self.opt_block(body.loop_block.as_ref());
        body.handlers.iter().for_each(|h| self.handler(h));
        body.states.iter().for_each(|s| self.state(s));
    }

    fn state(&mut self, s: &StateDecl) {
        self.attrs(&s.attrs);
        let b = &s.body;
        for p in &b.prelude {
            match p {
                // MIR: `StateFaultTarget`.
                StatePrelude::Fault(_) => {}
                StatePrelude::Var(v) => {
                    self.add(Sugar::StateVar);
                    self.var(v);
                }
                // MIR: `ScopedInstance`.
                StatePrelude::Instance(i) => self.instance(i),
            }
        }
        self.opt_block(b.enter.as_ref());
        self.opt_block(b.loop_block.as_ref());
        b.handlers.iter().for_each(|h| self.handler(h));
        if let Some(items) = &b.sequence {
            self.seq_items(items);
        }
        if let Some(t) = &b.sequence_timeout {
            self.add(Sugar::SequenceTimeout);
            self.timeout(t);
        }
        for t in &b.transitions {
            match &t.trigger {
                Trigger::When(g) => self.guard(g),
                Trigger::After(e) => self.expr(e),
            }
            t.actions.iter().for_each(|s| self.stmt(s));
        }
        self.opt_block(b.exit.as_ref());
        b.states.iter().for_each(|s| self.state(s));
    }

    fn handler(&mut self, h: &OnHandler) {
        if let Some((_, p)) = &h.pattern {
            self.pattern(p);
        }
        self.opt_expr(h.guard.as_ref());
        self.block(&h.body);
    }

    fn guard(&mut self, g: &Guard) {
        match g {
            Guard::Expr(e) | Guard::Next { subject: e, .. } => self.expr(e),
        }
    }

    fn timeout(&mut self, t: &Timeout) {
        self.expr(&t.duration);
        match &t.action {
            // MIR: `TargetFault`, `TargetState`.
            TimeoutAction::Fault | TimeoutAction::Goto(_) => {}
            TimeoutAction::Else(b) => {
                self.add(Sugar::TimeoutElse);
                self.block(b);
            }
        }
    }

    // MIR: jede Art eines Glieds hat ihre Kennung (`Seq`).
    fn seq_items(&mut self, items: &[SeqItem]) {
        for i in items {
            match i {
                SeqItem::Stmt(s) => self.stmt(s),
                SeqItem::Wait(e) => self.expr(e),
                SeqItem::Until { guard, timeout, .. } => {
                    self.guard(guard);
                    if let Some(t) = timeout {
                        self.timeout(t);
                    }
                }
                SeqItem::Expect { cond, .. } => self.expr(cond),
                SeqItem::Repeat { count, body, .. } => {
                    self.expr(count);
                    self.seq_items(body);
                }
                SeqItem::Step { body, .. } => self.seq_items(body),
            }
        }
    }

    fn var(&mut self, v: &VarDecl) {
        self.opt_ty(v.ty.as_ref());
        self.expr(&v.value);
    }

    fn opt_block(&mut self, b: Option<&Block>) {
        if let Some(b) = b {
            self.block(b);
        }
    }

    fn block(&mut self, b: &Block) {
        b.stmts.iter().for_each(|s| self.stmt(s));
    }

    fn stmt(&mut self, s: &Stmt) {
        match &s.kind {
            StmtKind::Assign { target, op, value } => {
                match op {
                    // MIR: `Assign`.
                    AssignOp::Set => {}
                    AssignOp::Add => self.add(Sugar::AssignAdd),
                    AssignOp::Sub => self.add(Sugar::AssignSub),
                    AssignOp::Mul => self.add(Sugar::AssignMul),
                    AssignOp::Div => self.add(Sugar::AssignDiv),
                }
                self.expr(target);
                self.expr(value);
            }
            StmtKind::Var(v) => {
                self.add(Sugar::LocalVar);
                self.var(v);
            }
            // MIR: `Job`, `Arm`, `Goto`, `Cancel`, `Raise`, `Break`, `Pass`.
            StmtKind::Job { args, .. } => self.args(args),
            StmtKind::Arm { .. }
            | StmtKind::Goto(_)
            | StmtKind::Cancel(_)
            | StmtKind::Raise(_)
            | StmtKind::Break
            | StmtKind::Pass => {}
            // MIR: `Check` mit `CheckConfirm`, `CheckWithin`, `TargetFault`.
            StmtKind::Check { cond, confirm, within, .. } => {
                self.expr(cond);
                self.opt_expr(confirm.as_ref());
                self.opt_expr(within.as_ref());
            }
            StmtKind::Alert { cond, confirm, .. } => {
                self.add(Sugar::Alert);
                self.expr(cond);
                self.opt_expr(confirm.as_ref());
            }
            StmtKind::Log(_) => self.add(Sugar::Log),
            // MIR: `Abort`.
            StmtKind::Abort(_) => {}
            // MIR: `Return`, `Send`.
            StmtKind::Return(e) | StmtKind::Send { value: e, .. } => self.expr(e),
            StmtKind::Pulse { value, duration, .. } => {
                self.add(Sugar::Pulse);
                self.expr(value);
                self.expr(duration);
            }
            StmtKind::Measure { value, .. } => {
                self.add(Sugar::Measure);
                self.expr(value);
            }
            StmtKind::Verify { cond, .. } => {
                self.add(Sugar::Verify);
                self.expr(cond);
            }
            StmtKind::Verdict { .. } => self.add(Sugar::Verdict),
            // MIR: `MethodCall` oder eine Zuweisung aus dem Aufruf.
            StmtKind::Expr(e) => self.expr(e),
            // MIR: `If`.
            StmtKind::If { branches, otherwise } => {
                if branches.len() > 1 {
                    self.add(Sugar::Elif);
                }
                for (cond, body) in branches {
                    self.expr(cond);
                    self.block(body);
                }
                self.opt_block(otherwise.as_ref());
            }
            StmtKind::For { target, iter, body } => {
                match target {
                    // MIR: `ForRange`, `ForEach`.
                    ForTarget::One(_) => {}
                    ForTarget::Pair(..) => self.add(Sugar::ForPair),
                }
                match iter {
                    ForIter::Range(e) | ForIter::Expr(e) => self.expr(e),
                }
                self.block(body);
            }
            StmtKind::Match { subject, cases } => {
                self.expr(subject);
                for case in cases {
                    match &case.pattern {
                        // MIR: `ArmVariant`, `ArmWild`.
                        CasePattern::Variant { .. } | CasePattern::Wild => {}
                        // MIR: `ArmValues`; ein Bereich darunter ist eine eigene Form.
                        CasePattern::Values(values) => {
                            for v in values {
                                self.expr(&v.from);
                                if let Some(to) = &v.to {
                                    self.add(Sugar::CaseRange);
                                    self.expr(to);
                                }
                            }
                        }
                    }
                    self.block(&case.body);
                }
            }
            // MIR: `At`, `Every`.
            StmtKind::At(at) => {
                self.expr(&at.time);
                self.block(&at.body);
            }
            StmtKind::Every { period, body } => {
                self.expr(period);
                self.block(body);
            }
        }
    }

    fn args(&mut self, args: &[Arg]) {
        if args.iter().any(|a| a.name.is_some()) {
            self.add(Sugar::NamedArgs);
        }
        args.iter().for_each(|a| self.expr(&a.value));
    }

    fn opt_expr(&mut self, e: Option<&Expr>) {
        if let Some(e) = e {
            self.expr(e);
        }
    }

    fn expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Number { unit, .. } => {
                if unit.is_some() {
                    self.add(Sugar::UnitLiteral);
                }
            }
            // MIR: `Duration`, `Str`, `Bool`, `None`, `Default`, Namen.
            ExprKind::Duration(_)
            | ExprKind::Str(_)
            | ExprKind::Bool(_)
            | ExprKind::None
            | ExprKind::Default
            | ExprKind::Ident(_) => {}
            // MIR: `Call`, `NativeCall`, `BlockInit`.
            ExprKind::Call { generics, args, .. } => {
                if !generics.is_empty() {
                    self.add(Sugar::ExplicitGenerics);
                }
                for g in generics {
                    match g {
                        GenericArg::Unit(_) => {}
                        GenericArg::Type(t) => self.ty(t),
                        GenericArg::Const(c) => self.expr(c),
                    }
                }
                self.args(args);
            }
            // MIR: `Variant`, `Record`, `Param`, `StateOf`.
            ExprKind::Upper { args, .. } | ExprKind::TypeName { args, .. } => {
                if let Some(a) = args {
                    self.args(a);
                }
            }
            ExprKind::Paren(x) => {
                self.add(Sugar::Paren);
                self.expr(x);
            }
            // MIR: `Tuple`, `Array`.
            ExprKind::Tuple(a, b) => {
                self.expr(a);
                self.expr(b);
            }
            ExprKind::Array(items) => items.iter().for_each(|x| self.expr(x)),
            ExprKind::InstanceArray { count, args, .. } => {
                self.add(Sugar::InstanceArray);
                self.expr(count);
                self.args(args);
            }
            // MIR: `Field`, `Accessor`, `MethodCall`, `Call`.
            ExprKind::Member { base, args, .. } => {
                self.expr(base);
                if let Some(a) = args {
                    self.args(a);
                }
            }
            // MIR: `Index`, `Slice`, `Index2`.
            ExprKind::Index { base, index } => {
                self.expr(base);
                self.expr(index);
            }
            ExprKind::Slice { base, from: a, to: b } | ExprKind::Index2 { base, row: a, col: b } => {
                self.expr(base);
                self.expr(a);
                self.expr(b);
            }
            // MIR: `Cast`, `Convert`.
            ExprKind::Cast { expr, ty } => {
                self.expr(expr);
                self.scalar(ty);
            }
            // MIR: `Unary`, `Binary`.
            ExprKind::Unary { expr, .. } => self.expr(expr),
            ExprKind::Binary { lhs, rhs, .. } => {
                self.expr(lhs);
                self.expr(rhs);
            }
            // MIR: `Matches`.
            ExprKind::Match { subject, pattern, .. } => {
                self.expr(subject);
                self.pattern(pattern);
            }
            // MIR: `Cond`.
            ExprKind::Conditional { then, cond, otherwise } => {
                self.expr(then);
                self.expr(cond);
                self.expr(otherwise);
            }
            // MIR: `Temporal`.
            ExprKind::Temporal { inner, .. } => self.expr(inner),
            ExprKind::Implies { lhs, rhs } => {
                self.add(Sugar::Implies);
                self.expr(lhs);
                self.expr(rhs);
            }
        }
    }

    fn pattern(&mut self, p: &Pattern) {
        match p {
            Pattern::Text(_) => {}
            Pattern::Record { fields, .. } => fields.iter().for_each(|(_, e)| self.expr(e)),
        }
    }

    fn range(&mut self, r: Option<&Range>) {
        if let Some(r) = r {
            self.expr(&r.from);
            self.expr(&r.to);
        }
    }

    fn opt_ty(&mut self, t: Option<&Type>) {
        if let Some(t) = t {
            self.ty(t);
        }
    }

    fn scalar(&mut self, s: &ScalarType) {
        if let ScalarType::Str(e) = s {
            self.expr(e);
        }
    }

    fn elem(&mut self, e: &ElemType) {
        match e {
            ElemType::Bytes(x) | ElemType::Line(x) => self.expr(x),
            ElemType::Capture { elem, len } => {
                self.ty(elem);
                self.expr(len);
            }
            ElemType::U8 | ElemType::Edge | ElemType::Named(_) => {}
        }
    }

    fn ty(&mut self, t: &Type) {
        match &t.kind {
            // MIR: die Typen (`Type`).
            TypeKind::Scalar { scalar, range, .. } => {
                self.scalar(scalar);
                self.range(range.as_ref());
            }
            TypeKind::Array { len, elem } | TypeKind::Vec { elem, len } | TypeKind::Samples { elem, len } => {
                self.expr(len);
                self.ty(elem);
            }
            TypeKind::Bytes(e) | TypeKind::Line(e) => self.expr(e),
            TypeKind::Stream(e) => self.elem(e),
            TypeKind::Table { key, value } => {
                self.ty(key);
                self.ty(value);
            }
            TypeKind::Map { key, value, len } => {
                self.ty(key);
                self.ty(value);
                self.expr(len);
            }
            TypeKind::Mat { rows, cols, .. } => {
                self.expr(rows);
                self.expr(cols);
            }
            TypeKind::MatDim { .. } => self.add(Sugar::MatDim),
            TypeKind::VecDim(_) => self.add(Sugar::VecDim),
            TypeKind::Wrapped { inner, .. } => self.ty(inner),
            TypeKind::Named { .. } | TypeKind::TypeVar { .. } => {}
        }
    }
}
