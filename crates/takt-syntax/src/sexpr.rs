//! Kompakte Ausgabe des Syntaxbaums als S-Expressions, ohne Spannen.
//!
//! Deklarationen, Bloecke und Anweisungen stehen je auf einer Zeile und werden
//! durch Einrueckung geschachtelt; Ausdruecke, Typen und Einheiten sind
//! einzeilig in Praefixnotation (`(and cmd_start (>= v_dc V_DC_MIN))`). Die
//! Form ist stabil und dient Lesern, Golden-Tests und dem Vergleich zweier
//! Parser; sie ist keine Takt-Syntax.

use std::fmt::Write;

use crate::ast::*;

/// Gibt eine Datei aus.
pub fn file(f: &File) -> String {
    let mut p = Printer::default();
    for item in &f.items {
        p.item(item);
    }
    p.finish()
}

/// Gibt einen Schnipsel aus.
pub fn snippet(items: &[SnippetItem]) -> String {
    let mut p = Printer::default();
    for item in items {
        match item {
            SnippetItem::Item(i) => p.item(i),
            SnippetItem::MachinePrelude(m) => p.machine_prelude(m),
            SnippetItem::Initial(name) => p.line(&format!("(initial {})", name.name)),
            SnippetItem::Enter(b) => p.node("enter", "", |p| p.block(b)),
            SnippetItem::Exit(b) => p.node("exit", "", |p| p.block(b)),
            SnippetItem::Loop(b) => p.node("loop", "", |p| p.block(b)),
            SnippetItem::On(h) => p.on_handler(h),
            SnippetItem::Sequence(items) => p.node("sequence", "", |p| p.seq_items(items)),
            SnippetItem::Transition(t) => p.transition(t),
            SnippetItem::State(s) => p.state(s),
            SnippetItem::Step(s) => p.step(s),
            SnippetItem::Seq(s) => p.seq_item(s),
            SnippetItem::Instance(i) => p.instance(i),
        }
    }
    p.finish()
}

#[derive(Default)]
struct Printer {
    out: String,
    depth: usize,
}

impl Printer {
    fn finish(mut self) -> String {
        self.out.push('\n');
        self.out
    }

    /// Beginnt eine neue Zeile auf der aktuellen Tiefe.
    fn newline(&mut self) {
        if !self.out.is_empty() {
            self.out.push('\n');
        }
        for _ in 0..self.depth {
            self.out.push_str("  ");
        }
    }

    /// Eine ganze Zeile.
    fn line(&mut self, text: &str) {
        self.newline();
        self.out.push_str(text);
    }

    /// `(head attrs` … Kinder … `)`; ohne Kinder bleibt alles auf einer Zeile.
    fn node(&mut self, head: &str, attrs: &str, children: impl FnOnce(&mut Self)) {
        self.newline();
        self.out.push('(');
        self.out.push_str(head);
        if !attrs.is_empty() {
            self.out.push(' ');
            self.out.push_str(attrs);
        }
        self.depth += 1;
        children(self);
        self.depth -= 1;
        self.out.push(')');
    }

    // ------------------------------------------------------------ Deklarationen

