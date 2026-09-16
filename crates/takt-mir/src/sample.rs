//! Ein konstruiertes Programm mit jeder Knotenart, fuer den Roundtrip-Test
//! des Formats (plan/mir.md, Abschnitt 4) und als Fixture spaeterer Stufen.
//! Es ist typkorrekt im Sinne der Tabellen, aber kein sinnvolles Steuerprogramm.

use takt_diag::Span;

use crate::expr::*;
use crate::fns::*;
use crate::ids::*;
use crate::machine::*;
use crate::pattern::*;
use crate::program::*;
use crate::stmt::*;
use crate::types::*;

fn sp(n: u32) -> Span {
    Span::new(n, n + 3)
}

fn e(kind: ExprKind, ty: TypeId) -> Expr {
    Expr::new(kind, ty, sp(7))
}

fn bx(x: Expr) -> Box<Expr> {
    Box::new(x)
}

/// Ein Ausdruck mit den Annotationen aus M3 (3.4): bewiesenes Intervall und
/// gewaehlte Darstellung. Der Roundtrip muss beide tragen.
fn annotated(kind: ExprKind, ty: TypeId) -> Expr {
    let mut x = e(kind, ty);
    x.range = Some(Range { lo: Const::Int(0), hi: Const::Int(9), origin: RangeOrigin::Proven });
    x.repr = Some(crate::expr::Repr::I32);
    x
}

fn stmt(kind: StmtKind) -> Stmt {
    Stmt::new(kind, sp(9))
}

