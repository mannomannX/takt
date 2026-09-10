//! Desugaring nach Referenz 6.2: Beispiel 6.3 und je eine Regel der Tabelle.

use takt_diag::Span;
use takt_mir::dump::dump_machine;
use takt_mir::expr::*;
use takt_mir::machine::*;
use takt_mir::pattern::Format;
use takt_mir::program::*;
use takt_mir::stmt::*;
use takt_mir::types::*;
use takt_mir::*;

/// Testrahmen: ein Programm mit `chamber_p`, `fuel_main`, `igniter`, `lox_main`,
/// `IGNITION_P`, `BURN_DURATION` und einer Maschine mit `IGNITION` und `SHUTDOWN`.
struct Rig {
    p: Program,
    m: MachineId,
    ignition: StateId,
    shutdown: StateId,
    t_bool: TypeId,
    t_int: TypeId,
    t_dur: TypeId,
    t_bar: TypeId,
    chamber_p: ChannelId,
    fuel_main: ChannelId,
    igniter: ChannelId,
    lox_main: ChannelId,
    ignition_p: ParamId,
    burn: ParamId,
    done: VarId,
}

impl Rig {
    fn new() -> Self {
        let mut p = Program::new(Config::new(1, 1_000_000));
        let t_bool = p.types.intern(Type::Bool);
        let t_int = p.types.intern(Type::Int { width: IntWidth::I64, unit: None, range: None });
        let t_dur = p.types.intern(Type::Duration { range: None });
        let t_bar = p.types.intern(Type::Float { width: FloatWidth::F64, unit: None, range: None });
        let channel = |p: &mut Program, dir, name: &str, ty| {
            p.add_channel(Channel {
                dir,
                name: name.into(),
                ty,
                binding: Binding::None,
                attrs: ChannelAttrs::default(),
                meta: Meta::default(),
                owner: None,
                span: Span::default(),
            })
        };
        let chamber_p = channel(&mut p, Direction::Input, "chamber_p", t_bar);
        let fuel_main = channel(&mut p, Direction::Output, "fuel_main", t_bool);
        let igniter = channel(&mut p, Direction::Output, "igniter", t_bool);
        let lox_main = channel(&mut p, Direction::Output, "lox_main", t_bool);
        for (name, ty) in [("IGNITION_P", t_bar), ("BURN_DURATION", t_dur)] {
            p.params.push(Param {
                name: name.into(),
                ty,
                default: Expr::new(ExprKind::Default, ty, Span::default()),
                tunable: false,
                meta: Meta::default(),
                span: Span::default(),
            });
        }
        let mut m = Machine::new("hotfire");
        let ignition = m.add_state(State::new("IGNITION", None));
        let shutdown = m.add_state(State::new("SHUTDOWN", None));
        m.initial = ignition;
        let done = m.add_var(VarDef {
            name: "done".into(),
            ty: t_bool,
            init: Some(Expr::new(ExprKind::Bool(false), t_bool, Span::default())),
            scope: VarScope::Lifted(ignition),
            public: false,
            span: Span::default(),
        });
        let m = p.add_machine(m);
        Rig {
            p,
            m,
            ignition,
            shutdown,
            t_bool,
            t_int,
            t_dur,
            t_bar,
            chamber_p,
            fuel_main,
            igniter,
            lox_main,
            ignition_p: ParamId(0),
            burn: ParamId(1),
            done,
        }
    }

    fn machine(&mut self) -> &mut Machine {
        &mut self.p.machines[self.m.index()]
    }

    fn e(&self, kind: ExprKind, ty: TypeId) -> Expr {
        Expr::new(kind, ty, Span::default())
    }

    fn set(&self, out: ChannelId, v: bool) -> Stmt {
        Stmt::new(
            StmtKind::Assign { target: Place::Output(out), value: self.e(ExprKind::Bool(v), self.t_bool) },
            Span::default(),
        )
    }

    fn pressure_ok(&self) -> Expr {
        self.e(
            ExprKind::Binary {
                op: BinaryOp::Gt,
                lhs: Box::new(self.e(ExprKind::Input { channel: self.chamber_p, dominated: true }, self.t_bar)),
                rhs: Box::new(self.e(ExprKind::Param(self.ignition_p), self.t_bar)),
            },
            self.t_bool,
        )
    }

    fn dur(&self, ns: i64) -> Expr {
        self.e(ExprKind::Duration(ns), self.t_dur)
    }

    fn check(&self, cond: Expr, msg: &str) -> Stmt {
        Stmt::new(
            StmtKind::Check {
                cond,
                message: Some(Format::text(msg)),
                confirm: None,
                target: None,
                req: None,
                kind: CheckKind::Check,
            },
            Span::default(),
        )
    }

    fn goto(&self, s: StateId) -> Stmt {
        Stmt::new(StmtKind::Goto(Target::State(s)), Span::default())
    }