    fn item(&mut self, item: &Item) {
        match item {
            Item::Import(Import::Module { path, alias, .. }) => {
                let path = path.iter().map(|i| i.name.as_str()).collect::<Vec<_>>().join(".");
                let alias = alias.as_ref().map(|a| format!(" as {}", a.name)).unwrap_or_default();
                self.line(&format!("(import {path}{alias})"));
            }
            Item::Import(Import::Channels { file, .. }) => self.line(&format!("(import-channels {})", string(file))),
            Item::System(s) => self.node("system", "", |p| {
                for item in &s.items {
                    p.line(&system_item(item));
                }
            }),
            Item::Type(t) => self.line(&format!("(type {} {})", t.name.name, ty(&t.ty))),
            Item::Unitvec(u) => {
                let units = u.units.iter().map(unit_expr).collect::<Vec<_>>().join(" ");
                self.line(&format!("(unitvec {} {units})", u.name.name));
            }
            Item::Enum(e) => {
                let mut attrs = e.name.name.clone();
                if let Some(l) = e.layout {
                    let _ = write!(attrs, " layout={}", int_type(l));
                }
                if e.open {
                    attrs.push_str(" open");
                }
                self.node("enum", &attrs, |p| {
                    for v in &e.variants {
                        let mut s = format!("({}", v.name.name);
                        if let Some(d) = &v.discriminant {
                            let _ = write!(s, " ={}", d.text);
                        }
                        for f in &v.fields {
                            let _ = write!(s, " ({})", field(f));
                        }
                        s.push(')');
                        p.line(&s);
                    }
                });
            }
            Item::Record(r) => {
                let mut attrs = r.name.name.clone();
                if let Some(l) = &r.layout {
                    let endian = match l.endian {
                        Endian::Little => "little",
                        Endian::Big => "big",
                    };
                    let _ = write!(attrs, " layout={endian}");
                    if let Some(a) = &l.align {
                        let _ = write!(attrs, " align={}", a.text);
                    }
                }
                self.node("record", &attrs, |p| {
                    for f in &r.fields {
                        match f {
                            RecordField::Plain(f) => p.line(&format!("({})", field(f))),
                            RecordField::Bits { name, ty, bits, .. } => {
                                p.node("bits", &format!("{}: {}", name.name, int_type(*ty)), |p| {
                                    for b in bits {
                                        let t = match &b.ty {
                                            BitType::Bool => "bool".to_string(),
                                            BitType::Int(i) => int_type(*i).to_string(),
                                        };
                                        let to = b.to.as_ref().map(|t| format!("..{}", t.text)).unwrap_or_default();
                                        p.line(&format!("({}: {t} at {}{to})", b.name.name, b.from.text));
                                    }
                                });
                            }
                        }
                    }
                });
            }
            Item::Unit(UnitDecl::Scaled { name, factor, unit, .. }) => {
                let unit = unit.as_ref().map(|u| format!(" {}", unit_expr(u))).unwrap_or_default();
                self.line(&format!("(unit {} {}{unit})", name.name, number(factor)));
            }
            Item::Unit(UnitDecl::Affine { name, base, offset, .. }) => {
                self.line(&format!("(unit {} affine {} {})", name.name, unit_expr(base), number(offset)));
            }
            Item::Stream(s) => {
                self.line(&format!("(stream {} :{}{})", s.name.name, elem_type(&s.elem), attrs(&s.attrs)));
            }
            Item::Port(p) => self.line(&format!("(port {} :{} @mmio({}))", p.name.name, p.regs.name, p.address.text)),
            Item::Node(n) => {
                let tick = n.tick.as_ref().map(|t| format!(" tick={}", duration(t))).unwrap_or_default();
                self.line(&format!("(node {} @hw({}){tick})", n.name.name, string(&n.address)));
            }
            Item::Property(p) => {
                let monitor = if p.monitor { " monitor" } else { "" };
                self.line(&format!("({} {}{monitor} {})", p.kind.word(), p.name.name, expr(&p.prop)));
            }
            Item::Const(c) => {
                let t = c.ty.as_ref().map(|t| format!(" :{}", ty(t))).unwrap_or_default();
                self.line(&format!("(const {}{t} {})", c.name.name, expr(&c.value)));
            }
            Item::Param(p) => {
                let head = if p.tunable { "tunable-param" } else { "param" };
                self.line(&format!("({head} {} :{} {}{})", p.name.name, ty(&p.ty), expr(&p.value), attrs(&p.attrs)));
            }
            Item::Profile(p) => self.node("profile", &p.name.name, |pr| {
                for (name, value) in &p.entries {
                    pr.line(&format!("({} {})", name.name, expr(value)));
                }
            }),
            Item::Channel(c) => {
                let dir = match c.dir {
                    Direction::Input => "input",
                    Direction::Output => "output",
                };
                let binding = match &c.binding {
                    Binding::Hw(s) => format!("@hw({})", string(s)),
                    Binding::Sim(s) => format!("@sim({})", string(s)),
                    Binding::None => "@none".to_string(),
                };
                self.line(&format!("({dir} {} :{} {binding}{})", c.name.name, ty(&c.ty), attrs(&c.attrs)));
            }
            Item::Command(c) => self.line(&format!("(command {}{})", c.name.name, attrs(&c.attrs))),
            Item::Fn(f) => {
                let ret = f.ret.as_ref().map(|t| format!(" -> {}", ty(t))).unwrap_or_default();
                let head = format!("{}{} {}{ret}", f.name.name, generic_vars(&f.generics), params(&f.params));
                self.node("fn", &head, |p| p.block(&f.body));
            }
            Item::Native(n) => {
                let head = match n.kind {
                    NativeKind::Fn => "native-fn",
                    NativeKind::Job => "native-job",
                };
                let mut s =
                    format!("{}{} {} -> {}", n.name.name, generic_vars(&n.generics), params(&n.params), ty(&n.ret));
                if let Some(f) = &n.from {
                    let _ = write!(s, " from={}", string(f));
                }
                let cost = match &n.cost {
                    CostSpec::Single(i) => i.text.clone(),
                    CostSpec::Classes(c) => {
                        let parts = c.iter().map(|(k, v)| format!("{}:{}", cost_class(*k), v.text)).collect::<Vec<_>>();
                        format!("{{{}}}", parts.join(" "))
                    }
                };
                let _ = write!(s, " cost={cost} stack={}", n.stack.text);
                if let Some(d) = &n.duration {
                    let _ = write!(s, " duration={}", duration(d));
                }
                self.line(&format!("({head} {s})"));
            }
            Item::Block(b) => {
                let head = format!("{}{} {}", b.name.name, generic_vars(&b.generics), params(&b.params));
                self.node("block", &head, |p| {
                    for v in &b.vars {
                        p.var_decl(v);
                    }
                    if let Some(s) = &b.step {
                        p.step(s);
                    }
                    for m in &b.methods {
                        let ret = m.ret.as_ref().map(|t| format!(" -> {}", ty(t))).unwrap_or_default();
                        p.node("method", &format!("{} {}{ret}", m.name.name, params(&m.params)), |p| p.block(&m.body));
                    }
                });
            }
            Item::Machine(m) => {
                let mut head = if m.driver { "driver-machine ".to_string() } else { "machine ".to_string() };
                head.push_str(&m.name.name);
                if !m.params.is_empty() {
                    let _ = write!(head, " {}", params(&m.params));
                }
                if !m.follows.is_empty() {
                    let names = m.follows.iter().map(|i| i.name.as_str()).collect::<Vec<_>>().join(",");
                    let _ = write!(head, " follows={names}");
                }
                if let Some(n) = &m.node {
                    let _ = write!(head, " node={}", n.name);
                }
                if let Some(e) = &m.every {
                    let _ = write!(head, " every={}", duration(e));
                }
                if let Some(ph) = &m.phase {
                    let _ = write!(head, " phase={}", duration(ph));
                }
                head.push_str(&attrs(&m.attrs));
                let (kind, rest) = head.split_once(' ').unwrap_or((&head, ""));
                let (kind, rest) = (kind.to_string(), rest.to_string());
                self.node(&kind, &rest, |p| p.machine_body(&m.body));
            }
            Item::Instance(i) => self.instance(i),
            Item::Scenario(s) => {
                let every = s.every.as_ref().map(|d| format!(" every={}", duration(d))).unwrap_or_default();
                self.node("scenario", &format!("{}{every}", string(&s.name)), |p| p.machine_body(&s.body));
            }
            Item::Campaign(c) => self.node("campaign", &c.name.name, |p| {
                for item in &c.items {
                    p.line(&campaign_item(item));
                }
            }),
            Item::Trigger(t) => {
                let mut head = t.name.name.clone();
                if let Some(n) = &t.node {
                    let _ = write!(head, " node={}", n.name);
                }
                let _ = write!(head, " bound={}", duration(&t.bound));
                self.node("trigger", &head, |p| {
                    p.line(&format!("(when {})", guard(&t.when)));
                    p.node("then", "", |p| p.at_stmt(&t.then));
                });
            }
        }
    }

