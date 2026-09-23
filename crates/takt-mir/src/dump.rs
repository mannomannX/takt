//! Lesbare Textform einer Maschine fuer Tests, Diagnosen und `takt graph`.
//!
//! Die Form ist an die Quellsprache angelehnt (ein Zustand je Block, Uebergaenge
//! als `when`/`after`), aber vollstaendig aufgeloest: Namen kommen aus den
//! Tabellen, Typen werden nicht wiederholt.

use std::fmt::Write;

use crate::expr::*;
use crate::ids::*;
use crate::machine::*;
use crate::pattern::{Format, FormatPiece, Pattern, PatternPiece};
use crate::program::Program;
use crate::stmt::*;

/// Textform einer Maschine.
pub fn dump_machine(p: &Program, id: MachineId) -> String {
    let mut d = Dumper { p, m: &p.machines[id.index()], out: String::new() };
    d.machine();
    d.out
}

struct Dumper<'a> {
    p: &'a Program,
    m: &'a Machine,
    out: String,
}

impl Dumper<'_> {
    fn line(&mut self, depth: usize, text: &str) {
        for _ in 0..depth {
            self.out.push_str("    ");
        }
        self.out.push_str(text);
        self.out.push('\n');
    }

    fn machine(&mut self) {
        let m = self.m;
        let head = if m.period == 1 {
            format!("machine {}:", m.name)
        } else {
            format!("machine {} every {}:", m.name, m.period)
        };
        self.line(0, &head);
        for (i, v) in m.vars.iter().enumerate() {
            if matches!(v.scope, VarScope::Machine) {
                let text = self.var_decl(v);
                // `persist` sieht man einer Maschinenvariablen sonst nicht an
                // (5.9), obwohl sie Neustarts ueberlebt.
                match m.persist.iter().find(|p| p.var.index() == i) {
                    Some(p) => {
                        self.line(1, &format!("persist {text}  # Schluessel {:#018x}", p.type_hash));
                    }
                    None => self.line(1, &text),
                }
            }
        }
        if !m.loop_block.stmts.is_empty() {
            self.block(1, "loop:", &m.loop_block);
        }
        // Eine Vorlage hat keine Zustaende: Ihr Rumpf entsteht erst in
        // der Instanz (5.8). `initial` zeigt dann ins Leere.
        let Some(state) = m.states.get(m.initial.index()) else {
            self.line(1, "(Vorlage; der Rumpf steht in den Instanzen)");
            return;
        };
        let initial = state.name.clone();
        self.line(1, &format!("initial {initial}"));
        for &s in &m.roots {
            self.state(1, s);
        }
    }

    fn var_decl(&self, v: &VarDef) -> String {
        let kind = if v.public { "pub var" } else { "var" };
        match &v.init {
            Some(e) => format!("{kind} {} = {}", v.name, self.expr(e)),
            None => format!("{kind} {}", v.name),
        }
    }

    fn state(&mut self, depth: usize, id: StateId) {
        let s = &self.m.states[id.index()];
        let mut head = format!("state {}", s.name);
        if s.idle {
            head.push_str(" idle");
        }
        if s.resume {
            head.push_str(" resume");
        }
        if let Some(n) = &s.step_name {
            write!(head, " step \"{n}\"").expect("String");
        }
        head.push(':');
        self.line(depth, &head);
        if let Some(t) = s.fault_target {
            let text = format!("fault -> {}", self.fault_target(t));
            self.line(depth + 1, &text);
        }
        for &v in &s.vars {
            let text = self.var_decl(&self.m.vars[v.index()]);
            self.line(depth + 1, &text);
        }
        if let Some(i) = s.initial {
            let text = format!("initial {}", self.m.states[i.index()].name);
            self.line(depth + 1, &text);
        }
        if !s.enter.stmts.is_empty() {
            self.block(depth + 1, "enter:", &s.enter);
        }
        if !s.loop_block.stmts.is_empty() {
            self.block(depth + 1, "loop:", &s.loop_block);
        }
        for h in &s.handlers {
            let mut head = format!("on {}", self.stream(h.stream));
            if let Some((k, pat)) = &h.pattern {
                let word = if *k == MatchKind::Matches { "matches" } else { "has" };
                write!(head, " {word} {}", self.pattern(pat)).expect("String");
            }
            if let Some(b) = h.binding {
                write!(head, " as {}", self.var_name(b)).expect("String");
            }
            if let Some(g) = &h.guard {
                write!(head, " when {}", self.expr(g)).expect("String");
            }
            head.push(':');
            self.block(depth + 1, &head, &h.body);
        }
        if let Some(seq) = &s.sequence {
            self.line(depth + 1, "sequence:");
            self.seq_items(depth + 2, &seq.items);
        }
        for t in &s.transitions {
            self.transition(depth + 1, t);
        }
        if !s.exit.stmts.is_empty() {
            self.block(depth + 1, "exit:", &s.exit);
        }
        for &c in &s.children {
            self.state(depth + 1, c);
        }
    }

    fn transition(&mut self, depth: usize, t: &Transition) {
        let trigger = match &t.trigger {
            TransTrigger::When(g) => format!("when {}", self.guard(g)),
            TransTrigger::After(d) => format!("after {}", self.expr(d)),
        };
        let mut text = format!("{trigger}:");
        for s in &t.actions.stmts {
            write!(text, " {};", self.stmt(s)).expect("String");
        }
        write!(text, " -> {}", self.target(t.target)).expect("String");
        self.line(depth, &text);
    }

    fn seq_items(&mut self, depth: usize, items: &[SeqItem]) {
        for item in items {
            match item {
                SeqItem::Stmt(s) => {
                    let text = self.stmt(s);
                    self.line(depth, &text);
                }
                SeqItem::Wait(d) => {
                    let text = format!("wait {}", self.expr(d));
                    self.line(depth, &text);
                }
                SeqItem::Until { guard, timeout, .. } => {
                    let mut text = format!("until {}", self.guard(guard));
                    if let Some(t) = timeout {
                        write!(text, " timeout {}", self.expr(&t.duration)).expect("String");
                        match &t.action {
                            TimeoutAction::Fault => {}
                            TimeoutAction::Goto(x) => write!(text, " -> {}", self.target(*x)).expect("String"),
                            TimeoutAction::Else(b) => {
                                text.push_str(" else:");
                                self.line(depth, &text);
                                self.stmts(depth + 1, b);
                                continue;
                            }
                        }
                    }
                    self.line(depth, &text);
                }
                SeqItem::Expect { cond, message, .. } => {
                    let mut text = format!("expect {}", self.expr(cond));
                    if let Some(m) = message {
                        write!(text, ", {}", self.format(m)).expect("String");
                    }
                    self.line(depth, &text);
                }
                SeqItem::Repeat { count, counter, body, .. } => {
                    let text = format!("repeat {} [{}]:", self.expr(count), self.var_name(*counter));
                    self.line(depth, &text);
                    self.seq_items(depth + 1, body);
                }
                SeqItem::Step { name, body, .. } => {
                    self.line(depth, &format!("step \"{name}\":"));
                    self.seq_items(depth + 1, body);
                }
            }
        }
    }

    fn block(&mut self, depth: usize, head: &str, b: &Block) {
        self.line(depth, head);
        self.stmts(depth + 1, b);
    }

    fn stmts(&mut self, depth: usize, b: &Block) {
        if b.stmts.is_empty() {
            self.line(depth, "pass");
        }
        for s in &b.stmts {
            match &s.kind {
                StmtKind::If { cond, then, otherwise } => {
                    let head = format!("if {}:", self.expr(cond));
                    self.block(depth, &head, then);
                    if !otherwise.stmts.is_empty() {
                        self.block(depth, "else:", otherwise);
                    }
                }
                StmtKind::ForRange { var, count, body } => {
                    let head = format!("for {} in range({}):", self.var_name(*var), self.expr(count));
                    self.block(depth, &head, body);
                }
                StmtKind::ForEach { vars, iter, body } => {
                    let names = match vars {
                        ForVars::One(v) => self.var_name(*v).to_string(),
                        ForVars::Pair(k, v) => format!("({}, {})", self.var_name(*k), self.var_name(*v)),
                    };
                    let head = format!("for {names} in {}:", self.expr(iter));
                    self.block(depth, &head, body);
                }
                StmtKind::Match { subject, arms } => {
                    let head = format!("match {}:", self.expr(subject));
                    self.line(depth, &head);
                    for arm in arms {
                        let pat = match &arm.pattern {
                            ArmPattern::Wild => "_".to_string(),
                            ArmPattern::Variant { variant, fields } => {
                                let names: Vec<&str> = fields.iter().map(|&v| self.var_name(v)).collect();
                                format!("V{variant}({})", names.join(", "))
                            }
                            ArmPattern::Values(vals) => vals
                                .iter()
                                .map(|v| match &v.hi {
                                    Some(hi) => format!("{}..{}", self.expr(&v.lo), self.expr(hi)),
                                    None => self.expr(&v.lo),
                                })
                                .collect::<Vec<_>>()
                                .join(", "),
                        };
                        self.block(depth + 1, &format!("case {pat}:"), &arm.body);
                    }
                }
                StmtKind::At { time, body } => {
                    let head = format!("at {}:", self.expr(time));
                    self.block(depth, &head, body);
                }
                StmtKind::Every { period, body, .. } => {
                    let head = format!("every {}:", self.expr(period));
                    self.block(depth, &head, body);
                }
                _ => {
                    let text = self.stmt(s);
                    self.line(depth, &text);
                }
            }
        }
    }

    /// Einzeilige Anweisung; Bloecke nennen nur ihren Kopf.
    fn stmt(&self, s: &Stmt) -> String {
        match &s.kind {
            StmtKind::Assign { target, value } => format!("{} = {}", self.place(target), self.expr(value)),
            StmtKind::Check { cond, message, confirm, within, target, req, kind } => {
                let mut t =
                    format!("{} {}", if *kind == CheckKind::Check { "check" } else { "expect" }, self.expr(cond));
                if let Some(m) = message {
                    write!(t, ", {}", self.format(m)).expect("String");
                }
                if let Some(c) = confirm {
                    write!(t, " for {}", self.expr(&c.duration)).expect("String");
                }
                if let Some(w) = within {
                    write!(t, " within {}", self.expr(w)).expect("String");
                }
                if let Some(x) = target {
                    write!(t, " -> {}", self.target(*x)).expect("String");
                }
                if let Some(r) = req {
                    write!(t, " req \"{r}\"").expect("String");
                }
                t
            }
            StmtKind::Goto(t) => format!("-> {}", self.target(*t)),
            StmtKind::Abort { message } => match message {
                Some(m) => format!("abort {}", self.format(m)),
                None => "abort".to_string(),
            },
            StmtKind::If { cond, .. } => format!("if {}: …", self.expr(cond)),
            StmtKind::ForRange { var, count, .. } => {
                format!("for {} in range({}): …", self.var_name(*var), self.expr(count))
            }
            StmtKind::ForEach { iter, .. } => format!("for … in {}: …", self.expr(iter)),
            StmtKind::Match { subject, .. } => format!("match {}: …", self.expr(subject)),
            StmtKind::Return(e) => format!("return {}", self.expr(e)),
            StmtKind::Send { stream, value, .. } => format!("send {}, {}", self.stream(*stream), self.expr(value)),
            StmtKind::At { time, .. } => format!("at {}: …", self.expr(time)),
            StmtKind::Cancel(c) => format!("cancel {}", self.p.channels[c.index()].name),
            StmtKind::Skip(s) => format!("{}.skip()", self.stream(*s)),
            StmtKind::Raise(s) => format!("raise {}", self.m.signals[s.index()].name),
            StmtKind::Job { handle, native, args } => {
                format!("job {} = {}({})", self.var_name(*handle), self.p.natives[native.index()].name, self.args(args))
            }
            StmtKind::Every { period, .. } => format!("every {}: …", self.expr(period)),
            StmtKind::Break => "break".to_string(),
            StmtKind::Observe(o) => match o {
                Observe::Alert { cond, message, confirm, .. } => {
                    let mut t = format!("alert {}, {}", self.expr(cond), self.format(message));
                    if let Some(c) = confirm {
                        write!(t, " for {}", self.expr(&c.duration)).expect("String");
                    }
                    t
                }
                Observe::Log(f) => format!("log {}", self.format(f)),
                Observe::Measure { name, value } => format!("measure {name} = {}", self.expr(value)),
                Observe::Verify { cond, message, req } => {
                    let mut t = format!("verify {}, {}", self.expr(cond), self.format(message));
                    if let Some(r) = req {
                        write!(t, " req \"{r}\"").expect("String");
                    }
                    t
                }
                Observe::Verdict { pass, message } => {
                    let mut t = format!("verdict {}", if *pass { "pass" } else { "fail" });
                    if let Some(m) = message {
                        write!(t, " {}", self.format(m)).expect("String");
                    }
                    t
                }
            },
            StmtKind::Arm { trigger, on } => {
                format!("{} {}", if *on { "arm" } else { "disarm" }, self.p.triggers[trigger.index()].name)
            }
            StmtKind::MethodCall { target, receiver, method, args } => {
                let call = format!("{}.{}({})", self.place(receiver), self.method(*method), self.args(args));
                match target {
                    Some(t) => format!("{} = {call}", self.place(t)),
                    None => call,
                }
            }
            StmtKind::Pass => "pass".to_string(),
        }
    }

    fn method(&self, m: Method) -> String {
        match m {
            Method::Block(f) => self.p.fns[f.index()].name.clone(),
            other => other.name().expect("Membername").to_string(),
        }
    }

    fn place(&self, p: &Place) -> String {
        match p {
            Place::Var(v) => self.var_name(*v).to_string(),
            Place::Output(c) => self.p.channels[c.index()].name.clone(),
            Place::Port(p) => self.p.ports[p.index()].name.clone(),
            Place::Field(b, f) => format!("{}.{f}", self.place(b)),
            Place::Index(b, i) => format!("{}[{}]", self.place(b), self.expr(i)),
            Place::Index2(b, i, j) => format!("{}[{}, {}]", self.place(b), self.expr(i), self.expr(j)),
        }
    }

    fn target(&self, t: Target) -> String {
        match t {
            Target::State(s) => self.m.states[s.index()].name.clone(),
            Target::Faulted => "FAULTED".to_string(),
            Target::Fault(k) => format!("[Fault {k:?}]"),
        }
    }

    fn fault_target(&self, t: FaultTarget) -> String {
        match t {
            FaultTarget::State(s) => self.m.states[s.index()].name.clone(),
            FaultTarget::Faulted => "FAULTED".to_string(),
        }
    }

    fn guard(&self, g: &Guard) -> String {
        match g {
            Guard::Expr(e) => self.expr(e),
            Guard::Match { subject, pattern, binding, kind } => {
                let op = match kind {
                    MatchKind::Matches => "matches",
                    MatchKind::Has => "has",
                };
                let mut t = format!("{} {op} {}", self.expr(subject), self.pattern(pattern));
                if let Some(b) = binding {
                    write!(t, " as {}", self.var_name(*b)).expect("String");
                }
                t
            }
            Guard::Next { stream, binding } => format!("{} as {}", self.stream(*stream), self.var_name(*binding)),
        }
    }

    fn stream(&self, s: StreamRef) -> String {
        match s {
            StreamRef::Channel(c) => self.p.channels[c.index()].name.clone(),
            StreamRef::Internal(s) => self.p.streams[s.index()].name.clone(),
            StreamRef::Fired(t) => format!("{}.fired", self.p.triggers[t.index()].name),
            StreamRef::Var(v) => self.var_name(v).to_string(),
        }
    }

    fn pattern(&self, p: &Pattern) -> String {
        match p {
            Pattern::Text { pieces, .. } => {
                let mut t = String::from("\"");
                for piece in pieces {
                    match piece {
                        PatternPiece::Text(s) => t.push_str(s),
                        PatternPiece::Capture { name, kind } => write!(t, "{{{name}:{kind:?}}}").expect("String"),
                        PatternPiece::Any => t.push_str("{_}"),
                    }
                }
                t.push('"');
                t
            }
            Pattern::Record { record, fields } => {
                let r = &self.p.records[record.index()];
                let parts: Vec<String> =
                    fields.iter().map(|(f, e)| format!("{} = {}", r.fields[*f as usize].name, self.expr(e))).collect();
                format!("{}({})", r.name, parts.join(", "))
            }
        }
    }

    fn format(&self, f: &Format) -> String {
        let mut t = String::from("\"");
        for piece in &f.pieces {
            match piece {
                FormatPiece::Text(s) => t.push_str(s),
                FormatPiece::Expr { expr, spec } => match spec {
                    Some(s) => write!(t, "{{{}:{s}}}", self.expr(expr)).expect("String"),
                    None => write!(t, "{{{}}}", self.expr(expr)).expect("String"),
                },
            }
        }
        t.push('"');
        t
    }

    fn var_name(&self, v: VarId) -> &str {
        &self.m.vars[v.index()].name
    }

    fn args(&self, args: &[Expr]) -> String {
        args.iter().map(|a| self.expr(a)).collect::<Vec<_>>().join(", ")
    }

    fn machine_ref(&self, r: &MachineRef) -> String {
        let name = &self.p.machines[r.machine.index()].name;
        match &r.index {
            Some(i) => format!("{name}[{}]", self.expr(i)),
            None => name.clone(),
        }
    }

    /// Ausdruck in Quellschreibweise mit vollstaendiger Klammerung der Operatoren.
    pub(crate) fn expr(&self, e: &Expr) -> String {
        match &e.kind {
            ExprKind::Bool(b) => b.to_string(),
            ExprKind::Int(i) => i.to_string(),
            ExprKind::Float(f) => format!("{f:?}"),
            ExprKind::Duration(ns) => duration(*ns),
            ExprKind::Str(s) => format!("{s:?}"),
            ExprKind::None => "none".to_string(),
            ExprKind::Default => "default".to_string(),
            ExprKind::Variant { enum_id, variant, fields } => {
                let v = &self.p.enums[enum_id.index()].variants[*variant as usize];
                if fields.is_empty() { v.name.clone() } else { format!("{}({})", v.name, self.args(fields)) }
            }
            ExprKind::Record { record, fields } => {
                format!("{}({})", self.p.records[record.index()].name, self.args(fields))
            }
            ExprKind::Array(items) => format!("[{}]", self.args(items)),
            ExprKind::Tuple(a, b) => format!("({}, {})", self.expr(a), self.expr(b)),
            ExprKind::BlockInit { block, args, count } => {
                let call = format!("{}({})", self.p.blocks[block.index()].name, self.args(args));
                match count {
                    Some(n) => format!("[{n}] {call}"),
                    None => call,
                }
            }
            ExprKind::Var(v) => self.var_name(*v).to_string(),
            ExprKind::Param(p) => self.p.params[p.index()].name.clone(),
            ExprKind::Command(c) => self.p.commands[c.index()].name.clone(),
            ExprKind::Input { channel, .. } | ExprKind::Output(channel) => {
                self.p.channels[channel.index()].name.clone()
            }
            ExprKind::Published { machine, var } => {
                format!(
                    "{}.{}",
                    self.machine_ref(machine),
                    self.p.machines[machine.machine.index()].vars[var.index()].name
                )
            }
            ExprKind::StateOf(m) => format!("{}.state", self.machine_ref(m)),
            ExprKind::JobState { handle, field } => format!(
                "{}.{}",
                self.var_name(*handle),
                match field {
                    crate::expr::JobField::Done => "done",
                    crate::expr::JobField::Result => "result",
                }
            ),
            ExprKind::Signal { machine, signal } => {
                format!(
                    "{}.{}",
                    self.machine_ref(machine),
                    self.p.machines[machine.machine.index()].signals[signal.index()].name
                )
            }
            ExprKind::Builtin(b) => match b {
                Builtin::Now => "now",
                Builtin::Tick => "tick",
                Builtin::TimeInState => "time_in_state",
                Builtin::LastFault => "last_fault",
                Builtin::Event => "event",
            }
            .to_string(),
            ExprKind::Armed(t) => format!("{}.armed", self.p.triggers[t.index()].name),
            ExprKind::PortRead(p) => self.p.ports[p.index()].name.clone(),
            ExprKind::Field { base, field } => format!("{}.{field}", self.expr(base)),
            ExprKind::Index { base, index } => format!("{}[{}]", self.expr(base), self.expr(index)),
            ExprKind::Index2 { base, row, col } => {
                format!("{}[{}, {}]", self.expr(base), self.expr(row), self.expr(col))
            }
            ExprKind::Slice { base, from, to } => {
                format!("{}[{}..{}]", self.expr(base), self.expr(from), self.expr(to))
            }
            ExprKind::Accessor { base, accessor, args } => {
                let name = accessor.name();
                if args.is_empty() {
                    format!("{}.{name}", self.expr(base))
                } else {
                    format!("{}.{name}({})", self.expr(base), self.args(args))
                }
            }
            ExprKind::Unary { op, expr } => {
                let sym = match op {
                    UnaryOp::Neg => "-",
                    UnaryOp::Not => "not ",
                    UnaryOp::BitNot => "~",
                };
                format!("({sym}{})", self.expr(expr))
            }
            ExprKind::Binary { op, lhs, rhs } => format!("({} {} {})", self.expr(lhs), binary(*op), self.expr(rhs)),
            ExprKind::Cond { cond, then, otherwise } => {
                format!("({} if {} else {})", self.expr(then), self.expr(cond), self.expr(otherwise))
            }
            ExprKind::Cast { expr, to } => format!("({} as T{})", self.expr(expr), to.0),
            ExprKind::Convert { expr, kind, unit } => {
                let word = match kind {
                    ConvertKind::To => "to",
                    ConvertKind::ToFloat => "to_float",
                    ConvertKind::As => "as",
                };
                format!("{}.{word}({})", self.expr(expr), self.p.units[unit.index()].name)
            }
            ExprKind::Matches { subject, kind, pattern, binding } => {
                let word = if *kind == MatchKind::Matches { "matches" } else { "has" };
                let mut t = format!("({} {word} {}", self.expr(subject), self.pattern(pattern));
                if let Some(b) = binding {
                    write!(t, " as {}", self.var_name(*b)).expect("String");
                }
                t.push(')');
                t
            }
            ExprKind::Call { callee, args } => format!("{}({})", self.p.fns[callee.index()].name, self.args(args)),
            ExprKind::NativeCall { native, args } => {
                format!("{}({})", self.p.natives[native.index()].name, self.args(args))
            }
            ExprKind::MatOp { op, args } => format!("{}({})", format!("{op:?}").to_lowercase(), self.args(args)),
            ExprKind::Decode { record, bytes } => {
                format!("{}.decode({})", self.p.records[record.index()].name, self.expr(bytes))
            }
            ExprKind::Checked { expr, kind } => format!("checked[{kind:?}]({})", self.expr(expr)),
            ExprKind::Lift(e) => format!("lift({})", self.expr(e)),
            ExprKind::Ok(e) => format!("OK({})", self.expr(e)),
            ExprKind::Err(e) => format!("ERR({})", self.expr(e)),
            ExprKind::Stream(s) => self.p.streams[s.index()].name.clone(),
            ExprKind::Format(f) => format!("format({})", self.format(f)),
            ExprKind::Intrinsic { op, args } => format!("{}({})", op.name(), self.args(args)),
        }
    }
}

fn binary(op: BinaryOp) -> &'static str {
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

/// Dauer in der groessten Einheit, in der sie ganzzahlig ist (3.3).
pub fn duration(ns: i64) -> String {
    const UNITS: [(&str, i64); 7] = [
        ("d", 86_400_000_000_000),
        ("h", 3_600_000_000_000),
        ("min", 60_000_000_000),
        ("s", 1_000_000_000),
        ("ms", 1_000_000),
        ("us", 1_000),
        ("ns", 1),
    ];
    for (name, factor) in UNITS {
        if ns != 0 && ns % factor == 0 {
            return format!("{} {name}", ns / factor);
        }
    }
    format!("{ns} ns")
}