    fn var(&mut self, name: &str, ty: TypeId, init: Option<Expr>) -> VarId {
        let ignition = self.ignition;
        self.machine().add_var(VarDef {
            name: name.into(),
            ty,
            init,
            scope: VarScope::Lifted(ignition),
            public: false,
            span: Span::default(),
        })
    }

    /// Setzt die Sequenz auf `IGNITION`, desugart und liefert den Dump.
    fn run(mut self, items: Vec<SeqItem>) -> String {
        let done = self.done;
        let ignition = self.ignition;
        self.machine().states[ignition.index()].sequence = Some(Sequence { items, done, span: Span::default() });
        desugar(&mut self.p).expect("Desugaring");
        assert!(self.p.is_core());
        dump_machine(&self.p, self.m)
    }
}

#[test]
fn example_6_3() {
    let r = Rig::new();
    let items = vec![
        SeqItem::Stmt(r.set(r.fuel_main, true)),
        SeqItem::Wait(r.dur(150_000_000)),
        SeqItem::Stmt(r.set(r.igniter, true)),
        SeqItem::Until {
            guard: Guard::Expr(r.pressure_ok()),
            timeout: Some(Timeout { duration: r.dur(500_000_000), action: TimeoutAction::Fault }),
            span: Span::default(),
        },
        SeqItem::Stmt(r.set(r.lox_main, true)),
        SeqItem::Stmt(r.check(r.pressure_ok(), "flameout")),
        SeqItem::Wait(r.e(ExprKind::Param(r.burn), r.t_dur)),
        SeqItem::Stmt(r.goto(r.shutdown)),
    ];
    let expected = "\
machine hotfire:
    initial IGNITION
    state IGNITION:
        var done = false
        initial IGNITION.S0
        state IGNITION.S0:
            enter:
                fuel_main = true
            after 150 ms: -> IGNITION.S1
        state IGNITION.S1:
            enter:
                igniter = true
            when (chamber_p > IGNITION_P): -> IGNITION.S2
            after 500 ms: -> [Fault Timeout]
        state IGNITION.S2:
            enter:
                lox_main = true
            loop:
                check (chamber_p > IGNITION_P), \"flameout\"
            after BURN_DURATION: -> SHUTDOWN
    state SHUTDOWN:
";
    assert_eq!(r.run(items), expected);
}

#[test]
fn end_without_goto_holds_and_sets_done() {
    let r = Rig::new();
    let items = vec![SeqItem::Stmt(r.set(r.fuel_main, true)), SeqItem::Wait(r.dur(1_000_000))];
    let out = r.run(items);
    assert!(out.contains("        state IGNITION.S1:\n            enter:\n                done = true\n"), "{out}");
    assert!(!out.contains("S2"), "{out}");
}

#[test]
fn check_is_continuous_from_its_position() {
    let r = Rig::new();
    let items = vec![
        SeqItem::Wait(r.dur(1_000_000)),
        SeqItem::Stmt(r.check(r.pressure_ok(), "p")),
        SeqItem::Wait(r.dur(1_000_000)),
        SeqItem::Wait(r.dur(1_000_000)),
    ];
    let out = r.run(items);
    let s0 = out.find("state IGNITION.S0:").expect("S0");
    let s1 = out.find("state IGNITION.S1:").expect("S1");
    assert!(!out[s0..s1].contains("check"), "S0 liegt vor dem check: {out}");
    assert_eq!(out.matches("check (chamber_p > IGNITION_P), \"p\"").count(), 3, "S1, S2, S3: {out}");
}

#[test]
fn expect_checks_once_via_flag() {
    let r = Rig::new();
    let items = vec![SeqItem::Expect { cond: r.pressure_ok(), message: None, span: Span::default() }];
    let out = r.run(items);
    let expected = "\
        state IGNITION.S0:
            var expect_1 = true
            enter:
                done = true
            loop:
                if expect_1:
                    expect (chamber_p > IGNITION_P)
                    expect_1 = false
";
    assert!(out.contains(expected), "{out}");
}