    fn step(&mut self, s: &StepDecl) {
        self.node("step", &format!("{} -> {}", params(&s.params), ty(&s.ret)), |p| p.block(&s.body));
    }

    fn instance(&mut self, i: &InstanceDecl) {
        let mut s = format!("(instance {}", i.name.name);
        if let Some((var, range)) = &i.index {
            let _ = write!(s, " [{} in {}]", var.name, range_str(range));
        }
        if i.resume {
            s.push_str(" resume");
        }
        let _ = write!(s, " {}{})", i.template.name, args(&i.args));
        self.line(&s);
    }

    // ------------------------------------------------------------ Maschinen

    fn machine_body(&mut self, b: &MachineBody) {
        for m in &b.prelude {
            self.machine_prelude(m);
        }
        self.line(&format!("(initial {})", b.initial.name));
        if let Some(l) = &b.loop_block {
            self.node("loop", "", |p| p.block(l));
        }
        for h in &b.handlers {
            self.on_handler(h);
        }
        for s in &b.states {
            self.state(s);
        }
    }

    fn machine_prelude(&mut self, m: &MachinePrelude) {
        match m {
            MachinePrelude::Var(v) => self.var_decl(v),
            MachinePrelude::Persist(p) => {
                let mi = p.min_interval.as_ref().map(|d| format!(" min_interval={}", duration(d))).unwrap_or_default();
                self.line(&format!("(persist {} :{} {}{mi})", p.name.name, ty(&p.ty), expr(&p.value)));
            }
            MachinePrelude::Signal(s) => self.line(&format!("(signal {})", s.name)),
            MachinePrelude::Fault(f) => self.line(&format!("(fault -> {})", f.name)),
        }
    }

    fn state(&mut self, s: &StateDecl) {
        let mut head = s.name.name.clone();
        if s.idle {
            head.push_str(" idle");
        }
        if s.resume {
            head.push_str(" resume");
        }
        head.push_str(&attrs(&s.attrs));
        self.node("state", &head, |p| {
            let b = &s.body;
            for pre in &b.prelude {
                match pre {
                    StatePrelude::Fault(f) => p.line(&format!("(fault -> {})", f.name)),
                    StatePrelude::Var(v) => p.var_decl(v),
                    StatePrelude::Instance(i) => p.instance(i),
                }
            }
            if let Some(i) = &b.initial {
                p.line(&format!("(initial {})", i.name));
            }
            if let Some(e) = &b.enter {
                p.node("enter", "", |p| p.block(e));
            }
            if let Some(l) = &b.loop_block {
                p.node("loop", "", |p| p.block(l));
            }
            for h in &b.handlers {
                p.on_handler(h);
            }
            if let Some(seq) = &b.sequence {
                let head = match &b.sequence_timeout {
                    Some(Timeout { duration, action: TimeoutAction::Goto(t) }) => {
                        format!("timeout={} -> {}", expr(duration), t.name)
                    }
                    Some(Timeout { duration, .. }) => format!("timeout={}", expr(duration)),
                    None => String::new(),
                };
                p.node("sequence", &head, |p| p.seq_items(seq));
            }
            for t in &b.transitions {
                p.transition(t);
            }
            if let Some(e) = &b.exit {
                p.node("exit", "", |p| p.block(e));
            }
            for child in &b.states {
                p.state(child);
            }
        });
    }