/// Programm mit jeder Knotenart.
pub fn full_program() -> Program {
    let mut config = Config::new(1, 1_000_000);
    config.output_timing = OutputTiming::Boundary;
    config.fault_is_fail = false;
    config.float_width = FloatWidth::F32;
    config.tick_source = Some(Address::simple("tim1/update"));
    config.tick_tolerance = Some(TickTolerance { pct: 2.0, ticks: 10 });
    config.target = Some("cortex-m4f".to_string());
    let mut p = Program::new(config);

    // Einheiten
    let bar = UnitId(0);
    let psi = UnitId(1);
    let s = UnitId(3);
    p.units.push(UnitDef {
        name: "bar".into(),
        dimension: [-1, 1, -2, 0, 0, 0, 0],
        factor: Rational::int(100_000),
        affine_offset: None,
        predefined: true,
        span: Span::default(),
    });
    p.units.push(UnitDef {
        name: "psi".into(),
        dimension: [-1, 1, -2, 0, 0, 0, 0],
        factor: Rational { num: 6_894_757_293_168, den: 1_000_000_000 },
        affine_offset: None,
        predefined: false,
        span: sp(1),
    });
    p.units.push(UnitDef {
        name: "degC".into(),
        dimension: [0, 0, 0, 0, 1, 0, 0],
        factor: Rational::int(1),
        affine_offset: Some(Rational { num: 27315, den: 100 }),
        predefined: true,
        span: Span::default(),
    });
    p.units.push(UnitDef {
        name: "s".into(),
        dimension: [0, 0, 1, 0, 0, 0, 0],
        factor: Rational::int(1),
        affine_offset: None,
        predefined: true,
        span: Span::default(),
    });

    // Typen
    let range = Range { lo: Const::Float(0.0), hi: Const::Float(100.0), origin: RangeOrigin::Declared };
    let t_bool = p.types.intern(Type::Bool);
    let t_int = p.types.intern(Type::Int { width: IntWidth::I64, unit: None, range: None });
    let t_i8 = p.types.intern(Type::Int {
        width: IntWidth::I8,
        unit: Some(bar),
        range: Some(Range { lo: Const::Int(-3), hi: Const::Int(3), origin: RangeOrigin::Proven }),
    });
    let t_u16 = p.types.intern(Type::Int { width: IntWidth::U16, unit: None, range: None });
    let t_u8 = p.types.intern(Type::Int { width: IntWidth::U8, unit: None, range: None });
    let t_float = p.types.intern(Type::Float { width: FloatWidth::F32, unit: None, range: None });
    let t_bar = p.types.intern(Type::Float { width: FloatWidth::F32, unit: Some(bar), range: Some(range) });
    let t_psi = p.types.intern(Type::Float { width: FloatWidth::F32, unit: Some(psi), range: None });
    let t_dur = p.types.intern(Type::Duration {
        range: Some(Range {
            lo: Const::Duration(0),
            hi: Const::Duration(1_000_000_000),
            origin: RangeOrigin::Declared,
        }),
    });
    let t_dur_plain = p.types.intern(Type::Duration { range: None });
    let e_valve = EnumId(0);
    let e_err = EnumId(1);
    let e_msg = EnumId(2);
    let t_valve = p.types.intern(Type::Enum(e_valve));
    let t_msg = p.types.intern(Type::Enum(e_msg));
    let r_frame = RecordId(0);
    let r_edge = RecordId(1);
    let t_frame = p.types.intern(Type::Record(r_frame));
    let t_edge = p.types.intern(Type::Record(r_edge));
    let t_arr = p.types.intern(Type::Array { elem: t_float, len: 16 });
    let t_bytes = p.types.intern(Type::Bytes { cap: 8 });
    let t_vec = p.types.intern(Type::Vec { elem: t_u8, cap: 32 });
    let t_str = p.types.intern(Type::Str { cap: 64 });
    let t_line = p.types.intern(Type::Line { cap: 256 });
    let t_samples = p.types.intern(Type::Samples { elem: t_float, len: 100 });
    let t_table = p.types.intern(Type::Table { key: t_float, value: t_float });
    let t_mat = p.types.intern(Type::Mat { rows: 2, cols: 2, units: MatUnits::Uniform(None) });
    let t_mat_dim = p.types.intern(Type::Mat {
        rows: 2,
        cols: 1,
        units: MatUnits::Dimensioned { rows: vec![bar, psi], cols: vec![s] },
    });
    let t_map = p.types.intern(Type::Map { key: t_u16, value: t_int, cap: 8 });
    let t_opt = p.types.intern(Type::Optional(t_bar));
    let t_res = p.types.intern(Type::Result { ok: t_frame, err: e_err });
    let t_stream_line = p.types.intern(Type::Stream(t_line));
    let t_stream_frame = p.types.intern(Type::Stream(t_frame));
    let t_stream_u8 = p.types.intern(Type::Stream(t_u8));
    let t_stream_edge = p.types.intern(Type::Stream(t_edge));
    let t_capture = p.types.intern(Type::Capture { elem: t_float, len: 4096 });
    let t_stream_capture = p.types.intern(Type::Stream(t_capture));
    let t_job = p.types.intern(Type::Handle(HandleKind::Job));
    let t_trig = p.types.intern(Type::Handle(HandleKind::Trigger));
    let t_stream_msg = p.types.intern(Type::Stream(t_msg));

    // Enums und Records
    p.enums.push(EnumDef {
        name: "ValveCmd".into(),
        variants: vec![
            VariantDef { name: "CLOSED".into(), discriminant: 0, fields: vec![], span: sp(2) },
            VariantDef { name: "OPEN".into(), discriminant: 1, fields: vec![], span: sp(3) },
        ],
        layout: Some(IntWidth::U8),
        open: false,
        builtin: false,
        span: sp(4),
    });
    p.enums.push(EnumDef {
        name: "HeaderErr".into(),
        variants: vec![VariantDef { name: "MAGIC".into(), discriminant: 0, fields: vec![], span: sp(5) }],
        layout: None,
        open: true,
        builtin: false,
        span: sp(5),
    });
    p.enums.push(EnumDef {
        name: "BootMsg".into(),
        variants: vec![
            VariantDef {
                name: "ERASE".into(),
                discriminant: 0,
                fields: vec![FieldDef {
                    name: "sector".into(),
                    ty: t_int,
                    const_value: None,
                    offset: None,
                    len_field: None,
                    bits: vec![],
                    span: sp(6),
                }],
                span: sp(6),
            },
            VariantDef { name: "OTHER".into(), discriminant: 1, fields: vec![], span: sp(6) },
        ],
        layout: None,
        open: false,
        builtin: false,
        span: sp(6),
    });
    p.records.push(RecordDef {
        name: "CanFrame".into(),
        fields: vec![
            FieldDef {
                name: "id".into(),
                ty: t_u16,
                const_value: Some(Const::Int(0x50AA)),
                offset: Some(0),
                len_field: None,
                bits: vec![],
                span: sp(8),
            },
            FieldDef {
                name: "flags".into(),
                ty: t_u16,
                const_value: None,
                offset: None,
                len_field: None,
                bits: vec![BitfieldDef { name: "encrypted".into(), ty: t_bool, lo: 0, hi: 0, span: sp(8) }],
                span: sp(8),
            },
            FieldDef {
                name: "data".into(),
                ty: t_bytes,
                const_value: None,
                offset: None,
                len_field: Some(1),
                bits: vec![],
                span: sp(8),
            },
        ],
        layout: Some(WireLayout { endian: Endian::Little, align: Some(4) }),
        builtin: false,
        span: sp(8),
        wire_size: None,
    });
    p.records.push(RecordDef {
        name: "Edge".into(),
        fields: vec![FieldDef {
            name: "rising".into(),
            ty: t_bool,
            const_value: None,
            offset: None,
            len_field: None,
            bits: vec![],
            span: Span::default(),
        }],
        layout: Some(WireLayout { endian: Endian::Big, align: None }),
        builtin: true,
        span: Span::default(),
        wire_size: None,
    });

    // Channels, Streams, Parameter, Commands, Knoten
    let ch_p = p.add_channel(Channel {
        dir: Direction::Input,
        name: "tank_p".into(),
        ty: t_bar,
        binding: Binding::Hw(Address {
            segments: vec![
                AddressSegment { name: "daq1".into(), range: None },
                AddressSegment { name: "ai".into(), range: Some((0, 16)) },
            ],
        }),
        attrs: ChannelAttrs {
            max_age: Some(5_000_000),
            max_slew: Some(e(ExprKind::Float(50.0), t_float)),
            debounce: Some(3),
            ..Default::default()
        },
        meta: Meta {
            label: Some("Tankdruck".into()),
            display: Some(psi),
            group: Some("tank".into()),
            doc: Some("Druck".into()),
        },
        owner: None,
        span: sp(10),
    });
    let ch_valve = p.add_channel(Channel {
        dir: Direction::Output,
        name: "fuel_main".into(),
        ty: t_valve,
        binding: Binding::Sim(Address::simple("plc1/do0")),
        attrs: ChannelAttrs {
            safe: Some(e(ExprKind::Variant { enum_id: e_valve, variant: 0, fields: vec![] }, t_valve)),
            jitter: Some(20_000),
            irreversible: true,
            ..Default::default()
        },
        meta: Meta::default(),
        owner: Some(MachineId(0)),
        span: sp(11),
    });
    let ch_log = p.add_channel(Channel {
        dir: Direction::Input,
        name: "dut_log".into(),
        ty: t_stream_line,
        binding: Binding::None,
        attrs: ChannelAttrs {
            max_rate: Some(e(ExprKind::Float(2000.0), t_float)),
            capacity: Some(64),
            framing: Some(Framing::Lines),
            overflow: Some(Overflow::DropOldest),
            wake: true,
            capacity_bytes: Some(4096),
            expect_len: Some(80),
            ..Default::default()
        },
        meta: Meta::default(),
        owner: None,
        span: sp(12),
    });
    let ch_can = p.add_channel(Channel {
        dir: Direction::Input,
        name: "can_rx".into(),
        ty: t_stream_frame,
        binding: Binding::Hw(Address::simple("can0/rx")),
        attrs: ChannelAttrs { framing: Some(Framing::LengthPrefixed(IntWidth::U16)), ..Default::default() },
        meta: Meta::default(),
        owner: None,
        span: sp(13),
    });
    let ch_tx = p.add_channel(Channel {
        dir: Direction::Output,
        name: "dut_tx".into(),
        ty: t_stream_u8,
        binding: Binding::Hw(Address::simple("uart0/tx")),
        attrs: ChannelAttrs { framing: Some(Framing::Fixed(8)), overflow: Some(Overflow::Drop), ..Default::default() },
        meta: Meta::default(),
        owner: Some(MachineId(0)),
        span: sp(14),
    });
    let ch_edges = p.add_channel(Channel {
        dir: Direction::Input,
        name: "edges".into(),
        ty: t_stream_edge,
        binding: Binding::Hw(Address::simple("gpio/cap0")),
        attrs: ChannelAttrs { framing: Some(Framing::Raw), ..Default::default() },
        meta: Meta::default(),
        owner: None,
        span: sp(15),
    });
    let ch_samples = p.add_channel(Channel {
        dir: Direction::Input,
        name: "i_dut".into(),
        ty: t_samples,
        binding: Binding::Hw(Address::simple("daq1/ai2")),
        attrs: ChannelAttrs {
            rate: Some(e(ExprKind::Float(100_000.0), t_float)),
            framing: Some(Framing::Cobs),
            ..Default::default()
        },
        meta: Meta::default(),
        owner: None,
        span: sp(16),
    });
    let ch_wave = p.add_channel(Channel {
        dir: Direction::Input,
        name: "vbus_wave".into(),
        ty: t_stream_capture,
        binding: Binding::Hw(Address::simple("daq1/cap0")),
        attrs: ChannelAttrs::default(),
        meta: Meta::default(),
        owner: None,
        span: sp(17),
    });
    let st_q = StreamId(0);
    p.streams.push(Stream {
        name: "update_q".into(),
        elem: t_msg,
        capacity: 16,
        capacity_bytes: None,
        expect_len: None,
        overflow: Overflow::Fault,
        writer: Some(MachineId(0)),
        readers: vec![MachineId(1)],
        span: sp(18),
    });
    let _ = t_stream_msg;
    let pa_limit = ParamId(0);
    p.params.push(Param {
        name: "LIMIT".into(),
        ty: t_bar,
        default: e(ExprKind::Float(85.0), t_bar),
        tunable: false,
        meta: Meta { label: Some("Grenze".into()), ..Default::default() },
        span: sp(19),
    });
    let pa_kp = ParamId(1);
    p.params.push(Param {
        name: "KP".into(),
        ty: t_float,
        default: e(ExprKind::Float(0.5), t_float),
        tunable: true,
        meta: Meta::default(),
        span: sp(20),
    });
    let pr_qual = ProfileId(0);
    p.profiles.push(Profile {
        name: "QUAL".into(),
        assignments: vec![(pa_limit, e(ExprKind::Float(120.0), t_bar))],
        span: sp(21),
    });
    let cmd_start = CommandId(0);
    p.commands.push(Command { name: "start".into(), wake: true, meta: Meta::default(), span: sp(22) });
    let node_io1 = NodeId(0);
    p.nodes.push(Node {
        name: "io1".into(),
        address: Address::simple("ethercat/1"),
        tick: Some(1_000_000),
        span: sp(23),
    });

    // Funktionen, Natives, Bloecke
    let fn_clamp = FnId(0);
    p.fns.push(Fn {
        name: "clamp[bar]".into(),
        params: vec![
            FnParam { name: "x".into(), ty: t_bar, inout: false, default: None, span: sp(24) },
            FnParam {
                name: "lo".into(),
                ty: t_bar,
                inout: true,
                default: Some(e(ExprKind::Float(0.0), t_bar)),
                span: sp(24),
            },
        ],
        ret: Some(t_bar),
        locals: vec![
            VarDef { name: "x".into(), ty: t_bar, init: None, scope: VarScope::Param, public: false, span: sp(24) },
            VarDef { name: "lo".into(), ty: t_bar, init: None, scope: VarScope::Param, public: false, span: sp(24) },
            VarDef {
                name: "y".into(),
                ty: t_bar,
                init: Some(e(ExprKind::Var(VarId(0)), t_bar)),
                scope: VarScope::Local,
                public: false,
                span: sp(24),
            },
        ],
        body: Block::new(vec![
            stmt(StmtKind::Assign { target: Place::Var(VarId(2)), value: e(ExprKind::Var(VarId(1)), t_bar) }),
            stmt(StmtKind::Return(e(
                ExprKind::Cond {
                    cond: bx(e(
                        ExprKind::Binary {
                            op: BinaryOp::Lt,
                            lhs: bx(e(ExprKind::Var(VarId(0)), t_bar)),
                            rhs: bx(e(ExprKind::Var(VarId(1)), t_bar)),
                        },
                        t_bool,
                    )),
                    then: bx(e(ExprKind::Var(VarId(1)), t_bar)),
                    otherwise: bx(e(ExprKind::Var(VarId(2)), t_bar)),
                },
                t_bar,
            ))),
        ]),
        cost: Some(CostVec { i32: 1, i64: 2, f32: 3, f64: 4, mem: 5, call: 6, native: 7 }),
        stack: Some(16),
        origin: Some(GenericOrigin {
            template: "clamp".into(),
            args: vec![GenericArgVal::Unit(bar), GenericArgVal::Type(t_float), GenericArgVal::Const(4)],
        }),
        span: sp(24),
    });
    let fn_reset = FnId(1);
    p.fns.push(Fn {
        name: "start".into(),
        params: vec![],
        ret: None,
        locals: vec![],
        body: Block::default(),
        cost: None,
        stack: None,
        origin: None,
        span: sp(25),
    });
    let na_crc = NativeId(0);
    p.natives.push(Native {
        kind: NativeKind::Fn,
        name: "crc32".into(),
        params: vec![FnParam { name: "b".into(), ty: t_bytes, inout: false, default: None, span: sp(26) }],
        ret: t_int,
        cost: CostVec { i32: 6000, ..Default::default() },
        stack: 256,
        duration: None,
        total: true,
        from: None,
        origin: None,
        span: sp(26),
    });
    let na_verify = NativeId(1);
    p.natives.push(Native {
        kind: NativeKind::Job,
        name: "ecdsa_p256_verify".into(),
        params: vec![],
        ret: t_bool,
        cost: CostVec { i32: 300, ..Default::default() },
        stack: 2048,
        duration: Some(120_000_000),
        total: true,
        from: Some("crypto.rs".into()),
        origin: None,
        span: sp(27),
    });
    let bl_lowpass = BlockId(0);
    p.blocks.push(BlockDef {
        name: "lowpass[bar]".into(),
        params: vec![FnParam { name: "tau".into(), ty: t_dur_plain, inout: false, default: None, span: sp(28) }],
        state_vars: vec![VarDef {
            name: "y".into(),
            ty: t_bar,
            init: Some(e(ExprKind::Float(0.0), t_bar)),
            scope: VarScope::Local,
            public: false,
            span: sp(28),
        }],
        step: Some(fn_clamp),
        methods: vec![fn_reset],
        origin: Some(GenericOrigin { template: "lowpass".into(), args: vec![GenericArgVal::Unit(bar)] }),
        span: sp(28),
    });

    // Trigger, Eigenschaften, Kampagnen
    let tr_cut = TriggerId(0);
    p.triggers.push(Trigger {
        name: "cut_on_erase".into(),
        node: Some(node_io1),
        guard: Guard::Match {
            kind: MatchKind::Matches,
            subject: e(ExprKind::Input { channel: ch_log, dominated: false }, t_stream_line),
            pattern: Pattern::Text {
                pieces: vec![
                    PatternPiece::Text("Erasing sector ".into()),
                    PatternPiece::Capture { name: "n".into(), kind: CaptureKind::Int },
                ],
                dfa: Some(Dfa { classes: vec![0; 256], class_count: 1, table: vec![1, 1], accept: vec![1] }),
            },
            binding: None,
        },
        time: e(
            ExprKind::Binary {
                op: BinaryOp::Add,
                lhs: bx(e(
                    ExprKind::Accessor {
                        base: bx(e(ExprKind::Builtin(Builtin::Event), t_line)),
                        accessor: Accessor::T,
                        args: vec![],
                    },
                    t_dur_plain,
                )),
                rhs: bx(e(ExprKind::Duration(250_000), t_dur_plain)),
            },
            t_dur_plain,
        ),
        then: Block::new(vec![stmt(StmtKind::Assign {
            target: Place::Output(ch_valve),
            value: e(ExprKind::Variant { enum_id: e_valve, variant: 0, fields: vec![] }, t_valve),
        })]),
        bound: 20_000,
        span: sp(29),
    });
    let atom = |k: ExprKind| TProp::Atom(e(k, t_bool));
    p.properties.push(Property {
        name: "no_ignition_without_fuel".into(),
        formula: TProp::Temporal {
            op: TemporalOp::Always,
            window: None,
            inner: Box::new(TProp::Implies(
                Box::new(atom(ExprKind::Output(ch_valve))),
                Box::new(TProp::Or(
                    Box::new(TProp::Temporal {
                        op: TemporalOp::Eventually,
                        window: Some(10_000_000_000),
                        inner: Box::new(atom(ExprKind::Bool(true))),
                    }),
                    Box::new(TProp::And(
                        Box::new(TProp::Temporal {
                            op: TemporalOp::Stable,
                            window: Some(50_000_000),
                            inner: Box::new(atom(ExprKind::Bool(false))),
                        }),
                        Box::new(TProp::Not(Box::new(TProp::Temporal {
                            op: TemporalOp::Once,
                            window: Some(1_000_000_000),
                            inner: Box::new(TProp::Temporal {
                                op: TemporalOp::Never,
                                window: None,
                                inner: Box::new(atom(ExprKind::Command(cmd_start))),
                            }),
                        }))),
                    )),
                )),
            )),
        },
        monitor: true,
        span: sp(30),
    });
    p.campaigns.push(Campaign {
        name: "brownout_scan".into(),
        program: Some("supply_interruption.takt".into()),
        profile: Some(pr_qual),
        sweeps: vec![
            Sweep::Range {
                param: pa_limit,
                from: e(ExprKind::Float(2.0), t_bar),
                to: e(ExprKind::Float(5.0), t_bar),
                step: e(ExprKind::Float(0.25), t_bar),
            },
            Sweep::List {
                param: pa_kp,
                values: vec![e(ExprKind::Float(0.1), t_float), e(ExprKind::Float(0.2), t_float)],
            },
        ],
        repeat: 2,
        stop_on: StopOn::Fail,
        span: sp(31),
    });

    // Maschine mit allem
    let mut m = Machine::new("hotfire");
    m.driver = true;
    m.params = vec![FnParam {
        name: "travel".into(),
        ty: t_dur_plain,
        inout: false,
        default: Some(e(ExprKind::Duration(2_000_000_000), t_dur_plain)),
        span: sp(32),
    }];
    m.period = 10;
    m.phase = 2;
    m.follows = vec![MachineId(1)];
    m.node = Some(node_io1);
    m.meta = Meta { label: Some("Zuendung".into()), ..Default::default() };
    m.span = sp(33);
    let v_count = m.add_var(VarDef {
        name: "count".into(),
        ty: t_int,
        init: Some(e(ExprKind::Int(0), t_int)),
        scope: VarScope::Machine,
        public: true,
        span: sp(34),
    });
    let v_filter = m.add_var(VarDef {
        name: "filter".into(),
        ty: t_bar,
        init: Some(e(
            ExprKind::BlockInit {
                block: bl_lowpass,
                args: vec![e(ExprKind::Duration(50_000_000), t_dur_plain)],
                count: Some(8),
            },
            t_arr,
        )),
        scope: VarScope::Machine,
        public: false,
        span: sp(35),
    });
    let v_job = m.add_var(VarDef {
        name: "sig".into(),
        ty: t_job,
        init: None,
        scope: VarScope::Machine,
        public: false,
        span: sp(36),
    });
    let v_trig = m.add_var(VarDef {
        name: "trig".into(),
        ty: t_trig,
        init: None,
        scope: VarScope::Machine,
        public: false,
        span: sp(36),
    });
    let v_frame = m.add_var(VarDef {
        name: "frame".into(),
        ty: t_frame,
        init: Some(e(ExprKind::Default, t_frame)),
        scope: VarScope::Machine,
        public: false,
        span: sp(37),
    });
    let v_map = m.add_var(VarDef {
        name: "table".into(),
        ty: t_map,
        init: Some(e(ExprKind::Default, t_map)),
        scope: VarScope::Machine,
        public: false,
        span: sp(38),
    });
    let v_vec = m.add_var(VarDef {
        name: "buf".into(),
        ty: t_vec,
        init: Some(e(ExprKind::Default, t_vec)),
        scope: VarScope::Machine,
        public: false,
        span: sp(39),
    });
    let v_mat = m.add_var(VarDef {
        name: "pmat".into(),
        ty: t_mat,
        init: Some(e(
            ExprKind::Array(vec![e(
                ExprKind::Array(vec![e(ExprKind::Float(1.0), t_float), e(ExprKind::Float(0.0), t_float)]),
                t_arr,
            )]),
            t_mat,
        )),
        scope: VarScope::Machine,
        public: false,
        span: sp(40),
    });
    let v_res = m.add_var(VarDef {
        name: "hdr".into(),
        ty: t_res,
        init: None,
        scope: VarScope::Machine,
        public: false,
        span: sp(41),
    });
    let v_travel = m.add_var(VarDef {
        name: "travel".into(),
        ty: t_dur_plain,
        init: None,
        scope: VarScope::Param,
        public: false,
        span: sp(32),
    });
    m.persist.push(PersistVar { var: v_count, min_interval: Some(10_000_000_000), type_hash: 0xDEAD_BEEF });
    let sig_done = SignalId(0);
    m.signals.push(SignalDef { name: "done".into(), span: sp(42) });

    let s_run = m.add_state(State::new("RUN", None));
    let s_safe = m.add_state(State::new("SAFE", None));
    let s_manual = m.add_state(State::new("MANUAL", None));
    let s_coarse = m.add_state(State::new("COARSE", Some(s_manual)));
    let s_standby = m.add_state(State::new("STANDBY", None));
    m.initial = s_run;
    m.fault_target = FaultTarget::State(s_safe);
    let v_tries = m.add_var(VarDef {
        name: "tries".into(),
        ty: t_int,
        init: Some(e(ExprKind::Int(0), t_int)),
        scope: VarScope::State(s_run),
        public: false,
        span: sp(43),
    });
    let v_m = m.add_var(VarDef {
        name: "m".into(),
        ty: t_line,
        init: None,
        scope: VarScope::Lifted(s_run),
        public: false,
        span: sp(44),
    });
    let v_i = m.add_var(VarDef {
        name: "i".into(),
        ty: t_int,
        init: None,
        scope: VarScope::Lifted(s_run),
        public: false,
        span: sp(45),
    });
    let v_k = m.add_var(VarDef {
        name: "k".into(),
        ty: t_u16,
        init: None,
        scope: VarScope::Lifted(s_run),
        public: false,
        span: sp(45),
    });
    let v_x = m.add_var(VarDef {
        name: "x".into(),
        ty: t_float,
        init: None,
        scope: VarScope::Lifted(s_run),
        public: false,
        span: sp(45),
    });
    let v_sector = m.add_var(VarDef {
        name: "sector".into(),
        ty: t_int,
        init: None,
        scope: VarScope::Lifted(s_run),
        public: false,
        span: sp(46),
    });
    let v_done = m.add_var(VarDef {
        name: "done".into(),
        ty: t_bool,
        init: Some(e(ExprKind::Bool(false), t_bool)),
        scope: VarScope::Lifted(s_run),
        public: false,
        span: sp(47),
    });
    let v_rep = m.add_var(VarDef {
        name: "k_r".into(),
        ty: t_int,
        init: Some(e(ExprKind::Int(0), t_int)),
        scope: VarScope::Lifted(s_run),
        public: false,
        span: sp(48),
    });
    let v_ev = m.add_var(VarDef {
        name: "ev".into(),
        ty: t_edge,
        init: None,
        scope: VarScope::Lifted(s_run),
        public: false,
        span: sp(49),
    });
    m.layout = Layout {
        timers: vec![Timer { state: s_run, width: IntWidth::U32 }],
        every_counters: vec![CounterSite { state: Some(s_run), span: sp(50) }],
        viol_sites: vec![CounterSite { state: None, span: sp(51) }],
        block_instances: vec![BlockInstance { var: v_filter, block: bl_lowpass, count: 8 }],
        jobs_max: 2,
        job_slots: Vec::new(),
        saved_paths: vec![s_manual],
        trigger_flags: vec![tr_cut],
        output_queues: vec![ch_valve, ch_tx],
        cursors: vec![
            StreamRef::Channel(ch_log),
            StreamRef::Internal(st_q),
            StreamRef::Fired(tr_cut),
            StreamRef::Var(v_trig),
        ],
        scratch_bytes: Some(128),
    };
    m.budget = Some(Budget {
        activation: CostVec { i32: 10, ..Default::default() },
        fault_path: CostVec { mem: 3, ..Default::default() },
    });

    let input_p = || e(ExprKind::Input { channel: ch_p, dominated: true }, t_bar);
    let msg = |s: &str| Format::text(s);
    // Deklariertes Budget (7.2); Formatversion 6 traegt das Feld.
    m.declared_budget = Some(crate::machine::DeclaredBudget { ram: Some(4096), wcet_ns: None, span: Span::default() });
    m.loop_block = Block::new(vec![
        stmt(StmtKind::Check {
            cond: e(
                ExprKind::Binary { op: BinaryOp::Lt, lhs: bx(input_p()), rhs: bx(e(ExprKind::Param(pa_limit), t_bar)) },
                t_bool,
            ),
            message: Some(Format {
                pieces: vec![
                    FormatPiece::Text("overpressure ".into()),
                    FormatPiece::Expr { expr: input_p(), spec: Some(".3".into()) },
                ],
                len_max: 32,
            }),
            confirm: Some(Confirm { duration: e(ExprKind::Duration(5_000_000), t_dur_plain), site: SiteId(0) }),
            // `within 50 ms` (9.4.5); Formatversion 6 traegt das Feld.
            within: Some(e(ExprKind::Duration(50_000_000), t_dur_plain)),
            target: Some(Target::State(s_safe)),
            req: Some("SR-12".into()),
            kind: CheckKind::Check,
        }),
        stmt(StmtKind::Observe(Observe::Alert {
            cond: e(ExprKind::Bool(true), t_bool),
            message: msg("warm"),
            confirm: None,
        })),
    ]);
    m.handlers.push(Handler {
        guard: None,
        stream: StreamRef::Channel(ch_can),
        pattern: Some((
            MatchKind::Matches,
            Pattern::Record { record: r_frame, fields: vec![(0, e(ExprKind::Int(0x7E8), t_u16))] },
        )),
        binding: Some(v_frame),
        body: Block::new(vec![stmt(StmtKind::Raise(sig_done))]),
        span: sp(52),
    });
    m.faulted.transitions.push(Transition {
        trigger: TransTrigger::When(Guard::Expr(e(ExprKind::Command(cmd_start), t_bool))),
        actions: Block::default(),
        target: Target::State(s_run),
        kind: TransKind::Weak,
        span: sp(53),
    });

    let run = &mut m.states[s_run.index()];
    run.fault_target = Some(FaultTarget::Faulted);
    run.meta = Meta { doc: Some("Betrieb".into()), ..Default::default() };
    run.enter = Block::new(vec![
        stmt(StmtKind::Assign {
            target: Place::Output(ch_valve),
            value: e(ExprKind::Variant { enum_id: e_valve, variant: 1, fields: vec![] }, t_valve),
        }),
        stmt(StmtKind::At {
            time: e(
                ExprKind::Binary {
                    op: BinaryOp::Add,
                    lhs: bx(e(ExprKind::Builtin(Builtin::Now), t_dur_plain)),
                    rhs: bx(e(ExprKind::Duration(20_000), t_dur_plain)),
                },
                t_dur_plain,
            ),
            body: Block::new(vec![stmt(StmtKind::Assign {
                target: Place::Output(ch_valve),
                value: e(ExprKind::Output(ch_valve), t_valve),
            })]),
        }),
        stmt(StmtKind::Cancel(ch_valve)),
        stmt(StmtKind::Job { handle: v_job, native: na_verify, args: vec![] }),
        stmt(StmtKind::Arm { trigger: tr_cut, on: true }),
        stmt(StmtKind::Send {
            stream: StreamRef::Channel(ch_tx),
            value: e(ExprKind::Str("UPDATE\n".into()), t_str),
            len_max: 7,
        }),
        stmt(StmtKind::Send {
            stream: StreamRef::Internal(st_q),
            value: e(ExprKind::Variant { enum_id: e_msg, variant: 1, fields: vec![] }, t_msg),
            len_max: 1,
        }),
        stmt(StmtKind::Observe(Observe::Log(msg("entered")))),
        stmt(StmtKind::Observe(Observe::Measure {
            name: "boot_time".into(),
            value: e(ExprKind::Builtin(Builtin::TimeInState), t_dur),
        })),
        stmt(StmtKind::Observe(Observe::Verify {
            cond: e(ExprKind::Bool(true), t_bool),
            message: msg("ok"),
            req: Some("SR-1".into()),
        })),
        stmt(StmtKind::Observe(Observe::Verdict { pass: true, message: Some(msg("recovery ok")) })),
        stmt(StmtKind::Observe(Observe::Verdict { pass: false, message: None })),
        stmt(StmtKind::MethodCall {
            target: Some(Place::Var(v_x)),
            receiver: Place::Index(Box::new(Place::Var(v_filter)), e(ExprKind::Int(0), t_int)),
            method: Method::Step,
            args: vec![e(ExprKind::Float(1.0), t_float)],
        }),
        stmt(StmtKind::MethodCall {
            target: None,
            receiver: Place::Var(v_filter),
            method: Method::Reset,
            args: vec![],
        }),
        stmt(StmtKind::MethodCall {
            target: None,
            receiver: Place::Var(v_filter),
            method: Method::Block(fn_reset),
            args: vec![],
        }),
        stmt(StmtKind::MethodCall {
            target: Some(Place::Field(Box::new(Place::Var(v_frame)), 1)),
            receiver: Place::Var(v_vec),
            method: Method::Push,
            args: vec![e(ExprKind::Int(1), t_u8)],
        }),
        // `append` haengt eine ganze Folge an (3.9); Formatversion 5.
        stmt(StmtKind::MethodCall {
            target: None,
            receiver: Place::Var(v_vec),
            method: Method::Append,
            args: vec![e(ExprKind::Var(v_vec), t_vec)],
        }),
        stmt(StmtKind::MethodCall {
            target: None,
            receiver: Place::Var(v_map),
            method: Method::Insert,
            args: vec![e(ExprKind::Int(1), t_u16), e(ExprKind::Int(2), t_int)],
        }),
        stmt(StmtKind::MethodCall {
            target: None,
            receiver: Place::Var(v_map),
            method: Method::Remove,
            args: vec![e(ExprKind::Int(1), t_u16)],
        }),
        stmt(StmtKind::MethodCall { target: None, receiver: Place::Var(v_vec), method: Method::Clear, args: vec![] }),
        stmt(StmtKind::Skip(StreamRef::Var(v_trig))),
        // Ein Ausdruck mit Intervall und Darstellung (M3, 3.4).
        stmt(StmtKind::Assign { target: Place::Var(v_i), value: annotated(ExprKind::Int(3), t_int) }),
        stmt(StmtKind::Assign {
            target: Place::Index2(Box::new(Place::Var(v_mat)), e(ExprKind::Int(0), t_int), e(ExprKind::Int(1), t_int)),
            value: e(
                ExprKind::Index2 {
                    base: bx(e(ExprKind::Var(v_mat), t_mat)),
                    row: bx(e(ExprKind::Int(1), t_int)),
                    col: bx(e(ExprKind::Int(0), t_int)),
                },
                t_float,
            ),
        }),
        stmt(StmtKind::Pass),
    ]);
    run.exit =
        Block::new(vec![stmt(StmtKind::Abort { message: Some(msg("stop")) }), stmt(StmtKind::Abort { message: None })]);
    run.loop_block = Block::new(vec![
        stmt(StmtKind::Every {
            period: e(ExprKind::Duration(100_000_000), t_dur_plain),
            counter: CounterId(0),
            body: Block::new(vec![stmt(StmtKind::If {
                cond: e(
                    ExprKind::Accessor {
                        base: bx(e(ExprKind::Input { channel: ch_p, dominated: false }, t_bar)),
                        accessor: Accessor::Valid,
                        args: vec![],
                    },
                    t_bool,
                ),
                then: Block::new(vec![stmt(StmtKind::Goto(Target::State(s_safe)))]),
                otherwise: Block::new(vec![stmt(StmtKind::Goto(Target::Faulted))]),
            })]),
        }),
        stmt(StmtKind::ForRange {
            var: v_i,
            count: e(ExprKind::Int(16), t_int),
            body: Block::new(vec![stmt(StmtKind::Break)]),
        }),
        stmt(StmtKind::ForEach {
            vars: ForVars::One(v_x),
            iter: e(ExprKind::Input { channel: ch_samples, dominated: true }, t_samples),
            body: Block::default(),
        }),
        stmt(StmtKind::ForEach {
            vars: ForVars::Pair(v_k, v_i),
            iter: e(ExprKind::Var(v_map), t_map),
            body: Block::default(),
        }),
        stmt(StmtKind::ForEach {
            vars: ForVars::One(v_ev),
            iter: e(ExprKind::Input { channel: ch_edges, dominated: true }, t_stream_edge),
            body: Block::default(),
        }),
        stmt(StmtKind::Match {
            subject: e(
                ExprKind::Published {
                    machine: MachineRef { machine: MachineId(1), index: Some(bx(e(ExprKind::Var(v_i), t_int))) },
                    var: VarId(0),
                },
                t_msg,
            ),
            arms: vec![
                Arm {
                    pattern: ArmPattern::Variant { variant: 0, fields: vec![v_sector] },
                    body: Block::new(vec![stmt(StmtKind::Observe(Observe::Measure {
                        name: "erase_sector".into(),
                        value: e(ExprKind::Var(v_sector), t_int),
                    }))]),
                    span: sp(54),
                },
                Arm {
                    pattern: ArmPattern::Values(vec![
                        CaseValue { lo: e(ExprKind::Int(1), t_int), hi: Some(e(ExprKind::Int(3), t_int)) },
                        CaseValue { lo: e(ExprKind::Int(7), t_int), hi: None },
                    ]),
                    body: Block::default(),
                    span: sp(55),
                },
                Arm { pattern: ArmPattern::Wild, body: Block::new(vec![stmt(StmtKind::Pass)]), span: sp(56) },
            ],
        }),
        stmt(StmtKind::Check {
            cond: e(ExprKind::Bool(true), t_bool),
            message: None,
            confirm: None,
            within: None,
            target: None,
            req: None,
            kind: CheckKind::Expect,
        }),
    ]);
    run.handlers.push(Handler {
        guard: None,
        stream: StreamRef::Channel(ch_log),
        pattern: Some((
            MatchKind::Has,
            Pattern::Text {
                pieces: vec![
                    PatternPiece::Any,
                    PatternPiece::Text("CRC".into()),
                    PatternPiece::Capture { name: "rest".into(), kind: CaptureKind::Str(32) },
                ],
                dfa: None,
            },
        )),
        binding: Some(v_m),
        body: Block::default(),
        span: sp(57),
    });
    run.handlers.push(Handler {
        guard: None,
        stream: StreamRef::Channel(ch_wave),
        pattern: None,
        binding: None,
        body: Block::default(),
        span: sp(58),
    });
    let wide = vec![
        e(ExprKind::Lift(bx(input_p())), t_opt),
        e(ExprKind::Ok(bx(e(ExprKind::Var(v_frame), t_frame))), t_res),
        e(ExprKind::Err(bx(e(ExprKind::Variant { enum_id: e_err, variant: 0, fields: vec![] }, t_res))), t_res),
        e(ExprKind::Intrinsic { op: Intrinsic::Sqrt, args: vec![e(ExprKind::Float(4.0), t_float)] }, t_float),
        e(ExprKind::Intrinsic { op: Intrinsic::Round, args: vec![e(ExprKind::Var(v_x), t_float)] }, t_int),
        e(
            ExprKind::Intrinsic {
                op: Intrinsic::Fma,
                args: vec![
                    e(ExprKind::Float(1.0), t_float),
                    e(ExprKind::Float(2.0), t_float),
                    e(ExprKind::Float(3.0), t_float),
                ],
            },
            t_float,
        ),
        e(
            ExprKind::Intrinsic {
                op: Intrinsic::WrappingAdd,
                args: vec![e(ExprKind::Var(v_i), t_int), e(ExprKind::Int(1), t_int)],
            },
            t_int,
        ),
        e(
            ExprKind::Accessor {
                base: bx(e(ExprKind::Var(v_frame), t_frame)),
                accessor: Accessor::Encode,
                args: vec![],
            },
            t_bytes,
        ),
        e(ExprKind::Decode { record: r_frame, bytes: bx(e(ExprKind::Var(v_vec), t_bytes)) }, t_opt),
        e(
            ExprKind::Checked {
                expr: bx(e(
                    ExprKind::Binary {
                        op: BinaryOp::Div,
                        lhs: bx(e(ExprKind::Int(1), t_int)),
                        rhs: bx(e(ExprKind::Var(v_i), t_int)),
                    },
                    t_int,
                )),
                kind: CheckedKind::DivZero,
            },
            t_int,
        ),
        e(ExprKind::Checked { expr: bx(e(ExprKind::Int(1), t_int)), kind: CheckedKind::Overflow }, t_int),
        e(ExprKind::Checked { expr: bx(e(ExprKind::Float(1.0), t_float)), kind: CheckedKind::NonFinite }, t_float),
        e(ExprKind::Checked { expr: bx(e(ExprKind::Float(1.0), t_float)), kind: CheckedKind::Domain }, t_float),
        e(ExprKind::Checked { expr: bx(e(ExprKind::Var(v_i), t_int)), kind: CheckedKind::Index { len: 16 } }, t_int),
        e(ExprKind::Checked { expr: bx(e(ExprKind::Var(v_x), t_float)), kind: CheckedKind::Range(range) }, t_bar),
        e(
            ExprKind::Checked {
                expr: bx(e(ExprKind::Cast { expr: bx(e(ExprKind::Var(v_i), t_int)), to: t_u16 }, t_u16)),
                kind: CheckedKind::Convert,
            },
            t_u16,
        ),
        e(ExprKind::Checked { expr: bx(e(ExprKind::Var(v_i), t_int)), kind: CheckedKind::Shift }, t_int),
        e(
            ExprKind::Checked {
                expr: bx(e(ExprKind::Input { channel: ch_p, dominated: false }, t_bar)),
                kind: CheckedKind::Valid,
            },
            t_bar,
        ),
        e(ExprKind::Checked { expr: bx(e(ExprKind::Var(v_res), t_res)), kind: CheckedKind::Missing }, t_frame),
        e(ExprKind::Convert { expr: bx(input_p()), kind: ConvertKind::To, unit: psi }, t_psi),
        e(ExprKind::Convert { expr: bx(e(ExprKind::Var(v_i), t_i8)), kind: ConvertKind::ToFloat, unit: bar }, t_bar),
        e(
            ExprKind::Convert {
                expr: bx(e(ExprKind::Builtin(Builtin::Tick), t_dur_plain)),
                kind: ConvertKind::As,
                unit: s,
            },
            t_float,
        ),
        e(
            ExprKind::Matches {
                subject: bx(e(ExprKind::Var(v_m), t_line)),
                kind: MatchKind::Matches,
                pattern: Pattern::Text {
                    pieces: vec![
                        PatternPiece::Capture { name: "v".into(), kind: CaptureKind::Hex },
                        PatternPiece::Capture { name: "f".into(), kind: CaptureKind::Float },
                        PatternPiece::Capture { name: "w".into(), kind: CaptureKind::Word },
                    ],
                    dfa: None,
                },
                binding: Some(v_m),
            },
            t_bool,
        ),
        e(ExprKind::Call { callee: fn_clamp, args: vec![input_p(), e(ExprKind::Float(0.0), t_bar)] }, t_bar),
        e(ExprKind::NativeCall { native: na_crc, args: vec![e(ExprKind::Var(v_vec), t_bytes)] }, t_int),
        e(
            ExprKind::MatOp {
                op: MatOp::Inv,
                args: vec![e(
                    ExprKind::MatOp { op: MatOp::Transpose, args: vec![e(ExprKind::Var(v_mat), t_mat)] },
                    t_mat,
                )],
            },
            t_mat,
        ),
        e(ExprKind::MatOp { op: MatOp::Det, args: vec![e(ExprKind::Var(v_mat), t_mat_dim)] }, t_float),
        e(
            ExprKind::MatOp {
                op: MatOp::Solve,
                args: vec![e(ExprKind::Var(v_mat), t_mat), e(ExprKind::Var(v_mat), t_mat)],
            },
            t_mat,
        ),
        e(ExprKind::MatOp { op: MatOp::Cholesky, args: vec![e(ExprKind::Var(v_mat), t_mat)] }, t_opt),
        e(
            ExprKind::Accessor {
                base: bx(e(ExprKind::Var(v_i), t_int)),
                accessor: Accessor::Wrap(IntWidth::U16),
                args: vec![],
            },
            t_u16,
        ),
        e(
            ExprKind::Accessor {
                base: bx(e(ExprKind::Var(v_i), t_int)),
                accessor: Accessor::Bits,
                args: vec![e(ExprKind::Int(7), t_int), e(ExprKind::Int(4), t_int)],
            },
            t_int,
        ),
        e(
            ExprKind::Accessor { base: bx(e(ExprKind::Var(v_job), t_job)), accessor: Accessor::Done, args: vec![] },
            t_bool,
        ),
        e(
            ExprKind::Accessor { base: bx(e(ExprKind::Var(v_job), t_job)), accessor: Accessor::Result, args: vec![] },
            t_res,
        ),
        e(
            ExprKind::Accessor { base: bx(e(ExprKind::Var(v_trig), t_trig)), accessor: Accessor::Armed, args: vec![] },
            t_bool,
        ),
        e(
            ExprKind::Accessor {
                base: bx(e(ExprKind::Var(v_res), t_res)),
                accessor: Accessor::Or,
                args: vec![e(ExprKind::Default, t_frame)],
            },
            t_frame,
        ),
        e(ExprKind::Field { base: bx(e(ExprKind::Var(v_frame), t_frame)), field: 2 }, t_bytes),
        e(ExprKind::Index { base: bx(e(ExprKind::Var(v_vec), t_vec)), index: bx(e(ExprKind::Int(0), t_int)) }, t_u8),
        e(
            ExprKind::Slice {
                base: bx(e(ExprKind::Var(v_vec), t_vec)),
                from: bx(e(ExprKind::Int(0), t_int)),
                to: bx(e(ExprKind::Int(2), t_int)),
            },
            t_bytes,
        ),
        e(
            ExprKind::Unary {
                op: UnaryOp::Neg,
                expr: bx(e(ExprKind::Unary { op: UnaryOp::BitNot, expr: bx(e(ExprKind::Var(v_i), t_int)) }, t_int)),
            },
            t_int,
        ),
        e(ExprKind::Unary { op: UnaryOp::Not, expr: bx(e(ExprKind::Bool(false), t_bool)) }, t_bool),
        e(ExprKind::Tuple(bx(e(ExprKind::Float(3.0), t_float)), bx(e(ExprKind::Float(0.0), t_float))), t_table),
        e(
            ExprKind::Record {
                record: r_frame,
                fields: vec![e(ExprKind::Int(1), t_u16), e(ExprKind::Int(0), t_u16), e(ExprKind::Var(v_vec), t_bytes)],
            },
            t_frame,
        ),
        e(ExprKind::Variant { enum_id: e_msg, variant: 0, fields: vec![e(ExprKind::Int(3), t_int)] }, t_msg),
        e(ExprKind::StateOf(MachineRef { machine: MachineId(1), index: None }), t_int),
        e(ExprKind::Signal { machine: MachineRef { machine: MachineId(0), index: None }, signal: sig_done }, t_bool),
        e(ExprKind::Builtin(Builtin::LastFault), t_int),
        e(ExprKind::None, t_opt),
        e(ExprKind::Var(v_travel), t_dur_plain),
        e(ExprKind::Param(pa_kp), t_float),
        e(
            ExprKind::Accessor {
                base: bx(e(ExprKind::Input { channel: ch_wave, dominated: true }, t_capture)),
                accessor: Accessor::Samples,
                args: vec![],
            },
            t_arr,
        ),
    ];
    for (i, w) in wide.into_iter().enumerate() {
        let target = if i % 2 == 0 { Place::Var(v_x) } else { Place::Var(v_i) };
        run.loop_block.stmts.push(stmt(StmtKind::Assign { target, value: w }));
    }
    run.transitions.push(Transition {
        trigger: TransTrigger::When(Guard::Next { stream: StreamRef::Fired(tr_cut), binding: v_m }),
        actions: Block::new(vec![stmt(StmtKind::Assign {
            target: Place::Var(v_tries),
            value: e(ExprKind::Int(1), t_int),
        })]),
        target: Target::State(s_manual),
        kind: TransKind::Strong,
        span: sp(59),
    });
    run.transitions.push(Transition {
        trigger: TransTrigger::After(e(ExprKind::Var(v_travel), t_dur_plain)),
        actions: Block::default(),
        target: Target::Fault(FaultKind::Timeout),
        kind: TransKind::Weak,
        span: sp(60),
    });
    run.transitions.push(Transition {
        trigger: TransTrigger::When(Guard::Expr(e(ExprKind::Bool(false), t_bool))),
        actions: Block::default(),
        target: Target::Fault(FaultKind::Arithmetic(ArithKind::Singular)),
        kind: TransKind::Weak,
        span: sp(61),
    });
    run.transitions.push(Transition {
        trigger: TransTrigger::When(Guard::Expr(e(ExprKind::Bool(false), t_bool))),
        actions: Block::default(),
        target: Target::Fault(FaultKind::Runtime(RuntimeKind::Node)),
        kind: TransKind::Weak,
        span: sp(62),
    });
    run.sequence = Some(Sequence {
        items: vec![
            SeqItem::Stmt(stmt(StmtKind::Assign { target: Place::Var(v_count), value: e(ExprKind::Int(1), t_int) })),
            SeqItem::Wait(e(ExprKind::Duration(150_000_000), t_dur_plain)),
            SeqItem::Until {
                guard: Guard::Expr(e(ExprKind::Bool(true), t_bool)),
                timeout: Some(Timeout {
                    duration: e(ExprKind::Duration(500_000_000), t_dur_plain),
                    action: TimeoutAction::Fault,
                }),
                span: sp(63),
            },
            SeqItem::Until {
                guard: Guard::Match {
                    kind: MatchKind::Matches,
                    subject: e(ExprKind::Input { channel: ch_log, dominated: true }, t_stream_line),
                    pattern: Pattern::Text { pieces: vec![PatternPiece::Text("Boot".into())], dfa: None },
                    binding: Some(v_m),
                },
                timeout: Some(Timeout {
                    duration: e(ExprKind::Duration(2_000_000_000), t_dur_plain),
                    action: TimeoutAction::Goto(Target::State(s_safe)),
                }),
                span: sp(64),
            },
            SeqItem::Until {
                guard: Guard::Next { stream: StreamRef::Internal(st_q), binding: v_ev },
                timeout: Some(Timeout {
                    duration: e(ExprKind::Duration(1_000_000_000), t_dur_plain),
                    action: TimeoutAction::Else(Block::new(vec![
                        stmt(StmtKind::Observe(Observe::Verdict { pass: false, message: Some(msg("no completion")) })),
                        stmt(StmtKind::Goto(Target::State(s_safe))),
                    ])),
                }),
                span: sp(65),
            },
            SeqItem::Expect { cond: e(ExprKind::Bool(true), t_bool), message: Some(msg("expected")), span: sp(66) },
            SeqItem::Repeat {
                count: e(ExprKind::Int(3), t_int),
                counter: v_rep,
                body: vec![SeqItem::Step {
                    name: "erase".into(),
                    body: vec![SeqItem::Wait(e(ExprKind::Duration(1_000_000), t_dur_plain))],
                    span: sp(67),
                }],
                span: sp(68),
            },
            SeqItem::Stmt(stmt(StmtKind::Goto(Target::State(s_standby)))),
        ],
        done: v_done,
        span: sp(69),
    });
    let manual = &mut m.states[s_manual.index()];
    manual.resume = true;
    manual.initial = Some(s_coarse);
    manual.step_name = Some("manuell".into());
    manual.instances.push(ScopedInstance { machine: MachineId(1), scope: s_manual, resume: true, span: sp(70) });
    m.states[s_standby.index()].idle = true;
    let mid_main = p.add_machine(m);

    // Vorlage, Instanz, Szenario
    let mut template = Machine::new("cell_monitor");
    template.kind = MachineKind::Template;
    template.params = vec![FnParam { name: "v".into(), ty: t_bar, inout: false, default: None, span: sp(71) }];
    let t_run = template.add_state(State::new("RUN", None));
    template.initial = t_run;
    template.add_var(VarDef {
        name: "level".into(),
        ty: t_msg,
        init: Some(e(ExprKind::Variant { enum_id: e_msg, variant: 1, fields: vec![] }, t_msg)),
        scope: VarScope::Machine,
        public: true,
        span: sp(72),
    });
    let mid_template = p.add_machine(template);
    let mut instance = Machine::new("cells[0]");
    instance.kind =
        MachineKind::Instance(InstanceInfo { template: mid_template, args: vec![input_p()], array: Some((0, 8)) });
    let i_run = instance.add_state(State::new("RUN", None));
    instance.initial = i_run;
    p.add_machine(instance);
    let mut scenario = Machine::new("erase interrupted");
    scenario.kind = MachineKind::Scenario;
    let sc_run = scenario.add_state(State::new("RUN", None));
    scenario.initial = sc_run;
    p.add_machine(scenario);
    let _ = mid_main;
    p
}