#[test]
fn until_timeout_goto_and_else() {
    let mut r = Rig::new();
    let shutdown = r.shutdown;
    let m = r.var("m", r.t_int, None);
    let items = vec![
        SeqItem::Until {
            guard: Guard::Expr(r.pressure_ok()),
            timeout: Some(Timeout {
                duration: r.dur(2_000_000_000),
                action: TimeoutAction::Goto(Target::State(shutdown)),
            }),
            span: Span::default(),
        },
        SeqItem::Until {
            guard: Guard::Next { stream: StreamRef::Channel(r.chamber_p), binding: m },
            timeout: Some(Timeout {
                duration: r.dur(3_000_000_000),
                action: TimeoutAction::Else(Block::new(vec![
                    Stmt::new(StmtKind::Observe(Observe::Verdict { pass: false, message: None }), Span::default()),
                    r.goto(shutdown),
                ])),
            }),
            span: Span::default(),
        },
        SeqItem::Until {
            guard: Guard::Expr(r.pressure_ok()),
            timeout: Some(Timeout {
                duration: r.dur(4_000_000_000),
                action: TimeoutAction::Else(Block::new(vec![r.set(r.igniter, false)])),
            }),
            span: Span::default(),
        },
    ];
    let out = r.run(items);
    assert!(
        out.contains("            when (chamber_p > IGNITION_P): -> IGNITION.S1\n            after 2 s: -> SHUTDOWN\n"),
        "{out}"
    );
    assert!(
        out.contains(
            "            when chamber_p as m: -> IGNITION.S2\n            after 3 s: verdict fail; -> SHUTDOWN\n"
        ),
        "{out}"
    );
    assert!(out.contains("            when (chamber_p > IGNITION_P): -> IGNITION.S3\n            after 4 s: igniter = false; -> IGNITION.S3\n"), "{out}");
}

#[test]
fn repeat_counts_with_weak_back_jump() {
    let mut r = Rig::new();
    let k = r.var("k_r", r.t_int, Some(r.e(ExprKind::Int(0), r.t_int)));
    let items = vec![
        SeqItem::Stmt(r.set(r.fuel_main, true)),
        SeqItem::Repeat {
            count: r.e(ExprKind::Int(3), r.t_int),
            counter: k,
            body: vec![
                SeqItem::Step {
                    name: "puls".into(),
                    body: vec![SeqItem::Stmt(r.set(r.igniter, true))],
                    span: Span::default(),
                },
                SeqItem::Wait(r.dur(10_000_000)),
            ],
            span: Span::default(),
        },
        SeqItem::Stmt(r.set(r.lox_main, true)),
    ];
    let expected = "\
        state IGNITION.S0:
            enter:
                fuel_main = true
            when true: -> IGNITION.S1
        state IGNITION.S1 step \"puls\":
            enter:
                igniter = true
            after 10 ms: -> IGNITION.S2
        state IGNITION.S2:
            when ((k_r + 1) < 3): k_r = (k_r + 1); -> IGNITION.S1
            when true: k_r = 0; -> IGNITION.S3
        state IGNITION.S3:
            enter:
                lox_main = true
                done = true
";
    let out = r.run(items);
    assert!(out.contains(expected), "{out}");
}

#[test]
fn if_branches_with_goto_become_transitions() {
    let r = Rig::new();
    let shutdown = r.shutdown;
    let cond_a = r.e(ExprKind::Input { channel: r.chamber_p, dominated: true }, r.t_bool);
    let cond_b = r.e(ExprKind::Output(r.igniter), r.t_bool);
    let branch = Stmt::new(
        StmtKind::If {
            cond: cond_a,
            then: Block::new(vec![r.set(r.igniter, false), r.goto(shutdown)]),
            otherwise: Block::new(vec![Stmt::new(
                StmtKind::If {
                    cond: cond_b,
                    then: Block::new(vec![r.goto(shutdown)]),
                    otherwise: Block::new(vec![r.set(r.lox_main, false)]),
                },
                Span::default(),
            )]),
        },
        Span::default(),
    );
    let items = vec![SeqItem::Stmt(r.set(r.fuel_main, true)), SeqItem::Stmt(branch), SeqItem::Wait(r.dur(1_000_000))];
    let expected = "\
        state IGNITION.S0:
            enter:
                fuel_main = true
            when chamber_p: igniter = false; -> SHUTDOWN
            when ((not chamber_p) and igniter): -> SHUTDOWN
            when ((not chamber_p) and (not igniter)): lox_main = false; -> IGNITION.S1
        state IGNITION.S1:
            after 1 ms: -> IGNITION.S2
        state IGNITION.S2:
            enter:
                done = true
";
    let out = r.run(items);
    assert!(out.contains(expected), "{out}");
}

#[test]
fn goto_inside_a_loop_is_rejected() {
    let mut r = Rig::new();
    let shutdown = r.shutdown;
    let i = r.var("i", r.t_int, None);
    let bad = Stmt::new(
        StmtKind::ForRange {
            var: i,
            count: r.e(ExprKind::Int(4), r.t_int),
            body: Block::new(vec![Stmt::new(StmtKind::Goto(Target::State(shutdown)), Span::new(10, 12))]),
        },
        Span::new(1, 30),
    );
    let done = r.done;
    let ignition = r.ignition;
    r.machine().states[ignition.index()].sequence =
        Some(Sequence { items: vec![SeqItem::Stmt(bad)], done, span: Span::default() });
    let err = desugar(&mut r.p).expect_err("Fehler");
    assert_eq!(err.code, "MIR");
    assert_eq!(err.span, Span::new(10, 12), "die Meldung zeigt auf das `->`");
}