    fn on_handler(&mut self, h: &OnHandler) {
        let mut head = h.stream.name.clone();
        if let Some((kind, pat)) = &h.pattern {
            let _ = write!(head, " {} {}", match_kind(*kind), pattern(pat));
        }
        if let Some(b) = &h.binding {
            let _ = write!(head, " as {}", b.name);
        }
        if let Some(g) = &h.guard {
            let _ = write!(head, " when {}", expr(g));
        }
        self.node("on", &head, |p| p.block(&h.body));
    }

    fn transition(&mut self, t: &Transition) {
        let (kind, trigger) = match &t.trigger {
            Trigger::When(g) => ("when", guard(g)),
            Trigger::After(e) => ("after", expr(e)),
        };
        self.node(kind, &format!("{trigger} -> {}", t.target.name), |p| {
            for s in &t.actions {
                p.stmt(s);
            }
        });
    }

    fn seq_items(&mut self, items: &[SeqItem]) {
        for item in items {
            self.seq_item(item);
        }
    }

    fn seq_item(&mut self, item: &SeqItem) {
        match item {
            SeqItem::Stmt(s) => self.stmt(s),
            SeqItem::Wait(e) => self.line(&format!("(wait {})", expr(e))),
            SeqItem::Until { guard: g, timeout, .. } => {
                let mut head = guard(g);
                match timeout {
                    None => self.line(&format!("(until {head})")),
                    Some(Timeout { duration: d, action }) => {
                        let _ = write!(head, " timeout={}", expr(d));
                        match action {
                            TimeoutAction::Fault => self.line(&format!("(until {head})")),
                            TimeoutAction::Goto(t) => self.line(&format!("(until {head} -> {})", t.name)),
                            TimeoutAction::Else(b) => {
                                self.node("until", &head, |p| p.node("else", "", |p| p.block(b)));
                            }
                        }
                    }
                }
            }
            SeqItem::Expect { cond, message, .. } => {
                let m = message.as_ref().map(|m| format!(" {}", string(m))).unwrap_or_default();
                self.line(&format!("(expect {}{m})", expr(cond)));
            }
            SeqItem::Repeat { count, body, .. } => self.node("repeat", &expr(count), |p| p.seq_items(body)),
            SeqItem::Step { name, body, .. } => self.node("step", &string(name), |p| p.seq_items(body)),
        }
    }

    // ------------------------------------------------------------ Anweisungen

    fn block(&mut self, b: &Block) {
        for s in &b.stmts {
            self.stmt(s);
        }
    }

    fn var_decl(&mut self, v: &VarDecl) {
        let head = if v.public { "pub-var" } else { "var" };
        let t = v.ty.as_ref().map(|t| format!(" :{}", ty(t))).unwrap_or_default();
        self.line(&format!("({head} {}{t} {})", v.name.name, expr(&v.value)));
    }

    fn at_stmt(&mut self, a: &AtStmt) {
        self.node("at", &expr(&a.time), |p| p.block(&a.body));
    }

    fn stmt(&mut self, s: &Stmt) {
        match &s.kind {
            StmtKind::Assign { target, op, value } => {
                let op = match op {
                    AssignOp::Set => "=",
                    AssignOp::Add => "+=",
                    AssignOp::Sub => "-=",
                    AssignOp::Mul => "*=",
                    AssignOp::Div => "/=",
                };
                self.line(&format!("({op} {} {})", expr(target), expr(value)));
            }
            StmtKind::Var(v) => self.var_decl(v),
            StmtKind::Job { handle, callee, args: a } => {
                self.line(&format!("(job {} {}{})", handle.name, callee.name, args(a)));
            }
            StmtKind::Arm { arm, trigger } => {
                self.line(&format!("({} {})", if *arm { "arm" } else { "disarm" }, trigger.name));
            }
            StmtKind::Check { cond, message, confirm, within, target, req } => {
                let mut s = format!("(check {}", expr(cond));
                if let Some(m) = message {
                    let _ = write!(s, " {}", string(m));
                }
                if let Some(c) = confirm {
                    let _ = write!(s, " for={}", expr(c));
                }
                if let Some(w) = within {
                    let _ = write!(s, " within={}", expr(w));
                }
                if let Some(t) = target {
                    let _ = write!(s, " -> {}", t.name);
                }
                if let Some(r) = req {
                    let _ = write!(s, " req={}", string(r));
                }
                s.push(')');
                self.line(&s);
            }
            StmtKind::Alert { cond, message, confirm } => {
                let c = confirm.as_ref().map(|c| format!(" for={}", expr(c))).unwrap_or_default();
                self.line(&format!("(alert {} {}{c})", expr(cond), string(message)));
            }
            StmtKind::Log(m) => self.line(&format!("(log {})", string(m))),
            StmtKind::Goto(t) => self.line(&format!("(-> {})", t.name)),
            StmtKind::Abort(m) => {
                let m = m.as_ref().map(|m| format!(" {}", string(m))).unwrap_or_default();
                self.line(&format!("(abort{m})"));
            }
            StmtKind::Return(e) => self.line(&format!("(return {})", expr(e))),
            StmtKind::Send { stream, value } => self.line(&format!("(send {} {})", stream.name, expr(value))),
            StmtKind::Pulse { output, value, duration: d } => {
                self.line(&format!("(pulse {} {} for={})", output.name, expr(value), expr(d)));
            }
            StmtKind::Cancel(h) => self.line(&format!("(cancel {})", h.name)),
            StmtKind::Measure { name, value } => self.line(&format!("(measure {} {})", name.name, expr(value))),
            StmtKind::Verify { cond, message, req } => {
                let r = req.as_ref().map(|r| format!(" req={}", string(r))).unwrap_or_default();
                self.line(&format!("(verify {} {}{r})", expr(cond), string(message)));
            }
            StmtKind::Verdict { pass, message } => {
                let m = message.as_ref().map(|m| format!(" {}", string(m))).unwrap_or_default();
                self.line(&format!("(verdict {}{m})", if *pass { "pass" } else { "fail" }));
            }
            StmtKind::Raise(s) => self.line(&format!("(raise {})", s.name)),
            StmtKind::Break => self.line("(break)"),
            StmtKind::Pass => self.line("(pass)"),
            StmtKind::Expr(e) => self.line(&expr(e)),
            StmtKind::If { branches, otherwise } => {
                for (i, (cond, body)) in branches.iter().enumerate() {
                    self.node(if i == 0 { "if" } else { "elif" }, &expr(cond), |p| p.block(body));
                }
                if let Some(b) = otherwise {
                    self.node("else", "", |p| p.block(b));
                }
            }
            StmtKind::For { target, iter, body } => {
                let t = match target {
                    ForTarget::One(i) => i.name.clone(),
                    ForTarget::Pair(a, b) => format!("({} {})", a.name, b.name),
                };
                let it = match iter {
                    ForIter::Range(n) => format!("(range {})", expr(n)),
                    ForIter::Expr(e) => expr(e),
                };
                self.node("for", &format!("{t} in {it}"), |p| p.block(body));
            }
            StmtKind::Match { subject, cases } => self.node("match", &expr(subject), |p| {
                for c in cases {
                    let pat = match &c.pattern {
                        CasePattern::Variant { name, fields } if fields.is_empty() => name.name.clone(),
                        CasePattern::Variant { name, fields } => {
                            let f = fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>().join(" ");
                            format!("({} {f})", name.name)
                        }
                        CasePattern::Wild => "_".to_string(),
                        CasePattern::Values(values) => values
                            .iter()
                            .map(|v| match &v.to {
                                Some(to) => format!("{}..{}", expr(&v.from), expr(to)),
                                None => expr(&v.from),
                            })
                            .collect::<Vec<_>>()
                            .join(" "),
                    };
                    p.node("case", &pat, |p| p.block(&c.body));
                }
            }),
            StmtKind::At(a) => self.at_stmt(a),
            StmtKind::Every { period, body } => self.node("every", &expr(period), |p| p.block(body)),
        }
    }
}

// ---------------------------------------------------------------- Einzeilige Formen

fn system_item(item: &SystemItem) -> String {
    match item {
        SystemItem::Tick(d) => format!("(tick {})", duration(d)),
        SystemItem::OutputTiming(OutputTiming::Asap) => "(output_timing asap)".into(),
        SystemItem::OutputTiming(OutputTiming::Boundary) => "(output_timing boundary)".into(),
        SystemItem::FaultIsFail(b) => format!("(fault_is_fail {b})"),
        SystemItem::TickSource(s) => format!("(tick_source {})", string(s)),
        SystemItem::TickTolerance { value, ticks } => {
            let t = ticks.as_ref().map(|t| format!(" for={}", t.text)).unwrap_or_default();
            format!("(tick_tolerance {}{t})", expr(value))
        }
        SystemItem::Target(i) => format!("(target {})", i.name),
        SystemItem::TcbPolicy(TcbPolicy::CuratedOnly) => "(tcb_policy curated_only)".into(),
        SystemItem::TcbPolicy(TcbPolicy::Allowlist(names)) => {
            let list: Vec<&str> = names.iter().map(|n| n.name.as_str()).collect();
            format!("(tcb_policy (allowlist {}))", list.join(" "))
        }
        SystemItem::Float(w) => format!("(float {})", float_width(*w)),
        SystemItem::Language(i) => format!("(language {})", i.text),
    }
}

fn campaign_item(item: &CampaignItem) -> String {
    match item {
        CampaignItem::Program(s) => format!("(program {})", string(s)),
        CampaignItem::Profile(p) => format!("(profile {})", p.name),
        CampaignItem::SweepRange { param, from, to, step } => {
            format!("(sweep {} {}..{} step={})", param.name, expr(from), expr(to), expr(step))
        }
        CampaignItem::SweepList { param, values } => {
            let v = values.iter().map(expr).collect::<Vec<_>>().join(" ");
            format!("(sweep {} [{v}])", param.name)
        }
        CampaignItem::Repeat(n) => format!("(repeat {})", n.text),
        CampaignItem::StopOn(StopOn::Fail) => "(stop_on fail)".into(),
        CampaignItem::StopOn(StopOn::Never) => "(stop_on never)".into(),
    }
}

fn guard(g: &Guard) -> String {
    match g {
        Guard::Expr(e) => expr(e),
        Guard::Next { subject, binding } => format!("(as {} {})", expr(subject), binding.name),
    }
}

fn attrs(list: &[Attr]) -> String {
    let mut s = String::new();
    for a in list {
        s.push(' ');
        s.push_str(&match &a.kind {
            AttrKind::Safe(e) => format!("safe={}", expr(e)),
            AttrKind::Budget(items) => format!(
                "budget={{{}}}",
                items
                    .iter()
                    .map(|i| format!(
                        "{}={}",
                        match i.kind {
                            BudgetKind::Ram => "ram",
                            BudgetKind::Wcet => "wcet",
                        },
                        expr(&i.value)
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            AttrKind::MaxAge(d) => format!("max_age={}", duration(d)),
            AttrKind::Rate(e) => format!("rate={}", expr(e)),
            AttrKind::MaxRate(e) => format!("max_rate={}", expr(e)),
            AttrKind::Capacity(i) => format!("capacity={}", i.text),
            AttrKind::Framing(f) => format!(
                "framing={}",
                match f {
                    Framing::Raw => "raw".to_string(),
                    Framing::Lines => "lines".to_string(),
                    Framing::Cobs => "cobs".to_string(),
                    Framing::LengthPrefixed(w) => format!("length_prefixed({})", w.name),
                    Framing::Fixed(n) => format!("fixed({})", n.text),
                }
            ),
            AttrKind::Overflow(o) => format!(
                "overflow={}",
                match o {
                    Overflow::Fault => "fault",
                    Overflow::DropOldest => "drop_oldest",
                    Overflow::Drop => "drop",
                }
            ),
            AttrKind::Wake(b) => format!("wake={b}"),
            AttrKind::Jitter(d) => format!("jitter={}", duration(d)),
            AttrKind::MaxSlew(e) => format!("max_slew={}", expr(e)),
            AttrKind::Debounce(i) => format!("debounce={}", i.text),
            AttrKind::CapacityBytes(i) => format!("capacity_bytes={}", i.text),
            AttrKind::ExpectLen(i) => format!("expect_len={}", i.text),
            AttrKind::Irreversible => "irreversible".to_string(),
            AttrKind::Label(s) => format!("label={}", string(s)),
            AttrKind::Display(u) => format!("display={}", unit_expr(u)),
            AttrKind::Group(s) => format!("group={}", string(s)),
            AttrKind::Doc(s) => format!("doc={}", string(s)),
        });
    }
    s
}

fn generic_vars(vars: &[GenericVar]) -> String {
    if vars.is_empty() {
        return String::new();
    }
    let parts = vars
        .iter()
        .map(|v| match v {
            GenericVar::Unit(u) => u.name.clone(),
            GenericVar::Type { name, capability } => {
                let c = capability.map(|c| format!(":{}", capability_name(c))).unwrap_or_default();
                format!("type {}{c}", name.name)
            }
            GenericVar::Const { name, range } => {
                let r = range.as_ref().map(|r| format!(" in {}", range_str(r))).unwrap_or_default();
                format!("const {}{r}", name.name)
            }
        })
        .collect::<Vec<_>>();
    format!(" [{}]", parts.join(", "))
}

fn capability_name(c: Capability) -> &'static str {
    match c {
        Capability::Pod => "pod",
        Capability::Eq => "eq",
        Capability::Ord => "ord",
        Capability::Numeric => "numeric",
        Capability::Integer => "integer",
        Capability::Float => "float",
    }
}

fn params(list: &[Param]) -> String {
    let parts = list
        .iter()
        .map(|p| {
            let mut s = String::new();
            if p.inout {
                s.push_str("inout ");
            }
            let _ = write!(s, "{}:", p.name.name);
            match p.dir {
                Some(Direction::Input) => s.push_str("input "),
                Some(Direction::Output) => s.push_str("output "),
                None => {}
            }
            s.push_str(&ty(&p.ty));
            if let Some(d) = &p.default {
                let _ = write!(s, "={}", expr(d));
            }
            s
        })
        .collect::<Vec<_>>();
    format!("({})", parts.join(" "))
}

fn args(list: &[Arg]) -> String {
    let mut s = String::new();
    for a in list {
        s.push(' ');
        if let Some(n) = &a.name {
            let _ = write!(s, "{}=", n.name);
        }
        s.push_str(&expr(&a.value));
    }
    s
}

fn field(f: &Field) -> String {
    let mut s = format!("{}: {}", f.name.name, ty(&f.ty));
    if let Some(v) = &f.value {
        let _ = write!(s, " = {}", expr(v));
    }
    if let Some(o) = &f.offset {
        let _ = write!(s, " offset={}", o.text);
    }
    if let Some(l) = &f.len_field {
        let _ = write!(s, " len={}", l.name);
    }
    s
}

fn pattern(p: &Pattern) -> String {
    match p {
        Pattern::Text(s) => string(s),
        Pattern::Record { ty, fields } => {
            let f = fields.iter().map(|(n, e)| format!(" {}={}", n.name, expr(e))).collect::<String>();
            format!("({}{f})", ty.name)
        }
    }
}

fn match_kind(k: MatchKind) -> &'static str {
    match k {
        MatchKind::Matches => "matches",
        MatchKind::Has => "has",
    }
}

fn cost_class(c: CostClass) -> &'static str {
    match c {
        CostClass::I32 => "i32",
        CostClass::I64 => "i64",
        CostClass::F32 => "f32",
        CostClass::F64 => "f64",
        CostClass::Mem => "mem",
        CostClass::Call => "call",
        CostClass::Native => "native",
    }
}

/// Ausdruck in Praefixnotation.
pub fn expr(e: &Expr) -> String {
    match &e.kind {
        ExprKind::Number { value, unit } => match unit {
            Some(u) => format!("{}[{}]", number(value), unit_expr(u)),
            None => number(value),
        },
        ExprKind::Duration(d) => duration(d),
        ExprKind::Str(s) => string(s),
        ExprKind::Bool(b) => b.to_string(),
        ExprKind::None => "none".into(),
        ExprKind::Default => "default".into(),
        ExprKind::Ident(i) => i.name.clone(),
        ExprKind::Call { callee, generics, args: a } => {
            let g = if generics.is_empty() {
                String::new()
            } else {
                let parts = generics
                    .iter()
                    .map(|g| match g {
                        GenericArg::Unit(u) => unit_expr(u),
                        GenericArg::Type(t) => ty(t),
                        GenericArg::Const(c) => expr(c),
                    })
                    .collect::<Vec<_>>();
                format!(" [{}]", parts.join(" "))
            };
            format!("(call {}{g}{})", callee.name, args(a))
        }
        ExprKind::Upper { name, args: a } | ExprKind::TypeName { name, args: a } => match a {
            Some(a) => format!("({}{})", name.name, args(a)),
            None => name.name.clone(),
        },
        ExprKind::Paren(inner) => expr(inner),
        ExprKind::Tuple(a, b) => format!("(tuple {} {})", expr(a), expr(b)),
        ExprKind::Array(items) => {
            let items = items.iter().map(expr).collect::<Vec<_>>().join(" ");
            format!("(array {items})")
        }
        ExprKind::InstanceArray { count, template, args: a } => {
            format!("(instances {} {}{})", expr(count), template.name, args(a))
        }
        ExprKind::Member { base, name, args: a } => match a {
            Some(a) => format!("(.{} {}{})", name.name, expr(base), args(a)),
            None => format!("(.{} {})", name.name, expr(base)),
        },
        ExprKind::Index { base, index } => format!("(index {} {})", expr(base), expr(index)),
        ExprKind::Slice { base, from, to } => format!("(slice {} {} {})", expr(base), expr(from), expr(to)),
        ExprKind::Index2 { base, row, col } => format!("(index {} {} {})", expr(base), expr(row), expr(col)),
        ExprKind::Cast { expr: inner, ty: t } => format!("(as {} {})", expr(inner), scalar_type(t)),
        ExprKind::Unary { op, expr: inner } => {
            let op = match op {
                UnaryOp::Neg => "-",
                UnaryOp::Not => "not",
                UnaryOp::BitNot => "~",
            };
            format!("({op} {})", expr(inner))
        }
        ExprKind::Binary { op, lhs, rhs } => format!("({} {} {})", binary_op(*op), expr(lhs), expr(rhs)),
        ExprKind::Match { subject, kind, pattern: p, binding } => {
            let b = binding.as_ref().map(|b| format!(" as {}", b.name)).unwrap_or_default();
            format!("({} {} {}{b})", match_kind(*kind), expr(subject), pattern(p))
        }
        ExprKind::Conditional { then, cond, otherwise } => {
            format!("(if {} {} {})", expr(cond), expr(then), expr(otherwise))
        }
        ExprKind::Temporal { op, window, inner } => {
            let op = match op {
                TemporalOp::Always => "always",
                TemporalOp::Never => "never",
                TemporalOp::Eventually => "eventually",
                TemporalOp::Stable => "stable",
                TemporalOp::Once => "once",
            };
            let w = window.as_ref().map(|d| format!("[{}]", duration(d))).unwrap_or_default();
            format!("({op}{w} {})", expr(inner))
        }
        ExprKind::Implies { lhs, rhs } => format!("(implies {} {})", expr(lhs), expr(rhs)),
    }
}

fn binary_op(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Or => "or",
        BinaryOp::And => "and",
        BinaryOp::Lt => "<",
        BinaryOp::Le => "<=",
        BinaryOp::Gt => ">",
        BinaryOp::Ge => ">=",
        BinaryOp::Eq => "==",
        BinaryOp::Ne => "!=",
        BinaryOp::BitOr => "|",
        BinaryOp::BitXor => "^",
        BinaryOp::BitAnd => "&",
        BinaryOp::Shl => "<<",
        BinaryOp::Shr => ">>",
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Rem => "%",
    }
}

/// Typ in Quelltextnaeher Schreibweise.
pub fn ty(t: &Type) -> String {
    match &t.kind {
        TypeKind::Scalar { scalar, range, wrap } => {
            let mut s = scalar_type(scalar);
            if let Some(r) = range {
                let _ = write!(s, " in {}", range_str(r));
            }
            s.push_str(&wrap_str(wrap));
            s
        }
        TypeKind::Array { len, elem } => format!("[{}]{}", expr(len), ty(elem)),
        TypeKind::Named { name, wrap } | TypeKind::TypeVar { name, wrap } => format!("{}{}", name.name, wrap_str(wrap)),
        TypeKind::Wrapped { inner, wrap } => format!("{}{}", ty(inner), wrap_str(&Some(wrap.clone()))),
        TypeKind::Bytes(n) => format!("bytes<{}>", expr(n)),
        TypeKind::Vec { elem, len } => format!("vec<{}, {}>", ty(elem), expr(len)),
        TypeKind::Line(n) => format!("line<{}>", expr(n)),
        TypeKind::Stream(e) => format!("stream<{}>", elem_type(e)),
        TypeKind::Samples { elem, len } => format!("samples<{}, {}>", ty(elem), expr(len)),
        TypeKind::Table { key, value } => format!("table<{}, {}>", ty(key), ty(value)),
        TypeKind::Mat { rows, cols, unit } => {
            let u = unit.as_ref().map(|u| format!("[{}]", unit_expr(u))).unwrap_or_default();
            format!("mat<{}, {}>{u}", expr(rows), expr(cols))
        }
        TypeKind::MatDim { rows, cols } => format!("mat[{}, {}]", unit_tuple(rows), unit_tuple(cols)),
        TypeKind::VecDim(t) => format!("vec[{}]", unit_tuple(t)),
        TypeKind::Map { key, value, len } => format!("map<{}, {}, {}>", ty(key), ty(value), expr(len)),
    }
}

fn wrap_str(w: &Option<Wrap>) -> String {
    match w {
        None => String::new(),
        Some(Wrap::Optional) => "?".into(),
        Some(Wrap::Result(e)) => format!("!{}", e.name),
    }
}

fn scalar_type(s: &ScalarType) -> String {
    match s {
        ScalarType::Bool => "bool".into(),
        ScalarType::Int { ty, unit } => format!("{}{}", int_type(*ty), bracket_unit(unit)),
        ScalarType::Float { width, unit } => {
            let w = width.map(float_width).unwrap_or("float");
            format!("{w}{}", bracket_unit(unit))
        }
        ScalarType::Duration => "Duration".into(),
        ScalarType::Str(n) => format!("str<{}>", expr(n)),
    }
}

fn bracket_unit(u: &Option<UnitExpr>) -> String {
    u.as_ref().map(|u| format!("[{}]", unit_expr(u))).unwrap_or_default()
}

fn int_type(t: IntType) -> &'static str {
    match t {
        IntType::Int => "int",
        IntType::I8 => "i8",
        IntType::I16 => "i16",
        IntType::I32 => "i32",
        IntType::I64 => "i64",
        IntType::U8 => "u8",
        IntType::U16 => "u16",
        IntType::U32 => "u32",
        IntType::U64 => "u64",
    }
}

fn float_width(w: FloatWidth) -> &'static str {
    match w {
        FloatWidth::F32 => "f32",
        FloatWidth::F64 => "f64",
    }
}

fn elem_type(e: &ElemType) -> String {
    match e {
        ElemType::U8 => "u8".into(),
        ElemType::Bytes(n) => format!("bytes<{}>", expr(n)),
        ElemType::Line(n) => format!("line<{}>", expr(n)),
        ElemType::Edge => "Edge".into(),
        ElemType::Named(n) => n.name.clone(),
        ElemType::Capture { elem, len } => format!("capture<{}, {}>", ty(elem), expr(len)),
    }
}

fn unit_tuple(t: &UnitTuple) -> String {
    match t {
        UnitTuple::Named(n) => n.name.clone(),
        UnitTuple::Inverse(n) => format!("1/{}", n.name),
        UnitTuple::Literal(units) => format!("({})", units.iter().map(unit_expr).collect::<Vec<_>>().join(", ")),
    }
}

/// Einheitenausdruck in kompakter Schreibweise (`K/min`, `m/s^2`, `1/s`).
pub fn unit_expr(u: &UnitExpr) -> String {
    let mut s = unit_term(&u.first);
    for (op, term) in &u.rest {
        s.push(match op {
            UnitOp::Mul => '*',
            UnitOp::Div => '/',
        });
        s.push_str(&unit_term(term));
    }
    s
}

fn unit_term(t: &UnitTerm) -> String {
    let mut s = t.name.as_ref().map(|n| n.name.clone()).unwrap_or_else(|| "1".into());
    if let Some(e) = &t.exponent {
        let _ = write!(s, "^{}", e.text);
    }
    s
}

fn range_str(r: &Range) -> String {
    format!("{}..{}", expr(&r.from), expr(&r.to))
}

fn number(n: &Number) -> String {
    match n {
        Number::Int(i) => i.text.clone(),
        Number::Float(f) => f.text.clone(),
    }
}

/// Dauer in der groessten Einheit, die sie ohne Rest darstellt.
pub fn duration(d: &DurationLit) -> String {
    const UNITS: [(&str, i64); 6] = [
        ("h", 3_600_000_000_000),
        ("min", 60_000_000_000),
        ("s", 1_000_000_000),
        ("ms", 1_000_000),
        ("us", 1_000),
        ("ns", 1),
    ];
    if d.ns == 0 {
        return "0s".into();
    }
    for (name, factor) in UNITS {
        if d.ns % factor == 0 {
            return format!("{}{name}", d.ns / factor);
        }
    }
    unreachable!("ns teilt jede Dauer")
}

fn string(s: &StrLit) -> String {
    let mut out = String::from("\"");
    for c in s.value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
