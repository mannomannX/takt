//! Ausdruecke (plan/m1.md 3.5, 3.6): `expr(ast, hint)` bestimmt den Typ
//! bidirektional und baut den MIR-Knoten; `coerce` prueft gegen einen
//! erwarteten Typ (Hebung `T` nach `T?`, Auspacken mit `Checked{Missing}`).

use takt_diag::{Span, Stage};
use takt_mir::expr::*;
use takt_mir::machine::{BlockInstance, VarDef, VarScope};
use takt_mir::stmt::{Method, Place, Stmt, StmtKind};
use takt_mir::types::{FloatWidth, HandleKind, IntWidth, Type};
use takt_mir::*;
use takt_syntax::ast;

use takt_mir::pattern::Pattern;

use super::{Lowerer, SC2, SC3, SC38, is_literal};
use crate::checks::{SC18, SC44, SC45, SC47};
use crate::symbols::Entity;
use crate::units::Unit;

impl Lowerer<'_> {
    /// Prueft einen Ausdruck gegen einen Typ.
    pub fn check(&mut self, e: &ast::Expr, ty: TypeId) -> Option<Expr> {
        let x = self.expr(e, Some(ty))?;
        self.coerce(x, ty)
    }

    /// Bool-Ausdruck.
    pub fn check_bool(&mut self, e: &ast::Expr) -> Option<Expr> {
        let ty = self.tys.bool;
        self.check(e, ty)
    }

    /// Koerzion auf einen Typ: gleich modulo Range, `T` → `T?` (Lift),
    /// `T?`/`T!E` → `T` (Auspacken, `Checked{Missing}`), sonst Fehler.
    pub fn coerce(&mut self, x: Expr, ty: TypeId) -> Option<Expr> {
        if self.same_base(x.ty, ty) {
            return Some(x);
        }
        // 8.3, 8.9: ein Modell speist einen oversampelten Kanal mit dem
        // Tick-Array, das der Treiber sonst liefert. Es hat dieselben
        // Elemente und dieselbe Laenge; nur der Name des Typs unterscheidet
        // sich, weil `samples` seine Herkunft nennt.
        if let (Type::Array { elem: a, len: n }, Type::Samples { elem: b, len: m }) =
            (self.ty(x.ty).clone(), self.ty(ty).clone())
        {
            if n == m && self.same_base(a, b) {
                let span = x.span;
                return Some(Expr::new(x.kind, ty, span));
            }
        }
        if let Type::Optional(inner) = self.ty(ty).clone() {
            if self.same_base(x.ty, inner) {
                let span = x.span;
                return Some(Expr::new(ExprKind::Lift(Box::new(x)), ty, span));
            }
        }
        match self.ty(x.ty).clone() {
            Type::Optional(inner) | Type::Result { ok: inner, .. } if self.same_base(inner, ty) => {
                let span = x.span;
                // 3.8: die unbewachte Verwendung bekommt `check x.valid` und
                // eine Warnung, damit `if x.valid:` oder `.or(…)` der
                // Normalfall bleibt. Unter Dominanz entfaellt beides.
                let key = Self::dom_key(&x);
                if !key.as_deref().is_some_and(|k| self.dominated(k)) {
                    let result = matches!(self.ty(x.ty), Type::Result { .. });
                    let (guard, alt) = if result { (".ok", "`check r.ok`") } else { (".valid", "`check x.valid`") };
                    self.warn_hint(
                        SC45,
                        span,
                        format!("Wert wird ohne `{guard}` benutzt; ein fehlender Wert faultet"),
                        format!("mit `{guard}` absichern, {alt} davorsetzen oder `.or(…)` benutzen (3.8)"),
                    );
                }
                return Some(Expr::new(
                    ExprKind::Checked { expr: Box::new(x), kind: CheckedKind::Missing },
                    inner,
                    span,
                ));
            }
            _ => {}
        }
        let (want, got) = (self.type_name(ty), self.type_name(x.ty));
        let mut hint = None;
        if is_literal(&x)
            && self.unit_of_type(ty).is_some_and(|u| !u.is_one())
            && self.unit_of_type(x.ty).is_some_and(|u| u.is_one())
        {
            if let Type::Float { unit: Some(u), .. } | Type::Int { unit: Some(u), .. } = self.ty(ty) {
                let text = match &x.kind {
                    ExprKind::Int(i) => i.to_string(),
                    ExprKind::Float(f) => format!("{f}"),
                    _ => "…".into(),
                };
                hint = Some(format!("meinst du `{text} {}`?", self.program.units[u.index()].name));
            }
        }
        let mut d = takt_diag::Diagnostic::error(SC3, x.span, format!("erwartet `{want}`, gefunden `{got}`"));
        if let Some(h) = hint {
            d = d.with_suggestion(h);
        }
        self.diags.push(d);
        None
    }

    /// Ausdruck mit Erwartung (nur Hinweis fuer Literale und Konstruktoren).
    pub fn expr(&mut self, e: &ast::Expr, hint: Option<TypeId>) -> Option<Expr> {
        let span = e.span;
        match &e.kind {
            ast::ExprKind::Number { value, unit } => self.number(value, unit.as_ref(), hint, span),
            ast::ExprKind::Duration(d) => {
                let ty = match hint {
                    Some(h) if self.is_duration(h) => h,
                    _ => self.tys.duration,
                };
                if let Some(r) = self.range_of(ty) {
                    if !takt_interp::eval::in_range(&takt_interp::Value::Duration(d.ns), &r) {
                        self.error(SC3, span, "Dauer ausserhalb der Range");
                        return None;
                    }
                }
                Some(Expr::new(ExprKind::Duration(d.ns), ty, span))
            }
            ast::ExprKind::Str(s) => {
                let len = s.value.len() as u32;
                let ty = match hint.map(|h| self.ty(h).clone()) {
                    Some(Type::Str { cap }) if cap >= len => hint.expect("Hinweis"),
                    Some(Type::Line { cap }) if cap >= len => hint.expect("Hinweis"),
                    _ => self.intern(Type::Str { cap: len }),
                };
                Some(Expr::new(ExprKind::Str(s.value.clone()), ty, span))
            }
            ast::ExprKind::Bool(b) => Some(Expr::new(ExprKind::Bool(*b), self.tys.bool, span)),
            ast::ExprKind::None => match hint.map(|h| self.ty(h).clone()) {
                Some(Type::Optional(_)) => Some(Expr::new(ExprKind::None, hint.expect("Hinweis"), span)),
                _ => {
                    self.error_hint(SC3, span, "Typ von `none` nicht ableitbar", "Variable mit `T?` annotieren");
                    None
                }
            },
            ast::ExprKind::Default => match hint {
                Some(h) => {
                    if matches!(self.ty(h), Type::Stream(_) | Type::Handle(_) | Type::Capture { .. }) {
                        self.error(SC3, span, "`default` nur fuer POD-Typen (3.7)");
                        return None;
                    }
                    Some(Expr::new(ExprKind::Default, h, span))
                }
                None => {
                    self.error_hint(SC3, span, "Typ von `default` nicht ableitbar", "Variable annotieren");
                    None
                }
            },
            ast::ExprKind::Ident(name) => self.ident(name, span),
            ast::ExprKind::Call { callee, generics, args } => self.call(callee, generics, args, hint, span),
            ast::ExprKind::Upper { name, args } => self.upper(name, args.as_deref(), hint, span),
            ast::ExprKind::TypeName { name, args } => match args {
                Some(args) => self.record_ctor(name, args, span),
                None => {
                    self.error_hint(
                        SC3,
                        span,
                        format!("`{}` ist ein Typ, kein Wert", name.name),
                        "Konstruktor `Typ(…)` oder `Typ.decode(…)`",
                    );
                    None
                }
            },
            ast::ExprKind::Paren(inner) => self.expr(inner, hint),
            ast::ExprKind::Tuple(a, b) => {
                let (kt, vt) = match hint.map(|h| self.ty(h).clone()) {
                    Some(Type::Table { key, value }) => (Some(key), Some(value)),
                    _ => (None, None),
                };
                let a = self.expr(a, kt)?;
                let a = match kt {
                    Some(t) => self.coerce(a, t)?,
                    None => a,
                };
                let b = self.expr(b, vt)?;
                let b = match vt {
                    Some(t) => self.coerce(b, t)?,
                    None => b,
                };
                let ty = match hint {
                    Some(h) if matches!(self.ty(h), Type::Table { .. }) => h,
                    _ => self.intern(Type::Table { key: a.ty, value: b.ty }),
                };
                Some(Expr::new(ExprKind::Tuple(Box::new(a), Box::new(b)), ty, span))
            }
            ast::ExprKind::Array(items) => self.array(items, hint, span),
            ast::ExprKind::InstanceArray { count, template, args } => {
                let n = self.const_int(count)?;
                if n <= 0 || n > 1024 {
                    self.error(SC3, count.span, "Zahl der Instanzen ausserhalb 1..1024");
                    return None;
                }
                let mut init = self.block_init(template, &[], args, span)?;
                if let ExprKind::BlockInit { count, .. } = &mut init.kind {
                    *count = Some(n as u32);
                }
                init.ty = self.intern(Type::Array { elem: init.ty, len: n as u32 });
                Some(init)
            }
            ast::ExprKind::Member { base, name, args } => self.member(base, name, args.as_deref(), hint, span),
            ast::ExprKind::Index { base, index } => self.index(base, index, span),
            ast::ExprKind::Slice { base, from, to } => {
                let b = self.expr(base, None)?;
                match self.ty(b.ty).clone() {
                    Type::Bytes { .. } | Type::Vec { .. } | Type::Array { .. } => {}
                    _ => {
                        let n = self.type_name(b.ty);
                        self.error(SC3, span, format!("Slice auf `{n}`"));
                        return None;
                    }
                }
                let int = self.tys.int;
                let from = self.check(from, int)?;
                let to = self.check(to, int)?;
                let ty = match self.ty(b.ty).clone() {
                    Type::Array { elem, .. } => self.intern(Type::Vec { elem, cap: 0 }),
                    other => self.intern(other),
                };
                Some(Expr::new(ExprKind::Slice { base: Box::new(b), from: Box::new(from), to: Box::new(to) }, ty, span))
            }
            ast::ExprKind::Index2 { base, row, col } => self.mat_index(base, row, col, span),
            ast::ExprKind::Cast { expr, ty } => self.cast(expr, ty, span),
            ast::ExprKind::Unary { op, expr } => self.unary(*op, expr, hint, span),
            ast::ExprKind::Binary { op, lhs, rhs } => self.binary(*op, lhs, rhs, hint, span),
            ast::ExprKind::Match { subject, kind, pattern, binding } => {
                self.matches(subject, *kind, pattern, binding.as_ref(), span)
            }
            ast::ExprKind::Conditional { then, cond, otherwise } => {
                let c = self.check_bool(cond)?;
                let facts = Self::facts_of(&c);
                self.facts.push(facts);
                let a = self.expr(then, hint);
                self.facts.pop();
                let a = a?;
                let b = self.expr(otherwise, Some(a.ty))?;
                let b = self.coerce(b, a.ty)?;
                let ty = self.base(a.ty);
                Some(Expr::new(
                    ExprKind::Cond { cond: Box::new(c), then: Box::new(a), otherwise: Box::new(b) },
                    ty,
                    span,
                ))
            }
            ast::ExprKind::Temporal { .. } | ast::ExprKind::Implies { .. } => {
                self.error(SC3, span, "Temporaloperatoren nur in `property` (13.3)");
                None
            }
        }
    }

    // ------------------------------------------------------------ Literale

    fn number(
        &mut self,
        value: &ast::Number,
        unit: Option<&ast::UnitExpr>,
        hint: Option<TypeId>,
        span: Span,
    ) -> Option<Expr> {
        let text = match value {
            ast::Number::Int(i) => i.text.replace('_', ""),
            ast::Number::Float(f) => f.text.replace('_', ""),
        };
        let is_int = matches!(value, ast::Number::Int(_));
        let unit = match unit {
            Some(u) => Some(self.unit_expr(u)?),
            None => None,
        };
        let hint_ty = hint.map(|h| self.ty(h).clone());
        // Ganzzahlliteral in Ganzzahlkontext
        if is_int && unit.is_none() {
            if let Some(Type::Int { width, range, unit: hu }) = hint_ty {
                if let Some(id) = hu.filter(|_| !is_zero(&text) && !self.in_range_bound) {
                    let name = self.program.units[id.index()].name.clone();
                    self.error_hint(
                        SC3,
                        span,
                        "einheitenloses Literal in Einheitenkontext (3.6)",
                        format!("meinst du `{text} {name}`?"),
                    );
                    return None;
                }
                let v = parse_int(&text).or_else(|| {
                    self.error(SC3, span, "Ganzzahlliteral nicht darstellbar");
                    None
                })?;
                let (lo, hi) = takt_interp::arith::bounds(width);
                if v < lo || v > hi {
                    self.error(SC3, span, format!("Literal passt nicht in `{}`", takt_interp::arith::name(width)));
                    return None;
                }
                if let Some(r) = range {
                    if !takt_interp::eval::in_range(&takt_interp::Value::Int(v as i64), &r) {
                        self.error(SC3, span, "Literal ausserhalb der Range");
                        return None;
                    }
                }
                return Some(Expr::new(ExprKind::Int(v as i64), hint.expect("Hinweis"), span));
            }
            if !matches!(hint_ty, Some(Type::Float { .. })) {
                let v = parse_int(&text).filter(|v| i64::try_from(*v).is_ok()).or_else(|| {
                    self.error(SC3, span, "Ganzzahlliteral passt nicht in `int`");
                    None
                })?;
                return Some(Expr::new(ExprKind::Int(v as i64), self.tys.int, span));
            }
        }
        // Ganzzahlliteral mit Einheit in Ganzzahlkontext (3.2, v1.1)
        if let (Some(Type::Int { width, unit: hu, range }), Some(u)) = (&hint_ty, &unit) {
            if !is_int {
                self.error(SC3, span, "Ganzzahlliteral erwartet");
                return None;
            }
            let v = parse_int(&text).or_else(|| {
                self.error(SC3, span, "Ganzzahlliteral nicht darstellbar");
                None
            })?;
            let (lo, hi) = takt_interp::arith::bounds(*width);
            if v < lo || v > hi {
                self.error(SC3, span, format!("Literal passt nicht in `{}`", takt_interp::arith::name(*width)));
                return None;
            }
            let hunit = hu.map(|id| self.units.unit_of(id)).unwrap_or_else(Unit::one);
            let ty = if hunit == *u {
                if let Some(r) = range {
                    if !takt_interp::eval::in_range(&takt_interp::Value::Int(v as i64), r) {
                        self.error(SC3, span, "Literal ausserhalb der Range");
                        return None;
                    }
                }
                hint.expect("Hinweis")
            } else {
                self.int_type(*width, u, None, span)?
            };
            return Some(Expr::new(ExprKind::Int(v as i64), ty, span));
        }
        // Fliesskomma (auch Ganzzahltext in Fliesskommakontext)
        if is_int && unit.is_none() && matches!(hint_ty, Some(Type::Int { .. })) {
            unreachable!("oben behandelt");
        }
        let (width, target_unit, ty) = match (&hint_ty, &unit) {
            (Some(Type::Float { width, unit: hu, range: _ }), None) => {
                let hunit = hu.map(|id| self.units.unit_of(id)).unwrap_or_else(Unit::one);
                if !hunit.is_one() && !is_zero(&text) && !self.in_range_bound {
                    let name = self.program.units[hu.expect("Einheit").index()].name.clone();
                    self.error_hint(
                        SC3,
                        span,
                        "einheitenloses Literal in Einheitenkontext (3.6)",
                        format!("meinst du `{text} {name}`?"),
                    );
                    return None;
                }
                (*width, hunit, hint.expect("Hinweis"))
            }
            (Some(Type::Float { width, unit: hu, .. }), Some(u)) => {
                let hunit = hu.map(|id| self.units.unit_of(id)).unwrap_or_else(Unit::one);
                if hunit == *u {
                    (*width, u.clone(), hint.expect("Hinweis"))
                } else {
                    let w = *width;
                    let ty = self.float_type(w, u, None, span)?;
                    (w, u.clone(), ty)
                }
            }
            (Some(Type::Duration { .. }), _) => {
                self.error_hint(SC3, span, "Dauer erwartet", "Zeiteinheit anfuegen, etwa `100 ms`");
                return None;
            }
            (_, Some(u)) => {
                let w = self.float_width();
                let ty = self.float_type(w, u, None, span)?;
                (w, u.clone(), ty)
            }
            (_, None) => (self.float_width(), Unit::one(), self.tys.float),
        };
        let _ = target_unit;
        let v = parse_float(&text, width).or_else(|| {
            self.error(SC3, span, "Fliesskommaliteral nicht darstellbar");
            None
        })?;
        if let Some(r) = self.range_of(ty) {
            if !takt_interp::eval::in_range(&takt_interp::Value::F64(v), &r) {
                self.error(SC3, span, "Literal ausserhalb der Range");
                return None;
            }
        }
        Some(Expr::new(ExprKind::Float(v), ty, span))
    }

    // ------------------------------------------------------------ Namen

    /// Lesen eines Inputs: mit `Checked{Valid}`, sofern nicht dominiert oder
    /// roh (Wrapper-Zugriffe, Elementzugriff).
    pub fn input_read(&mut self, channel: ChannelId, span: Span, raw: bool) -> Expr {
        let ty = self.program.channels[channel.index()].ty;
        let key = format!("in{}", channel.0);
        let dominated = self.dominated(&key);
        let read = Expr::new(ExprKind::Input { channel, dominated: dominated || raw }, ty, span);
        if raw || dominated {
            read
        } else {
            Expr::new(ExprKind::Checked { expr: Box::new(read), kind: CheckedKind::Valid }, ty, span)
        }
    }

    fn ident(&mut self, name: &ast::Ident, span: Span) -> Option<Expr> {
        match self.lookup(name)? {
            Entity::Var(v, ty) => Some(Expr::new(ExprKind::Var(v), ty, span)),
            Entity::Param(p, ty) => Some(Expr::new(ExprKind::Param(p), ty, span)),
            Entity::Const(e) => Some(Expr { span, ..e }),
            Entity::Command(c) => Some(Expr::new(ExprKind::Command(c), self.tys.bool, span)),
            Entity::Channel(c) => {
                let ch = &self.program.channels[c.index()];
                // Ein Stream als Ausdruck ist der Strom selbst: Subjekt eines
                // Guards, Traeger von `.count`, `.dropped` und Verwandten
                // (8.6). Ein Abtastfenster ist ein gewoehnlicher Wert (8.9).
                if matches!(self.ty(ch.ty), Type::Stream(_)) {
                    let ty = ch.ty;
                    return Some(Expr::new(ExprKind::Input { channel: c, dominated: true }, ty, span));
                }
                match ch.dir {
                    takt_mir::program::Direction::Input => Some(self.input_read(c, span, false)),
                    takt_mir::program::Direction::Output => Some(Expr::new(ExprKind::Output(c), ch.ty, span)),
                }
            }
            Entity::Builtin(b) => {
                let ty = match b {
                    Builtin::Now | Builtin::Tick | Builtin::TimeInState => self.tys.duration,
                    Builtin::LastFault => self.tys.last_fault_ty,
                    // 7.5: `event` ist das Element, das den Guard erfuellt
                    // hat — mit seinen Captures und `.t`. Ausserhalb eines
                    // Triggers gibt es kein Ereignis.
                    Builtin::Event => match self.event_ty {
                        Some(ty) => ty,
                        None => {
                            self.error(SC3, span, "`event` gibt es nur im `then`-Teil eines Triggers (7.5)");
                            return None;
                        }
                    },
                };
                if matches!(b, Builtin::TimeInState | Builtin::LastFault) && self.mctx.is_none() {
                    self.error(SC3, span, format!("`{}` nur in Maschinen", name.name));
                    return None;
                }
                Some(Expr::new(ExprKind::Builtin(b), ty, span))
            }
            Entity::Machine(_) | Entity::MachineTemplate(_) => {
                self.error_hint(
                    SC3,
                    span,
                    format!("`{}` ist eine Maschine, kein Wert", name.name),
                    "`.state`, eine `pub var` oder ein Signal lesen",
                );
                None
            }
            Entity::Stream(sid) => {
                // Ein interner Stream als Wert (8.6): Subjekt eines Guards
                // und Traeger seiner Zaehler.
                let elem = self.program.streams[sid.index()].elem;
                let ty = self.intern(Type::Stream(elem));
                Some(Expr::new(ExprKind::Stream(sid), ty, span))
            }
            Entity::Intrinsic(i) => {
                self.error(SC3, span, format!("`{}` ist eine Funktion; Aufruf mit `(...)`", i.name()));
                None
            }
            // 12.9: Ein Portlesen ist sofort und nur in einer `driver machine`.
            Entity::Port(p) => {
                if !self.mctx.as_ref().is_some_and(|m| m.machine.driver) {
                    self.error_hint(
                        crate::checks::SC64,
                        span,
                        format!("`{}` ist ein Port und nur in einer `driver machine` erreichbar (12.9)", name.name),
                        "`driver machine` erklaert, dass die Maschine Register anfasst",
                    );
                    return None;
                }
                let ty = self.program.ports[p.index()].ty;
                self.program.ports[p.index()].owner = self.mctx.as_ref().map(|m| m.id);
                Some(Expr::new(ExprKind::PortRead(p), ty, span))
            }
            _ => {
                self.error(SC3, span, format!("`{}` ist kein Wert", name.name));
                None
            }
        }
    }

    fn upper(
        &mut self,
        name: &ast::Ident,
        args: Option<&[ast::Arg]>,
        hint: Option<TypeId>,
        span: Span,
    ) -> Option<Expr> {
        // 3.12: Eine Konstantenvariable ist in ihrer Instanz eine Konstante
        // — in Typen, in `range(N)` und in der Rechnung.
        if args.is_none() {
            if let Some(n) = self.env.const_value(&name.name) {
                let int = self.tys.int;
                return Some(Expr::new(ExprKind::Int(n), int, span));
            }
        }
        // typgefuehrt: Variante des erwarteten Enums oder OK/ERR eines T!E
        if let Some(h) = hint {
            match self.ty(h).clone() {
                Type::Enum(e) => {
                    if let Some(v) = self.program.enums[e.index()].variants.iter().position(|v| v.name == name.name) {
                        return self.variant(e, v as u32, args, span, h);
                    }
                }
                Type::Result { ok, err } if (name.name == "OK" || name.name == "ERR") => {
                    let args = args.unwrap_or(&[]);
                    if args.len() != 1 {
                        self.error(SC3, span, format!("`{}` verlangt genau ein Argument", name.name));
                        return None;
                    }
                    let inner = if name.name == "OK" { ok } else { self.intern(Type::Enum(err)) };
                    let value = self.check(&args[0].value, inner)?;
                    let kind =
                        if name.name == "OK" { ExprKind::Ok(Box::new(value)) } else { ExprKind::Err(Box::new(value)) };
                    return Some(Expr::new(kind, h, span));
                }
                _ => {}
            }
        }
        match self.peek(&name.name).cloned() {
            Some(Entity::Const(e)) if args.is_none() => return Some(Expr { span, ..e }),
            Some(Entity::Param(p, ty)) if args.is_none() => return Some(Expr::new(ExprKind::Param(p), ty, span)),
            Some(Entity::State(_)) => {
                self.error_hint(
                    SC3,
                    span,
                    format!("`{}` ist ein Zustand", name.name),
                    "`m.state == ZUSTAND` vergleicht mit dem Zustandstyp",
                );
                return None;
            }
            _ => {}
        }
        // eindeutige Variante ueber alle Enums
        let candidates: Vec<(EnumId, u32)> = self
            .program
            .enums
            .iter()
            .enumerate()
            .filter(|(_, e)| !e.name.contains('.'))
            .flat_map(|(i, e)| {
                e.variants
                    .iter()
                    .enumerate()
                    .filter(|(_, v)| v.name == name.name)
                    .map(move |(j, _)| (EnumId(i as u32), j as u32))
            })
            .collect();
        match candidates.as_slice() {
            [(e, v)] => {
                let ty = self.intern(Type::Enum(*e));
                self.variant(*e, *v, args, span, ty)
            }
            [] => {
                if name.name == "OK" || name.name == "ERR" {
                    self.error_hint(
                        SC3,
                        span,
                        format!("Typ von `{}(…)` nicht ableitbar", name.name),
                        "Rueckgabetyp `T!E` oder Annotation",
                    );
                    return None;
                }
                self.lookup(name);
                None
            }
            many => {
                let names: Vec<String> = many.iter().map(|(e, _)| self.program.enums[e.index()].name.clone()).collect();
                self.error_hint(
                    SC3,
                    span,
                    format!("`{}` ist mehrdeutig: {}", name.name, names.join(", ")),
                    "Typ durch Annotation oder Vergleich festlegen",
                );
                None
            }
        }
    }

    fn variant(&mut self, e: EnumId, v: u32, args: Option<&[ast::Arg]>, span: Span, ty: TypeId) -> Option<Expr> {
        let def = &self.program.enums[e.index()].variants[v as usize];
        let params: Vec<(String, TypeId, Option<Expr>)> =
            def.fields.iter().map(|f| (f.name.clone(), f.ty, None)).collect();
        let fields = match args {
            None if params.is_empty() => Vec::new(),
            None => {
                self.error(SC3, span, format!("Variante `{}` verlangt Felder", def.name));
                return None;
            }
            Some(args) => self.args(&params, args, span)?,
        };
        Some(Expr::new(ExprKind::Variant { enum_id: e, variant: v, fields }, ty, span))
    }

    fn record_ctor(&mut self, name: &ast::Ident, args: &[ast::Arg], span: Span) -> Option<Expr> {
        let Entity::Record(r) = self.lookup(name)? else {
            self.error(SC3, span, format!("`{}` ist kein Record", name.name));
            return None;
        };
        let def = &self.program.records[r.index()];
        let params: Vec<(String, TypeId, Option<Expr>)> = def
            .fields
            .iter()
            .filter(|f| f.name != "_")
            .map(|f| (f.name.clone(), f.ty, f.const_value.map(|c| const_expr(c, f.ty, span))))
            .collect();
        // Padding (3.7) nennt kein Argument, steht aber im Wert: `default`.
        // So traegt jedes Literal alle Felder, und Interpreter wie Codegen
        // sehen dieselbe Form (`encode`, Struct-Literal).
        let padding: Vec<(usize, TypeId)> =
            def.fields.iter().enumerate().filter(|(_, f)| f.name == "_").map(|(i, f)| (i, f.ty)).collect();
        let mut fields = self.args(&params, args, span)?;
        for (i, ty) in padding {
            fields.insert(i, Expr::new(ExprKind::Default, ty, span));
        }
        let ty = self.intern(Type::Record(r));
        Some(Expr::new(ExprKind::Record { record: r, fields }, ty, span))
    }

    /// Argumente gegen Parameter: positional, dann benannt; Defaults fuellen.
    pub fn args(
        &mut self,
        params: &[(String, TypeId, Option<Expr>)],
        args: &[ast::Arg],
        span: Span,
    ) -> Option<Vec<Expr>> {
        let mut slots: Vec<Option<Expr>> = vec![None; params.len()];
        let mut ok = true;
        for (i, a) in args.iter().enumerate() {
            let idx = match &a.name {
                None => {
                    if i >= params.len() {
                        self.error(SC3, a.span, format!("zu viele Argumente ({} erwartet)", params.len()));
                        ok = false;
                        continue;
                    }
                    i
                }
                Some(n) => match params.iter().position(|(p, _, _)| *p == n.name) {
                    Some(idx) => idx,
                    None => {
                        let names: Vec<&str> = params.iter().map(|(p, _, _)| p.as_str()).collect();
                        self.error_hint(
                            SC3,
                            n.span,
                            format!("unbekanntes Argument `{}`", n.name),
                            format!("Parameter: {}", names.join(", ")),
                        );
                        ok = false;
                        continue;
                    }
                },
            };
            if slots[idx].is_some() {
                self.error(SC3, a.span, format!("Argument `{}` doppelt", params[idx].0));
                ok = false;
                continue;
            }
            match self.check(&a.value, params[idx].1) {
                Some(e) => slots[idx] = Some(e),
                None => ok = false,
            }
        }
        for (i, slot) in slots.iter_mut().enumerate() {
            if slot.is_none() {
                match &params[i].2 {
                    Some(d) => *slot = Some(d.clone()),
                    None => {
                        self.error(SC3, span, format!("Argument `{}` fehlt", params[i].0));
                        ok = false;
                    }
                }
            }
        }
        if !ok {
            return None;
        }
        Some(slots.into_iter().map(|s| s.expect("gefuellt")).collect())
    }

    // ------------------------------------------------------------ Aufrufe

    fn call(
        &mut self,
        callee: &ast::Ident,
        generics: &[ast::GenericArg],
        args: &[ast::Arg],
        hint: Option<TypeId>,
        span: Span,
    ) -> Option<Expr> {
        if callee.name == "range" {
            self.error(SC3, span, "`range(N)` nur in `for`");
            return None;
        }
        if callee.name == "solve" && self.peek("solve").is_none() {
            return self.mat_solve(args, span);
        }
        let entity = self.lookup(callee)?;
        match entity {
            Entity::Fn(id) => {
                if !generics.is_empty() {
                    self.error(SC3, span, format!("`{}` ist nicht generisch", callee.name));
                    return None;
                }
                let f = &self.program.fns[id.index()];
                let params: Vec<(String, TypeId, Option<Expr>)> =
                    f.params.iter().map(|p| (p.name.clone(), p.ty, p.default.clone())).collect();
                let ret = f.ret.unwrap_or(self.tys.bool);
                let inout: Vec<bool> = f.params.iter().map(|p| p.inout).collect();
                let args = self.args(&params, args, span)?;
                self.check_no_aliasing(&inout, &args);
                Some(Expr::new(ExprKind::Call { callee: id, args }, ret, span))
            }
            Entity::FnTemplate(idx) => self.call_fn_template(idx, generics, args, span),
            Entity::Native(id) => {
                let n = &self.program.natives[id.index()];
                let params: Vec<(String, TypeId, Option<Expr>)> =
                    n.params.iter().map(|p| (p.name.clone(), p.ty, p.default.clone())).collect();
                let ret = n.ret;
                if n.kind == takt_mir::fns::NativeKind::Job {
                    self.error_hint(
                        SC3,
                        span,
                        format!("`{}` ist ein Job", callee.name),
                        "`job v = …` als Anweisung (4.5)",
                    );
                    return None;
                }
                let args = self.args(&params, args, span)?;
                Some(Expr::new(ExprKind::NativeCall { native: id, args }, ret, span))
            }
            Entity::Block(_) | Entity::BlockTemplate(_) => self.block_init(callee, generics, args, span),
            Entity::Intrinsic(op) => self.intrinsic(op, args, hint, span),
            _ => {
                self.error(SC3, span, format!("`{}` ist nicht aufrufbar", callee.name));
                None
            }
        }
    }

    /// Blockinstanz `lowpass[bar](tau = 50 ms)`.
    pub fn block_init(
        &mut self,
        name: &ast::Ident,
        generics: &[ast::GenericArg],
        args: &[ast::Arg],
        span: Span,
    ) -> Option<Expr> {
        let block = match self.lookup(name)? {
            Entity::Block(b) => {
                if !generics.is_empty() {
                    self.error(SC3, span, format!("`{}` ist nicht generisch", name.name));
                    return None;
                }
                b
            }
            Entity::BlockTemplate(idx) => self.instantiate_block_template(idx, generics, args, span)?,
            _ => {
                self.error(SC3, span, format!("`{}` ist kein Block", name.name));
                return None;
            }
        };
        let def = &self.program.blocks[block.index()];
        // 5.7: `if rose(start):` — ein Block ohne Konstruktorparameter, mit
        // Argumenten gerufen, ist eine anonyme Instanz je Aufrufstelle; die
        // Argumente gehen an `step`.
        if def.params.is_empty() && !args.is_empty() {
            return self.anonymous_step(block, args, span);
        }
        let params: Vec<(String, TypeId, Option<Expr>)> =
            def.params.iter().map(|p| (p.name.clone(), p.ty, p.default.clone())).collect();
        let args = self.args(&params, args, span)?;
        let ty = self.block_type(block);
        Some(Expr::new(ExprKind::BlockInit { block, args, count: None }, ty, span))
    }

    /// Anonyme Instanz (5.7): versteckte Maschinenvariable plus
    /// `tmp = inst.step(args)` vor der umgebenden Anweisung. Ausdruecke
    /// bleiben seiteneffektfrei (4.4); der Aufruf steht als Anweisung.
    fn anonymous_step(&mut self, block: BlockId, args: &[ast::Arg], span: Span) -> Option<Expr> {
        if self.mctx.is_none() || !self.in_stmt {
            self.error_hint(
                SC3,
                span,
                "anonyme Blockinstanz nur in einer Anweisung (5.7)",
                "fuer Guards eine benannte Instanz anlegen und ihr `step` als Anweisung rufen",
            );
            return None;
        }
        if self.for_depth > 0 {
            self.error(SC3, span, "`step` hoechstens einmal je Tick: kein Aufruf in einer `for`-Schleife (5.7)");
            return None;
        }
        let def = self.program.blocks[block.index()].clone();
        let Some(step) = def.step else {
            self.error(SC3, span, format!("Block `{}` hat kein `step`", def.name));
            return None;
        };
        let f = &self.program.fns[step.index()];
        let tys: Vec<TypeId> = f.params.iter().map(|p| p.ty).collect();
        let Some(ret) = f.ret else {
            self.error(SC3, span, format!("`{}` liefert keinen Wert", def.name));
            return None;
        };
        let arg_exprs = self.method_args(args, &tys, span)?;
        let n = self.anon;
        self.anon += 1;
        let inst_ty = self.block_type(block);
        let init = Expr::new(ExprKind::BlockInit { block, args: Vec::new(), count: None }, inst_ty, span);
        let inst = self.new_var(VarDef {
            name: format!("_{}{n}", def.name),
            ty: inst_ty,
            init: Some(init),
            scope: VarScope::Machine,
            public: false,
            span,
        });
        self.mctx.as_mut()?.machine.layout.block_instances.push(BlockInstance { var: inst, block, count: 1 });
        let out = self.new_var(VarDef {
            name: format!("_{}{n}_out", def.name),
            ty: ret,
            init: Some(Expr::new(ExprKind::Default, ret, span)),
            scope: VarScope::Machine,
            public: false,
            span,
        });
        let call = StmtKind::MethodCall {
            target: Some(Place::Var(out)),
            receiver: Place::Var(inst),
            method: Method::Step,
            args: arg_exprs,
        };
        self.pending.push(Stmt { kind: call, span });
        Some(Expr::new(ExprKind::Var(out), ret, span))
    }

    /// Typ einer Blockinstanz: ein Handle je Block (Bloecke sind keine
    /// Werte, 5.7); der Codegen liest die Identitaet zusaetzlich aus
    /// `Layout::block_instances`.
    pub fn block_type(&mut self, block: BlockId) -> TypeId {
        self.intern(Type::Handle(HandleKind::Block(block)))
    }

    /// Nur was `libtaktm` korrekt gerundet rechnet, darf ins Programm (13.8).
    ///
    /// Die Referenz verlangt in 4.2 bitidentische Ergebnisse auf jedem
    /// Target. Die Standardbibliothek der Plattform kann das nicht zusagen,
    /// und eine Zahl, die nur auf *dieser* Maschine stimmt, ist schlechter
    /// als eine Meldung: Sie sieht aus wie eine Zusage. 13.8 zieht die Linie
    /// ausdruecklich — eine Funktion kommt in die kuratierte Menge,
    /// *nachdem* ihre Bit-Gleichheit belegt ist.
    ///
    /// Die Meldung traegt darum keine Stufe (das Konstrukt ist v1), sondern
    /// nennt den Grund.
    fn curated(&mut self, op: Intrinsic, span: Span) -> Option<()> {
        let Some(f) = libtaktm_fun(op) else { return Some(()) };
        if libtaktm::curated(f) == libtaktm::Curated::Yes {
            return Some(());
        }
        self.error_hint(
            SC3,
            span,
            format!("`{}` ist noch nicht korrekt gerundet implementiert (4.2)", op.name()),
            "bis dahin nicht benutzbar: eine Zahl aus der Plattformbibliothek waere auf einem anderen Target eine andere (Satz 9.4.4)",
        );
        None
    }

    fn intrinsic(&mut self, op: Intrinsic, args: &[ast::Arg], hint: Option<TypeId>, span: Span) -> Option<Expr> {
        self.curated(op, span)?;
        let want = match op {
            Intrinsic::Atan2
            | Intrinsic::Pow
            | Intrinsic::Min
            | Intrinsic::Max
            | Intrinsic::Rotl
            | Intrinsic::Rotr
            | Intrinsic::Interp => 2,
            Intrinsic::WrappingAdd
            | Intrinsic::WrappingSub
            | Intrinsic::WrappingMul
            | Intrinsic::SaturatingAdd
            | Intrinsic::SaturatingSub => 2,
            Intrinsic::Fma => 3,
            _ => 1,
        };
        if args.len() != want || args.iter().any(|a| a.name.is_some()) {
            self.error(SC3, span, format!("`{}` verlangt {want} positionale Argumente", op.name()));
            return None;
        }
        let first = self.expr(&args[0].value, hint)?;
        let ty0 = first.ty;
        let mut out = vec![first];
        let float_dimless = |this: &mut Self, t: TypeId| -> Option<TypeId> {
            if !this.is_float(t) {
                let n = this.type_name(t);
                this.error(SC3, span, format!("`{}` verlangt Fliesskomma, gefunden `{n}`", op.name()));
                return None;
            }
            Some(this.without_unit(t))
        };
        let ty = match op {
            Intrinsic::Abs => {
                if !(self.is_numeric(ty0) || self.is_duration(ty0)) {
                    let n = self.type_name(ty0);
                    self.error(SC3, span, format!("`abs` auf `{n}`"));
                    return None;
                }
                self.base(ty0)
            }
            Intrinsic::Min | Intrinsic::Max => {
                let b = self.check(&args[1].value, ty0)?;
                out.push(b);
                if !(self.is_numeric(ty0) || self.is_duration(ty0)) {
                    let n = self.type_name(ty0);
                    self.error(SC3, span, format!("`{}` auf `{n}`", op.name()));
                    return None;
                }
                // Ein Betrag setzt einen Nullpunkt voraus; ein affiner Wert
                // ist ein Punkt, kein Vektor (3.2).
                if self.affine_type(ty0) {
                    self.error_hint(
                        SC3,
                        span,
                        format!("`{}` auf einer affinen Einheit (3.2)", op.name()),
                        "Differenzen in `K` rechnen",
                    );
                    return None;
                }
                self.base(ty0)
            }
            Intrinsic::Sqrt => {
                if !self.is_float(ty0) {
                    let n = self.type_name(ty0);
                    self.error(SC3, span, format!("`sqrt` verlangt Fliesskomma, gefunden `{n}`"));
                    return None;
                }
                let unit = self.unit_of_type(ty0).expect("Fliesskomma");
                if unit.factors.iter().any(|(_, e)| e % 2 != 0) {
                    let name = self.units.display(&self.program, &unit);
                    self.error(SC3, span, format!("`sqrt` von `{name}`: Exponenten nicht durch 2 teilbar"));
                    return None;
                }
                let half = Unit { factors: unit.factors.iter().map(|(a, e)| (*a, e / 2)).collect(), overflow: false };
                let Type::Float { width, .. } = self.ty(ty0).clone() else { unreachable!() };
                self.float_type(width, &half, None, span)?
            }
            Intrinsic::Sin
            | Intrinsic::Cos
            | Intrinsic::Tan
            | Intrinsic::Asin
            | Intrinsic::Acos
            | Intrinsic::Atan
            | Intrinsic::Exp
            | Intrinsic::Log => {
                let t = float_dimless(self, ty0)?;
                if t != self.base(ty0) {
                    let n = self.type_name(ty0);
                    self.error(
                        SC3,
                        span,
                        format!("`{}` verlangt einen einheitenlosen Wert, gefunden `{n}`", op.name()),
                    );
                    return None;
                }
                t
            }
            Intrinsic::Atan2 => {
                let b = self.check(&args[1].value, ty0)?;
                out.push(b);
                if !self.is_float(ty0) {
                    self.error(SC3, span, "`atan2` verlangt Fliesskomma");
                    return None;
                }
                self.without_unit(ty0)
            }
            Intrinsic::Pow => {
                // Wie `sin` und `exp`: einheitenlos verlangt, nicht still
                // verworfen (3.2). Ein Exponent ist zur Uebersetzungszeit
                // nicht bekannt, also gibt es keinen Einheitenausgang.
                let t = float_dimless(self, ty0)?;
                if t != self.base(ty0) {
                    let n = self.type_name(ty0);
                    self.error_hint(
                        SC3,
                        span,
                        format!("`pow` verlangt einen einheitenlosen Wert, gefunden `{n}`"),
                        "Einheit vorher herausrechnen, etwa `pow(x / (1 bar), 2.0)`",
                    );
                    return None;
                }
                let b = self.check(&args[1].value, t)?;
                out.push(b);
                t
            }
            Intrinsic::Fma => {
                let b = self.expr(&args[1].value, None)?;
                if !(self.is_float(ty0) && self.is_float(b.ty)) {
                    self.error(SC3, span, "`fma` verlangt Fliesskomma");
                    return None;
                }
                let (ua, ub) = (self.unit_of_type(ty0).expect("float"), self.unit_of_type(b.ty).expect("float"));
                let Type::Float { width, .. } = self.ty(ty0).clone() else { unreachable!() };
                let product = ua.mul(&ub);
                let t = self.float_type(width, &product, None, span)?;
                let c = self.check(&args[2].value, t)?;
                out.push(b);
                out.push(c);
                t
            }
            Intrinsic::Round | Intrinsic::Floor | Intrinsic::Ceil => {
                // Das Ergebnis ist eine Ganzzahl ohne Einheit; sie einfach zu
                // verwerfen waere eine implizite Konversion (3.2). Die
                // Referenz schreibt das Muster in 14.7:
                // `round(soc / (100 pct) * 255)`.
                let t = float_dimless(self, ty0)?;
                if t != self.base(ty0) {
                    let n = self.type_name(ty0);
                    self.error_hint(
                        SC3,
                        span,
                        format!("`{}` verlangt einen einheitenlosen Wert, gefunden `{n}`", op.name()),
                        "Einheit vorher herausrechnen, etwa `round(x / (1 bar))`",
                    );
                    return None;
                }
                self.tys.int
            }
            Intrinsic::Rotl | Intrinsic::Rotr => {
                if !self.is_int(ty0) {
                    self.error(SC3, span, format!("`{}` verlangt Ganzzahl", op.name()));
                    return None;
                }
                let int = self.tys.int;
                let b = self.check(&args[1].value, int)?;
                out.push(b);
                self.base(ty0)
            }
            Intrinsic::WrappingAdd
            | Intrinsic::WrappingSub
            | Intrinsic::WrappingMul
            | Intrinsic::SaturatingAdd
            | Intrinsic::SaturatingSub => {
                if !self.is_int(ty0) {
                    self.error(SC3, span, format!("`{}` verlangt Ganzzahl", op.name()));
                    return None;
                }
                let b = self.check(&args[1].value, ty0)?;
                out.push(b);
                self.base(ty0)
            }
            Intrinsic::Interp => {
                let Type::Table { key, value } = self.ty(ty0).clone() else {
                    let n = self.type_name(ty0);
                    self.error(SC3, span, format!("`interp` verlangt eine Tabelle, gefunden `{n}`"));
                    return None;
                };
                let x = self.check(&args[1].value, key)?;
                out.push(x);
                value
            }
        };
        Some(Expr::new(ExprKind::Intrinsic { op, args: out }, ty, span))
    }

    // ------------------------------------------------------------ Zugriffe

    fn member(
        &mut self,
        base: &ast::Expr,
        name: &ast::Ident,
        args: Option<&[ast::Arg]>,
        hint: Option<TypeId>,
        span: Span,
    ) -> Option<Expr> {
        let member = name.name.as_str();
        // Maschine: .state, pub var, Signal
        if let ast::ExprKind::Ident(id) = &base.kind {
            if let Some(Entity::Machine(m)) = self.peek(&id.name).cloned() {
                return self.machine_member(m, name, args, span);
            }
            if let Some(Entity::Var(v, t)) = self.peek(&id.name).cloned() {
                if matches!(self.ty(t), Type::Handle(HandleKind::Job)) {
                    return self.job_member(v, id, name, args, span);
                }
            }
            if let Some(Entity::Channel(c)) = self.peek(&id.name).cloned() {
                if self.program.channels[c.index()].dir == takt_mir::program::Direction::Input && is_wrapper(member) {
                    let raw = self.input_read(c, base.span, true);
                    return self.wrapper_access(raw, name, args, span);
                }
            }
            // 7.5: `t.armed` ist ein `bool`, `t.fired` der Eingangsstrom.
            if let Some(Entity::Trigger(t)) = self.peek(&id.name).cloned() {
                return self.trigger_member(t, name, span);
            }
        }
        if let ast::ExprKind::TypeName { name: tn, args: None } = &base.kind {
            if member == "decode" {
                return self.decode(tn, args, span);
            }
        }
        // Elementzugriff auf ein Channel-Array: Wrapper je Element
        if let ast::ExprKind::Index { base: inner, index } = &base.kind {
            if let ast::ExprKind::Ident(id) = &inner.kind {
                if let Some(Entity::Channel(c)) = self.peek(&id.name).cloned() {
                    if is_wrapper(member) && self.program.channels[c.index()].dir == takt_mir::program::Direction::Input
                    {
                        let raw = self.input_read(c, inner.span, true);
                        let elem = self.index_of(raw, index, base.span, true)?;
                        return self.wrapper_access(elem, name, args, span);
                    }
                }
            }
        }
        let b = self.expr(base, None)?;
        if is_wrapper(member) && matches!(self.ty(b.ty), Type::Optional(_) | Type::Result { .. }) {
            return self.wrapper_access(b, name, args, span);
        }
        let bty = self.ty(b.ty).clone();
        let has_args = args.is_some_and(|a| !a.is_empty());
        let no_args = |this: &mut Self| -> bool {
            if has_args {
                this.error(SC3, span, format!("`{member}` nimmt keine Argumente"));
                false
            } else {
                true
            }
        };
        match (member, &bty) {
            ("as", Type::Duration { .. }) => {
                let unit = self.unit_arg(args, span)?;
                let dim = self.units.dimension(&self.program, &unit);
                if dim != [0, 0, 1, 0, 0, 0, 0] {
                    self.error(SC3, span, "`.as(U)` verlangt eine Zeiteinheit (3.3)");
                    return None;
                }
                let uid = self.unit_id(&unit, span)?;
                let width = self.float_width();
                let ty = self.intern(Type::Float { width, unit: Some(uid), range: None });
                Some(Expr::new(ExprKind::Convert { expr: Box::new(b), kind: ConvertKind::As, unit: uid }, ty, span))
            }
            ("to", Type::Float { width, .. }) => {
                let unit = self.unit_arg(args, span)?;
                let src = self.unit_of_type(b.ty).expect("float");
                self.check_dimension(&src, &unit, span)?;
                let uid = self.unit_id(&unit, span)?;
                let width = *width;
                let ty = self.intern(Type::Float { width, unit: Some(uid), range: None });
                Some(Expr::new(ExprKind::Convert { expr: Box::new(b), kind: ConvertKind::To, unit: uid }, ty, span))
            }
            // 3.2: Auf Ganzzahlen ist `.to(U)` eine Multiplikation mit dem
            // ganzzahligen Faktor (Pruefung 38); Overflow ist ein Fault wie 4.1.
            ("to", Type::Int { width, .. }) => {
                let unit = self.unit_arg(args, span)?;
                let src = self.unit_of_type(b.ty).expect("int");
                self.check_dimension(&src, &unit, span)?;
                let c = self.units.display(&self.program, &unit);
                if self.units.is_affine(&self.program, &src) || self.units.is_affine(&self.program, &unit) {
                    self.error_hint(
                        SC38,
                        span,
                        format!("`.to({c})`: affine Einheiten haben keinen ganzzahligen Versatz"),
                        format!("`.to_float({c})` (3.2)"),
                    );
                    return None;
                }
                let Some(k) = self.integral_factor(&src, &unit) else {
                    let a = self.units.display(&self.program, &src);
                    self.error_hint(
                        SC38,
                        span,
                        format!("`.to({c})`: der Faktor von `{a}` nach `{c}` ist nicht ganzzahlig"),
                        format!("`.to_float({c})` liefert das Ergebnis als Fliesskomma (3.2)"),
                    );
                    return None;
                };
                let width = *width;
                let ty = self.int_type(width, &unit, None, span)?;
                if k == 1 {
                    return Some(Expr::new(ExprKind::Cast { expr: Box::new(b), to: ty }, ty, span));
                }
                if k > takt_interp::arith::bounds(width).1 {
                    self.error(SC38, span, format!("Faktor {k} passt nicht in `{}`", takt_interp::arith::name(width)));
                    return None;
                }
                let scalar = self.without_unit(b.ty);
                let factor = Expr::new(ExprKind::Int(k as i64), scalar, span);
                Some(Expr::new(
                    ExprKind::Binary { op: BinaryOp::Mul, lhs: Box::new(b), rhs: Box::new(factor) },
                    ty,
                    span,
                ))
            }
            ("to_float", Type::Int { .. }) => {
                let unit = self.unit_arg(args, span)?;
                let src = self.unit_of_type(b.ty).expect("int");
                self.check_dimension(&src, &unit, span)?;
                let uid = self.unit_id(&unit, span)?;
                let width = self.float_width();
                let ty = self.intern(Type::Float { width, unit: Some(uid), range: None });
                Some(Expr::new(
                    ExprKind::Convert { expr: Box::new(b), kind: ConvertKind::ToFloat, unit: uid },
                    ty,
                    span,
                ))
            }
            ("bit", Type::Int { .. }) => {
                let int = self.tys.int;
                let i = self.one_arg(args, int, span)?;
                let ty = self.tys.bool;
                Some(Expr::new(
                    ExprKind::Accessor { base: Box::new(b), accessor: Accessor::Bit, args: vec![i] },
                    ty,
                    span,
                ))
            }
            ("bits", Type::Int { .. }) => {
                let int = self.tys.int;
                let (hi, lo) = self.two_args(args, int, int, span)?;
                let ty = self.tys.int;
                Some(Expr::new(
                    ExprKind::Accessor { base: Box::new(b), accessor: Accessor::Bits, args: vec![hi, lo] },
                    ty,
                    span,
                ))
            }
            ("with_bit", Type::Int { .. }) => {
                let (int, bool) = (self.tys.int, self.tys.bool);
                let (i, v) = self.two_args(args, int, bool, span)?;
                let ty = self.base(b.ty);
                Some(Expr::new(
                    ExprKind::Accessor { base: Box::new(b), accessor: Accessor::WithBit, args: vec![i, v] },
                    ty,
                    span,
                ))
            }
            (m, Type::Int { .. }) if m.starts_with("wrap_") => {
                let width = match &m[5..] {
                    "i8" => IntWidth::I8,
                    "i16" => IntWidth::I16,
                    "i32" => IntWidth::I32,
                    "i64" | "int" => IntWidth::I64,
                    "u8" => IntWidth::U8,
                    "u16" => IntWidth::U16,
                    "u32" => IntWidth::U32,
                    "u64" => IntWidth::U64,
                    other => {
                        self.error(SC3, span, format!("unbekannte Breite `{other}` in `{m}`"));
                        return None;
                    }
                };
                if !no_args(self) {
                    return None;
                }
                let ty = self.intern(Type::Int { width, unit: None, range: None });
                Some(Expr::new(
                    ExprKind::Accessor { base: Box::new(b), accessor: Accessor::Wrap(width), args: vec![] },
                    ty,
                    span,
                ))
            }
            (
                "len",
                Type::Bytes { .. } | Type::Vec { .. } | Type::Str { .. } | Type::Line { .. } | Type::Map { .. },
            ) => {
                if !no_args(self) {
                    return None;
                }
                // Die Laenge liegt immer in `0..Kapazitaet` (3.9). Ohne diese
                // Schranke fuegt jede Zuweisung an eine range-typisierte Stelle
                // eine implizite Pruefung ein, die nie scheitern kann.
                let cap = match self.ty(b.ty) {
                    Type::Bytes { cap }
                    | Type::Vec { cap, .. }
                    | Type::Map { cap, .. }
                    | Type::Str { cap }
                    | Type::Line { cap } => Some(i64::from(*cap)),
                    _ => None,
                };
                let ty = match cap {
                    Some(cap) => {
                        let range = takt_mir::types::Range {
                            lo: takt_mir::types::Const::Int(0),
                            hi: takt_mir::types::Const::Int(cap),
                            origin: takt_mir::types::RangeOrigin::Declared,
                        };
                        self.intern(Type::Int { width: IntWidth::I64, unit: None, range: Some(range) })
                    }
                    None => self.tys.int,
                };
                Some(Expr::new(
                    ExprKind::Accessor { base: Box::new(b), accessor: Accessor::Len, args: vec![] },
                    ty,
                    span,
                ))
            }
            ("count", Type::Array { .. } | Type::Samples { .. }) => {
                if !no_args(self) {
                    return None;
                }
                let ty = self.tys.int;
                Some(Expr::new(
                    ExprKind::Accessor { base: Box::new(b), accessor: Accessor::Count, args: vec![] },
                    ty,
                    span,
                ))
            }
            ("last", Type::Array { elem, .. } | Type::Samples { elem, .. }) => {
                if !no_args(self) {
                    return None;
                }
                let ty = *elem;
                Some(Expr::new(
                    ExprKind::Accessor { base: Box::new(b), accessor: Accessor::Last, args: vec![] },
                    ty,
                    span,
                ))
            }
            ("min" | "max" | "mean" | "rms", Type::Array { elem, .. } | Type::Samples { elem, .. }) => {
                if !no_args(self) {
                    return None;
                }
                let elem = *elem;
                let (acc, ty) = match member {
                    "min" => (Accessor::Min, self.base(elem)),
                    "max" => (Accessor::Max, self.base(elem)),
                    "mean" => (Accessor::Mean, self.base(elem)),
                    _ => (Accessor::Rms, self.base(elem)),
                };
                if member != "min" && member != "max" && !self.is_float(elem) {
                    self.error(SC3, span, format!("`{member}` verlangt Fliesskomma-Elemente"));
                    return None;
                }
                Some(Expr::new(ExprKind::Accessor { base: Box::new(b), accessor: acc, args: vec![] }, ty, span))
            }
            ("get", Type::Map { key, value, .. }) => {
                let (key, value) = (*key, *value);
                let k = self.one_arg(args, key, span)?;
                let ty = self.intern(Type::Optional(value));
                Some(Expr::new(
                    ExprKind::Accessor { base: Box::new(b), accessor: Accessor::Get, args: vec![k] },
                    ty,
                    span,
                ))
            }
            ("get", Type::Vec { elem, .. } | Type::Array { elem, .. }) => {
                let int = self.tys.int;
                let i = self.one_arg(args, int, span)?;
                let elem = *elem;
                let ty = self.intern(Type::Optional(elem));
                Some(Expr::new(
                    ExprKind::Accessor { base: Box::new(b), accessor: Accessor::Get, args: vec![i] },
                    ty,
                    span,
                ))
            }
            ("starts_with" | "contains", Type::Str { .. } | Type::Line { .. }) => {
                let Some([arg]) = args else {
                    self.error(SC3, span, format!("`{member}` verlangt ein Literal"));
                    return None;
                };
                let lit = self.expr(&arg.value, None)?;
                if !matches!(self.ty(lit.ty), Type::Str { .. }) {
                    self.error(SC3, span, format!("`{member}` verlangt ein Stringliteral"));
                    return None;
                }
                let acc = if member == "starts_with" { Accessor::StartsWith } else { Accessor::Contains };
                let ty = self.tys.bool;
                Some(Expr::new(ExprKind::Accessor { base: Box::new(b), accessor: acc, args: vec![lit] }, ty, span))
            }
            ("truncated", Type::Line { .. }) => {
                let ty = self.tys.bool;
                Some(Expr::new(
                    ExprKind::Accessor { base: Box::new(b), accessor: Accessor::Truncated, args: vec![] },
                    ty,
                    span,
                ))
            }
            ("encode", Type::Record(r)) => {
                if !no_args(self) {
                    return None;
                }
                // `f.encode() -> bytes<SIZE>` (3.7); ohne `layout` gibt es
                // keine Byte-Repraesentation.
                let Some(size) = self.program.records[r.index()].wire_size else {
                    let name = self.program.records[r.index()].name.clone();
                    self.error_hint(
                        SC3,
                        span,
                        format!("`{name}` hat kein `layout`"),
                        "`layout little` oder `layout big` am Record ergaenzen (3.7)",
                    );
                    return None;
                };
                let ty = self.intern(Type::Bytes { cap: size });
                Some(Expr::new(
                    ExprKind::Accessor { base: Box::new(b), accessor: Accessor::Encode, args: Vec::new() },
                    ty,
                    span,
                ))
            }
            // 3.8: unter `.valid` oder `.ok` ist der Wert dominiert und sein
            // Inhalt direkt erreichbar. Der Feldzugriff packt ihn aus; der
            // implizite Check bleibt, die Warnung entfaellt bei Dominanz.
            (field, Type::Optional(inner) | Type::Result { ok: inner, .. })
                if record_field(self, *inner, field).is_some() =>
            {
                let inner = *inner;
                let (index, ty) = record_field(self, inner, field)?;
                let unwrapped = self.coerce(b, inner)?;
                Some(Expr::new(ExprKind::Field { base: Box::new(unwrapped), field: index }, ty, span))
            }
            // 3.7: ein benanntes Bitfeld ist eine Sicht auf sein Traegerfeld,
            // kein eigener Speicher. Der Zugriff wird zu `bit`/`bits` auf dem
            // Traeger; die Positionen stehen in der Deklaration.
            (field, Type::Int { .. }) if bitfield_of(self, &b, field).is_some() => {
                if !no_args(self) {
                    return None;
                }
                let bf = bitfield_of(self, &b, field).expect("gerade geprueft");
                let (lo, hi, ty) = (bf.lo, bf.hi, bf.ty);
                // 3.7: `wo` liefert nicht den geschriebenen Wert, `rsvd`
                // hat keinen.
                if !bf.access.readable() {
                    self.error_hint(
                        crate::checks::SC46,
                        span,
                        format!("`{field}` ist `{}` und nicht lesbar (3.7)", bf.access.name()),
                        "ein `wo`-Feld liefert beim Lesen nicht, was geschrieben wurde",
                    );
                    return None;
                }
                let int = self.tys.int;
                let pos = |v: u8| Expr::new(ExprKind::Int(i64::from(v)), int, span);
                // `bit` liefert `bool`, `bits` einen `int`; das Bitfeld traegt
                // seinen deklarierten Typ.
                let is_bool = matches!(self.ty(ty), Type::Bool);
                let (accessor, args, raw) = if is_bool {
                    (Accessor::Bit, vec![pos(lo)], self.tys.bool)
                } else {
                    (Accessor::Bits, vec![pos(hi), pos(lo)], int)
                };
                let value = Expr::new(ExprKind::Accessor { base: Box::new(b), accessor, args }, raw, span);
                // 3.7: `active_low` kehrt den Wert an der Grenze um; der
                // Traeger bleibt roh.
                let value = match (bf.active_low, is_bool) {
                    (true, true) => Expr::new(ExprKind::Unary { op: UnaryOp::Not, expr: Box::new(value) }, raw, span),
                    (true, false) => {
                        Expr::new(ExprKind::Unary { op: UnaryOp::BitNot, expr: Box::new(value) }, raw, span)
                    }
                    (false, _) => value,
                };
                // Die Breite des Bitfelds passt in seinen Typ (Pruefung 46),
                // die Verengung ist also nachweislich verlustfrei.
                if is_bool {
                    Some(value)
                } else {
                    Some(Expr::new(ExprKind::Cast { expr: Box::new(value), to: ty }, ty, span))
                }
            }
            (field, Type::Record(r)) => {
                let found = self.program.records[r.index()].fields.iter().position(|f| f.name == field);
                match found {
                    Some(i) => {
                        if !no_args(self) {
                            return None;
                        }
                        let ty = self.program.records[r.index()].fields[i].ty;
                        Some(Expr::new(ExprKind::Field { base: Box::new(b), field: i as u32 }, ty, span))
                    }
                    None => {
                        let def = &self.program.records[r.index()];
                        let names: Vec<String> = def.fields.iter().map(|f| f.name.clone()).collect();
                        let rname = def.name.clone();
                        self.error_hint(
                            SC3,
                            name.span,
                            format!("`{rname}` hat kein Feld `{field}`"),
                            format!("Felder: {}", names.join(", ")),
                        );
                        None
                    }
                }
            }
            (m, _) if crate::lower::stmt::MUTATING.contains(&m) => {
                self.error_hint(
                    SC3,
                    span,
                    format!("`{member}` veraendert den Wert und steht nur als Anweisung"),
                    "als eigene Zeile schreiben, etwa `ok = v.push(x)`",
                );
                None
            }
            ("transpose" | "inv" | "det" | "cholesky", Type::Mat { .. }) => self.mat_member(b, member, args, span),
            ("armed" | "fired", _) => {
                self.stage(span, "Trigger", Stage::V1_2);
                None
            }
            // Zaehler und freier Platz eines Stroms (8.6, 8.8): lesen, ohne
            // zu untersuchen, also ohne den Cursor zu bewegen.
            ("count" | "dropped" | "overflowed" | "malformed", Type::Stream(_)) => {
                if !no_args(self) {
                    return None;
                }
                let acc = match member {
                    "count" => Accessor::Count,
                    "dropped" => Accessor::Dropped,
                    "overflowed" => Accessor::Overflowed,
                    _ => Accessor::Malformed,
                };
                let ty = self.tys.int;
                Some(Expr::new(ExprKind::Accessor { base: Box::new(b), accessor: acc, args: vec![] }, ty, span))
            }
            // `peek` untersucht das naechste Element, ohne es zu konsumieren
            // (8.6, FB-15): die Maschine wird Leser des Stroms.
            ("peek", Type::Stream(elem)) => {
                let elem = *elem;
                if args.is_none() {
                    self.error(SC3, span, "`peek()` ist ein Aufruf");
                    return None;
                }
                if !no_args(self) {
                    return None;
                }
                self.cursor_for(&b, span)?;
                let ty = self.intern(Type::Optional(elem));
                Some(Expr::new(
                    ExprKind::Accessor { base: Box::new(b), accessor: Accessor::Peek, args: vec![] },
                    ty,
                    span,
                ))
            }
            // `o.sent` (8.8, FB-132): was der Treiber im letzten Tick abgeholt
            // hat, hoechstens `max_rate * T0` Byte — ein Wert mit Unit-Delay,
            // kein Fenster.
            ("sent", Type::Stream(_)) => {
                if !no_args(self) {
                    return None;
                }
                // Ein Strom ist als Ausdruck immer `Input { channel }` (8.6);
                // die Richtung steht am Channel.
                let c = match b.kind {
                    ExprKind::Input { channel, .. }
                        if self.program.channels[channel.index()].dir == takt_mir::program::Direction::Output =>
                    {
                        channel
                    }
                    _ => {
                        self.error(SC3, span, "`sent` gibt es nur an einem Ausgabestrom (8.8)");
                        return None;
                    }
                };
                let cap = self.sent_per_tick(c);
                let bytes = self.intern(Type::Bytes { cap });
                let ty = self.intern(Type::Optional(bytes));
                Some(Expr::new(
                    ExprKind::Accessor { base: Box::new(b), accessor: Accessor::Sent, args: vec![] },
                    ty,
                    span,
                ))
            }
            ("free", Type::Stream(_)) => {
                if !no_args(self) {
                    return None;
                }
                let ty = self.tys.int;
                Some(Expr::new(
                    ExprKind::Accessor { base: Box::new(b), accessor: Accessor::Free, args: vec![] },
                    ty,
                    span,
                ))
            }
            // `capture<T, N>` (8.9, v1.2), gemessener Jitter aus der
            // Hardware-Konfiguration und `time_warped` (7.5, 8.10) haengen an
            // Konstrukten, die es noch nicht gibt. Die uebrigen Zugriffe
            // dieser Liste kennt M2; sie landen hier nur auf einem falschen
            // Traeger und sind dann ein Typfehler, kein Stufenproblem.
            // 8.9: `.t`, `.pre`, `.post`, `.samples`, `.rate` eines
            // Capture-Fensters.
            ("t" | "pre" | "post" | "samples" | "rate", Type::Capture { elem, len }) => {
                if !no_args(self) {
                    return None;
                }
                let (elem, len) = (*elem, *len);
                let (acc, ty) = match member {
                    "t" => (Accessor::T, self.tys.duration),
                    "pre" => (Accessor::Pre, self.tys.int),
                    "post" => (Accessor::Post, self.tys.int),
                    "rate" => (Accessor::Rate, self.hertz()),
                    _ => (Accessor::Samples, self.intern(Type::Array { elem, len })),
                };
                Some(Expr::new(ExprKind::Accessor { base: Box::new(b), accessor: acc, args: vec![] }, ty, span))
            }
            ("pre" | "post" | "samples" | "remaining" | "jitter" | "time_warped", _) => {
                self.stage(span, format!("`.{member}`").as_str(), Stage::V1_1);
                None
            }
            _ => {
                let _ = hint;
                let n = self.type_name(b.ty);
                // Traegt der Ausdruck Bitfelder, nennt der Hinweis sie (3.7).
                if let Some(names) = bitfield_names(self, &b) {
                    self.error_hint(
                        SC3,
                        name.span,
                        format!("kein Zugriff `{member}` auf `{n}`"),
                        format!("Bitfelder: {}", names.join(", ")),
                    );
                    return None;
                }
                self.error(SC3, name.span, format!("kein Zugriff `{member}` auf `{n}`"));
                None
            }
        }
    }

    /// `.valid .suspect .stale .age .reason .or .ok .err` auf Abtastung, `T?`, `T!E`.
    /// `x matches P [as m]` und `x has P` (8.7). Das Ergebnis ist ein Bool;
    /// die Bindung ist eine gehobene Variable, die der Abgleich schreibt.
    fn matches(
        &mut self,
        subject: &ast::Expr,
        kind: ast::MatchKind,
        pattern: &ast::Pattern,
        binding: Option<&ast::Ident>,
        span: Span,
    ) -> Option<Expr> {
        let subject = self.expr(subject, None)?;
        // `matches`/`has` gelten auf `str<N>`, `line<N>` und Recordwerten (8.7).
        let textual = matches!(self.ty(subject.ty), Type::Str { .. } | Type::Line { .. });
        let record = matches!(self.ty(subject.ty), Type::Record(_));
        if !(textual || record) {
            let n = self.type_name(subject.ty);
            self.error_hint(
                SC3,
                span,
                format!("`matches` verlangt Text oder einen Record, gefunden `{n}`"),
                "`matches` gilt auf `str<N>`, `line<N>` und Recordwerten (8.7)",
            );
            return None;
        }
        let lowered = self.pattern(pattern, Some(subject.ty), span)?;
        if matches!(lowered.pattern, Pattern::Text { .. }) && !textual {
            let n = self.type_name(subject.ty);
            self.error(SC18, span, format!("Textmuster auf `{n}`"));
            return None;
        }
        let var = match binding {
            Some(name) => Some(self.binding_var(name, &lowered.captures, None, span)?),
            None => None,
        };
        let ty = self.tys.bool;
        let kind = match kind {
            ast::MatchKind::Matches => MatchKind::Matches,
            ast::MatchKind::Has => MatchKind::Has,
        };
        Some(Expr::new(
            ExprKind::Matches { subject: Box::new(subject), kind, pattern: lowered.pattern, binding: var },
            ty,
            span,
        ))
    }

    /// Pruefung 47 (3.9): ein Argument darf pro Aufruf nur einmal als `inout`
    /// gebunden werden und nicht zugleich als weiteres Argument erscheinen.
    /// Sonst schriebe die Zeigeruebergabe in eine Stelle, die der Aufruf
    /// zugleich liest, und die Funktion waere nicht mehr rein.
    fn check_no_aliasing(&mut self, inout: &[bool], args: &[Expr]) {
        let keys: Vec<Option<String>> = args.iter().map(Self::dom_key).collect();
        for (i, marked) in inout.iter().enumerate() {
            if !marked {
                continue;
            }
            let Some(key) = keys.get(i).and_then(Option::as_deref) else { continue };
            for (j, other) in keys.iter().enumerate() {
                if i == j {
                    continue;
                }
                if other.as_deref() == Some(key) {
                    // `dom_key` ist ein interner Schluessel; die Meldung nennt
                    // die Stelle, nicht den Schluessel.
                    self.error_hint(
                        SC47,
                        args[i].span,
                        format!("dasselbe Argument steht als `inout` und als Argument {}", j + 1),
                        "jedes Argument nur einmal uebergeben; `inout` schreibt in die Stelle (3.9)",
                    );
                    return;
                }
            }
        }
    }

    /// `R.decode(b) -> R?` (3.7): der Record braucht ein `layout`, das
    /// Argument ist `bytes<N>`.
    fn decode(&mut self, name: &ast::Ident, args: Option<&[ast::Arg]>, span: Span) -> Option<Expr> {
        let record = match self.lookup(name)? {
            Entity::Record(r) => r,
            _ => {
                self.error(SC3, name.span, format!("`{}` ist kein Record", name.name));
                return None;
            }
        };
        if self.program.records[record.index()].wire_size.is_none() {
            self.error_hint(
                SC3,
                span,
                format!("`{}` hat kein `layout`", name.name),
                "`layout little` oder `layout big` am Record ergaenzen (3.7)",
            );
            return None;
        }
        let Some([arg]) = args else {
            self.error(SC3, span, "`decode` verlangt genau ein Argument");
            return None;
        };
        let bytes = self.expr(&arg.value, None)?;
        if !matches!(self.ty(bytes.ty), Type::Bytes { .. }) {
            let n = self.type_name(bytes.ty);
            self.error(SC3, span, format!("`decode` verlangt `bytes<N>`, gefunden `{n}`"));
            return None;
        }
        let record_ty = self.intern(Type::Record(record));
        let ty = self.intern(Type::Optional(record_ty));
        Some(Expr::new(ExprKind::Decode { record, bytes: Box::new(bytes) }, ty, span))
    }

    fn wrapper_access(&mut self, base: Expr, name: &ast::Ident, args: Option<&[ast::Arg]>, span: Span) -> Option<Expr> {
        let member = name.name.as_str();
        // Qualitaet und Alter gibt es nur an einem Input-Channel (3.5); ein
        // `T?` kennt allein `.valid` und `.or(d)` (3.8). Ein Index zaehlt nur
        // dann, wenn seine Basis ein Channel-Array ist, sonst galten die
        // Zugriffe auch fuer ein gewoehnliches Array von Optionalwerten.
        let is_input = is_channel_read(&base);
        let inner_ty = match self.ty(base.ty).clone() {
            Type::Optional(t) => t,
            Type::Result { ok, .. } => ok,
            _ if is_input => base.ty,
            _ => {
                let n = self.type_name(base.ty);
                self.error(SC3, span, format!("`.{member}` nur auf Channels, `T?` und `T!E`, gefunden `{n}`"));
                return None;
            }
        };
        let is_result = matches!(self.ty(base.ty), Type::Result { .. });
        let bool = self.tys.bool;
        let mk = |this: &mut Self, acc: Accessor, args: Vec<Expr>, ty: TypeId| {
            let _ = this;
            Some(Expr::new(ExprKind::Accessor { base: Box::new(base.clone()), accessor: acc, args }, ty, span))
        };
        match member {
            "valid" if !is_result => mk(self, Accessor::Valid, vec![], bool),
            "ok" if is_result => mk(self, Accessor::Ok, vec![], bool),
            "err" if is_result => {
                let Type::Result { err, .. } = self.ty(base.ty).clone() else { unreachable!() };
                let e = self.intern(Type::Enum(err));
                let ty = self.intern(Type::Optional(e));
                mk(self, Accessor::Err, vec![], ty)
            }
            "or" => {
                let d = self.one_arg(args, inner_ty, span)?;
                mk(self, Accessor::Or, vec![d], inner_ty)
            }
            "suspect" | "stale" if is_input => {
                mk(self, if member == "suspect" { Accessor::Suspect } else { Accessor::Stale }, vec![], bool)
            }
            "age" if is_input => {
                let ty = self.tys.duration;
                mk(self, Accessor::Age, vec![], ty)
            }
            "reason" if is_input => {
                let reason = match self.peek("Reason").cloned() {
                    Some(Entity::Enum(e)) => self.intern(Type::Enum(e)),
                    _ => {
                        self.error(SC3, span, "Enum `Reason` fehlt im Prelude");
                        return None;
                    }
                };
                let ty = self.intern(Type::Optional(reason));
                mk(self, Accessor::Reason, vec![], ty)
            }
            _ => {
                let n = self.type_name(base.ty);
                self.error(SC3, span, format!("kein Zugriff `{member}` auf `{n}`"));
                None
            }
        }
    }

    /// Wie viele Byte der Treiber je Tick abholt (8.8): `max_rate * T0`,
    /// mindestens eines; ohne `max_rate` die ganze Kapazitaet. Dieselbe
    /// Rechnung wie `TxBuffer::per_tick` im Interpreter.
    fn sent_per_tick(&self, c: ChannelId) -> u32 {
        let ch = &self.program.channels[c.index()];
        let hz = match ch.attrs.max_rate.as_ref().map(|e| &e.kind) {
            Some(ExprKind::Int(n)) => u64::try_from(*n).ok(),
            Some(ExprKind::Float(f)) if *f >= 0.0 => Some(*f as u64),
            _ => None,
        };
        match hz {
            Some(hz) => {
                let bytes = hz.saturating_mul(self.program.config.tick as u64) / 1_000_000_000;
                u32::try_from(bytes.max(1)).unwrap_or(u32::MAX)
            }
            None => ch.attrs.capacity.unwrap_or(256),
        }
    }

    /// `v.done`, `v.result` eines Job-Handles (4.5): Inputs, Teil von I_k.
    fn job_member(
        &mut self,
        handle: VarId,
        base: &ast::Ident,
        name: &ast::Ident,
        args: Option<&[ast::Arg]>,
        span: Span,
    ) -> Option<Expr> {
        if args.is_some() {
            self.error(SC3, span, format!("`{}` nimmt keine Argumente", name.name));
            return None;
        }
        let slot =
            self.mctx.as_ref().and_then(|m| m.machine.layout.job_slots.iter().find(|s| s.handle == handle).cloned());
        let Some(slot) = slot else {
            self.error(SC44, span, format!("`{}` ist noch keinem `job` zugewiesen (4.5)", base.name));
            return None;
        };
        match name.name.as_str() {
            "done" => {
                let ty = self.tys.bool;
                Some(Expr::new(ExprKind::JobState { handle, field: JobField::Done }, ty, span))
            }
            "result" => {
                let ok = self.program.natives[slot.native.index()].ret;
                let Some(Entity::Enum(err)) = self.peek("JobErr").cloned() else {
                    self.error(SC3, span, "`JobErr` fehlt im Prelude");
                    return None;
                };
                let ty = self.intern(Type::Result { ok, err });
                Some(Expr::new(ExprKind::JobState { handle, field: JobField::Result }, ty, span))
            }
            other => {
                self.error_hint(
                    SC3,
                    name.span,
                    format!("kein Zugriff `{other}` auf ein Job-Handle"),
                    "`done` oder `result` (4.5)",
                );
                None
            }
        }
    }

    /// `t.armed` und `t.fired` (7.5, v1.2).
    ///
    /// `armed` ist ein `bool` im Layout der armierenden Maschine, `fired`
    /// der interne Strom des Triggers — beides lesend, beides ohne
    /// Argumente.
    fn trigger_member(&mut self, t: TriggerId, name: &ast::Ident, span: Span) -> Option<Expr> {
        match name.name.as_str() {
            "armed" => {
                let bool = self.tys.bool;
                Some(Expr::new(ExprKind::Armed(t), bool, span))
            }
            "fired" => {
                let s = self.program.triggers[t.index()].fired;
                let elem = self.program.streams[s.index()].elem;
                let ty = self.intern(Type::Stream(elem));
                Some(Expr::new(ExprKind::Stream(s), ty, span))
            }
            other => {
                self.error_hint(
                    SC3,
                    name.span,
                    format!("ein Trigger hat kein `{other}`"),
                    "`armed` ist sein Zustand, `fired` sein Eingangsstrom (7.5)",
                );
                None
            }
        }
    }

    fn machine_member(
        &mut self,
        m: MachineId,
        name: &ast::Ident,
        args: Option<&[ast::Arg]>,
        span: Span,
    ) -> Option<Expr> {
        if args.is_some() {
            self.error(SC3, span, format!("`{}` nimmt keine Argumente", name.name));
            return None;
        }
        let mref = MachineRef { machine: m, index: None };
        if name.name == "state" {
            let e = self.state_enums.get(&m).copied().or_else(|| {
                self.error(SC3, span, "Zustandstyp der Maschine fehlt");
                None
            })?;
            let ty = self.intern(Type::Enum(e));
            return Some(Expr::new(ExprKind::StateOf(mref), ty, span));
        }
        let machine = &self.program.machines[m.index()];
        if let Some((i, v)) = machine.vars.iter().enumerate().find(|(_, v)| v.name == name.name) {
            if !v.public {
                self.error_hint(
                    SC3,
                    span,
                    format!("`{}` ist keine `pub var` von `{}`", name.name, machine.name),
                    "`pub var` deklarieren (5.8)",
                );
                return None;
            }
            let ty = v.ty;
            return Some(Expr::new(ExprKind::Published { machine: mref, var: VarId(i as u32) }, ty, span));
        }
        if let Some(i) = machine.signals.iter().position(|s| s.name == name.name) {
            let ty = self.tys.bool;
            return Some(Expr::new(ExprKind::Signal { machine: mref, signal: SignalId(i as u32) }, ty, span));
        }
        let mname = machine.name.clone();
        self.error(SC3, name.span, format!("`{mname}` hat weder `pub var` noch Signal `{}`", name.name));
        None
    }

    fn unit_arg(&mut self, args: Option<&[ast::Arg]>, span: Span) -> Option<Unit> {
        let Some([arg]) = args else {
            self.error(SC3, span, "genau eine Einheit als Argument erwartet");
            return None;
        };
        match &arg.value.kind {
            // Einheitennamen tragen jede Namensform (3.2): `bar`, `A`, `Hz`.
            ast::ExprKind::Ident(name)
            | ast::ExprKind::Upper { name, args: None }
            | ast::ExprKind::TypeName { name, args: None } => self.unit_name(name),
            ast::ExprKind::Number { value: ast::Number::Int(one), unit: Some(u) } if one.text == "1" => {
                self.unit_expr(u)
            }
            ast::ExprKind::Binary { .. } | ast::ExprKind::Paren(_) => {
                self.error_hint(
                    SC3,
                    arg.span,
                    "Einheitenausdruck erwartet",
                    "zusammengesetzte Einheiten als `1 K/min` schreiben",
                );
                None
            }
            _ => {
                self.error(SC3, arg.span, "Einheit erwartet");
                None
            }
        }
    }

    fn one_arg(&mut self, args: Option<&[ast::Arg]>, ty: TypeId, span: Span) -> Option<Expr> {
        let Some([arg]) = args else {
            self.error(SC3, span, "genau ein Argument erwartet");
            return None;
        };
        self.check(&arg.value, ty)
    }

    fn two_args(&mut self, args: Option<&[ast::Arg]>, a: TypeId, b: TypeId, span: Span) -> Option<(Expr, Expr)> {
        let Some([x, y]) = args else {
            self.error(SC3, span, "genau zwei Argumente erwartet");
            return None;
        };
        let x = self.check(&x.value, a)?;
        let y = self.check(&y.value, b)?;
        Some((x, y))
    }

    fn index(&mut self, base: &ast::Expr, index: &ast::Expr, span: Span) -> Option<Expr> {
        // Channel-Array: Element lesen mit Validitaet je Element
        if let ast::ExprKind::Ident(id) = &base.kind {
            if let Some(Entity::Channel(c)) = self.peek(&id.name).cloned() {
                let ch = &self.program.channels[c.index()];
                if ch.dir == takt_mir::program::Direction::Input && matches!(self.ty(ch.ty), Type::Array { .. }) {
                    let raw = self.input_read(c, base.span, true);
                    let elem = self.index_of(raw, index, span, true)?;
                    let key = Self::dom_key(&elem);
                    let dominated = key.as_deref().is_some_and(|k| self.dominated(k));
                    if dominated {
                        return Some(elem);
                    }
                    let ty = elem.ty;
                    return Some(Expr::new(
                        ExprKind::Checked { expr: Box::new(elem), kind: CheckedKind::Valid },
                        ty,
                        span,
                    ));
                }
            }
        }
        let b = self.expr(base, None)?;
        self.index_of(b, index, span, false)
    }

    /// Element mit Index; `Checked{Index}`, wenn der Index nicht beweisbar in der Range liegt.
    pub fn index_of(&mut self, b: Expr, index: &ast::Expr, span: Span, raw: bool) -> Option<Expr> {
        let (elem, len) = match self.ty(b.ty).clone() {
            Type::Array { elem, len } => (elem, Some(len)),
            Type::Samples { elem, len } => (elem, Some(len)),
            Type::Vec { elem, .. } => (elem, None),
            Type::Bytes { .. } => (self.intern(Type::Int { width: IntWidth::U8, unit: None, range: None }), None),
            _ => {
                let n = self.type_name(b.ty);
                self.error(SC3, span, format!("Index auf `{n}`"));
                return None;
            }
        };
        let int = self.tys.int;
        let i = self.expr(index, Some(int))?;
        if !self.is_int(i.ty) {
            let n = self.type_name(i.ty);
            self.error(SC3, index.span, format!("Index muss eine Ganzzahl sein, gefunden `{n}`"));
            return None;
        }
        let proven = match (&i.kind, len) {
            (ExprKind::Int(v), Some(n)) => *v >= 0 && (*v as u64) < u64::from(n),
            (_, Some(n)) => self.range_of(i.ty).is_some_and(|r| match (r.lo, r.hi) {
                (takt_mir::types::Const::Int(lo), takt_mir::types::Const::Int(hi)) => lo >= 0 && hi < i64::from(n),
                _ => false,
            }),
            _ => false,
        };
        let e = Expr::new(ExprKind::Index { base: Box::new(b), index: Box::new(i) }, elem, span);
        let _ = raw;
        if proven {
            Some(e)
        } else {
            Some(Expr::new(
                ExprKind::Checked { expr: Box::new(e), kind: CheckedKind::Index { len: len.unwrap_or(0) } },
                elem,
                span,
            ))
        }
    }

    fn array(&mut self, items: &[ast::Expr], hint: Option<TypeId>, span: Span) -> Option<Expr> {
        let hint_ty = hint.map(|h| self.ty(h).clone());
        match hint_ty {
            Some(Type::Table { key, value }) => {
                let table = hint.expect("Hinweis");
                let mut out = Vec::new();
                for item in items {
                    let ast::ExprKind::Tuple(_, _) = &item.kind else {
                        self.error(SC3, item.span, "Stuetzstelle `(x, y)` erwartet");
                        return None;
                    };
                    out.push(self.expr(item, Some(table))?);
                }
                let _ = (key, value);
                let lit = Expr::new(ExprKind::Array(out), table, span);
                let folded = self.fold(lit)?;
                if let ExprKind::Array(points) = &folded.kind {
                    let xs: Vec<f64> = points
                        .iter()
                        .filter_map(|p| match &p.kind {
                            ExprKind::Tuple(a, _) => match a.kind {
                                ExprKind::Float(x) => Some(x),
                                ExprKind::Int(x) => Some(x as f64),
                                _ => None,
                            },
                            _ => None,
                        })
                        .collect();
                    if xs.windows(2).any(|w| w[0] >= w[1]) {
                        self.error(SC3, span, "Stuetzstellen muessen streng steigende x haben (3.9)");
                        return None;
                    }
                }
                Some(folded)
            }
            Some(Type::Array { elem, len }) => {
                if items.len() != len as usize {
                    self.error(SC3, span, format!("{len} Elemente erwartet, {} gefunden", items.len()));
                    return None;
                }
                let out = items.iter().map(|x| self.check(x, elem)).collect::<Option<Vec<_>>>()?;
                Some(Expr::new(ExprKind::Array(out), hint.expect("Hinweis"), span))
            }
            Some(Type::Vec { elem, cap }) => {
                if items.len() > cap as usize {
                    self.error(SC3, span, format!("hoechstens {cap} Elemente"));
                    return None;
                }
                let out = items.iter().map(|x| self.check(x, elem)).collect::<Option<Vec<_>>>()?;
                Some(Expr::new(ExprKind::Array(out), hint.expect("Hinweis"), span))
            }
            Some(Type::Mat { .. }) => self.mat_literal(items, hint.expect("Hinweis"), span),
            _ => {
                let Some(first) = items.first() else {
                    self.error_hint(SC3, span, "Typ eines leeren Arrays nicht ableitbar", "Variable annotieren");
                    return None;
                };
                let f = self.expr(first, None)?;
                let elem = self.base(f.ty);
                let mut out = vec![f];
                for x in &items[1..] {
                    out.push(self.check(x, elem)?);
                }
                let ty = self.intern(Type::Array { elem, len: out.len() as u32 });
                Some(Expr::new(ExprKind::Array(out), ty, span))
            }
        }
    }

    fn cast(&mut self, expr: &ast::Expr, ty: &ast::ScalarType, span: Span) -> Option<Expr> {
        let x = self.expr(expr, None)?;
        let target = match ty {
            ast::ScalarType::Int { ty, unit: None } => {
                let width = match ty {
                    ast::IntType::Int | ast::IntType::I64 => IntWidth::I64,
                    ast::IntType::I8 => IntWidth::I8,
                    ast::IntType::I16 => IntWidth::I16,
                    ast::IntType::I32 => IntWidth::I32,
                    ast::IntType::U8 => IntWidth::U8,
                    ast::IntType::U16 => IntWidth::U16,
                    ast::IntType::U32 => IntWidth::U32,
                    ast::IntType::U64 => IntWidth::U64,
                };
                self.intern(Type::Int { width, unit: None, range: None })
            }
            ast::ScalarType::Float { width, unit: None } => {
                let width = match width {
                    Some(ast::FloatWidth::F32) => FloatWidth::F32,
                    Some(ast::FloatWidth::F64) => FloatWidth::F64,
                    None => self.float_width(),
                };
                self.intern(Type::Float { width, unit: None, range: None })
            }
            _ => {
                self.error_hint(
                    SC3,
                    span,
                    "`as` nur nach Ganzzahl- und Fliesskommabreiten (3.10)",
                    "Einheiten mit `.to(U)` wandeln",
                );
                return None;
            }
        };
        // `as` wechselt die Darstellung, nicht die Groesse: Die Einheit bleibt.
        let unit = match self.ty(x.ty) {
            Type::Int { unit, .. } | Type::Float { unit, .. } => *unit,
            _ => None,
        };
        let target = if unit.is_some() { self.with_unit(target, unit) } else { target };
        let (from_t, to_t) = (self.ty(x.ty).clone(), self.ty(target).clone());
        let checked = match (&from_t, &to_t) {
            (Type::Int { width: a, .. }, Type::Int { width: b, .. }) => {
                let (la, ha) = takt_interp::arith::bounds(*a);
                let (lb, hb) = takt_interp::arith::bounds(*b);
                !(la >= lb && ha <= hb)
            }
            (Type::Int { .. }, Type::Float { .. }) => false,
            (Type::Float { .. }, Type::Float { .. }) => false,
            (Type::Float { .. }, Type::Int { .. }) => {
                self.error_hint(
                    SC3,
                    span,
                    "Fliesskomma nach Ganzzahl nur ueber `round`, `floor`, `ceil` (4.1)",
                    "`round(x) as u8`",
                );
                return None;
            }
            (Type::Enum(_), _) => {
                let n = self.type_name(x.ty);
                self.error_hint(
                    SC3,
                    span,
                    format!("`as` auf `{n}`"),
                    "ein Enum traegt seine Diskriminante ins Drahtformat (`layout`, 3.7), nicht in die Rechnung",
                );
                return None;
            }
            _ => {
                let n = self.type_name(x.ty);
                self.error(SC3, span, format!("`as` auf `{n}`"));
                return None;
            }
        };
        let cast = Expr::new(ExprKind::Cast { expr: Box::new(x), to: target }, target, span);
        if checked {
            Some(Expr::new(ExprKind::Checked { expr: Box::new(cast), kind: CheckedKind::Convert }, target, span))
        } else {
            Some(cast)
        }
    }

    /// Traegt der Typ eine affine Einheit (`degC`, `degF`, 3.2)?
    fn affine_type(&self, ty: TypeId) -> bool {
        match self.unit_of_type(ty) {
            Some(u) => self.units.is_affine(&self.program, &u),
            None => false,
        }
    }

    fn unary(&mut self, op: ast::UnaryOp, expr: &ast::Expr, hint: Option<TypeId>, span: Span) -> Option<Expr> {
        match op {
            ast::UnaryOp::Not => {
                let x = self.check_bool(expr)?;
                let ty = self.tys.bool;
                Some(Expr::new(ExprKind::Unary { op: UnaryOp::Not, expr: Box::new(x) }, ty, span))
            }
            ast::UnaryOp::Neg => {
                // Ein negatives Literal ist *ein* Wert (3.1, 3.2): Breite und
                // Range gelten fuer ihn, nicht fuer seinen Betrag — `-32768`
                // passt in `i16`, `-20 inc` in `-100..0 inc`, und `-60 degC`
                // ist ein Punkt, nicht das Negative eines Punkts (FB-184).
                if let ast::ExprKind::Number { value, unit } = &expr.kind {
                    let negated = match value {
                        ast::Number::Int(i) => {
                            ast::Number::Int(ast::IntLit { text: format!("-{}", i.text), span: i.span })
                        }
                        ast::Number::Float(f) => {
                            ast::Number::Float(ast::FloatLit { text: format!("-{}", f.text), span: f.span })
                        }
                    };
                    return self.number(&negated, unit.as_ref(), hint, span);
                }
                let x = self.expr(expr, hint)?;
                if !(self.is_numeric(x.ty) || self.is_duration(x.ty)) {
                    let n = self.type_name(x.ty);
                    self.error(SC3, span, format!("`-` auf `{n}`"));
                    return None;
                }
                // Unaeres Minus ist die Multiplikation mit -1; auf einem
                // affinen Punkt ist sie ein Typfehler wie jedes Produkt
                // (3.2). `-(20 degC)` waere -293.15 K. Ein negatives Literal
                // wie `-60 degC` bezeichnet dagegen einen Punkt und ist
                // erlaubt (14.2 schreibt `in -60..200 degC`).
                if self.affine_type(x.ty) && !is_literal(&x) {
                    self.error_hint(SC3, span, "`-` auf einer affinen Einheit (3.2)", "Differenzen in `K` rechnen");
                    return None;
                }
                if let Type::Int { width, .. } = self.ty(x.ty) {
                    if !width.signed() {
                        self.error(SC3, span, "`-` auf unsignierter Breite");
                        return None;
                    }
                }
                let ty = self.base(x.ty);
                Some(Expr::new(ExprKind::Unary { op: UnaryOp::Neg, expr: Box::new(x) }, ty, span))
            }
            ast::UnaryOp::BitNot => {
                let x = self.expr(expr, hint)?;
                if !self.is_int(x.ty) {
                    let n = self.type_name(x.ty);
                    self.error(SC3, span, format!("`~` auf `{n}`"));
                    return None;
                }
                let ty = self.base(x.ty);
                Some(Expr::new(ExprKind::Unary { op: UnaryOp::BitNot, expr: Box::new(x) }, ty, span))
            }
        }
    }

    fn binary(
        &mut self,
        op: ast::BinaryOp,
        lhs: &ast::Expr,
        rhs: &ast::Expr,
        hint: Option<TypeId>,
        span: Span,
    ) -> Option<Expr> {
        use ast::BinaryOp as B;
        let mop = match op {
            B::Or => BinaryOp::Or,
            B::And => BinaryOp::And,
            B::Lt => BinaryOp::Lt,
            B::Le => BinaryOp::Le,
            B::Gt => BinaryOp::Gt,
            B::Ge => BinaryOp::Ge,
            B::Eq => BinaryOp::Eq,
            B::Ne => BinaryOp::Ne,
            B::BitOr => BinaryOp::BitOr,
            B::BitXor => BinaryOp::BitXor,
            B::BitAnd => BinaryOp::BitAnd,
            B::Shl => BinaryOp::Shl,
            B::Shr => BinaryOp::Shr,
            B::Add => BinaryOp::Add,
            B::Sub => BinaryOp::Sub,
            B::Mul => BinaryOp::Mul,
            B::Div => BinaryOp::Div,
            B::Rem => BinaryOp::Rem,
        };
        let bool = self.tys.bool;
        if matches!(mop, BinaryOp::And | BinaryOp::Or) {
            let a = self.check_bool(lhs)?;
            let facts = if mop == BinaryOp::And { Self::facts_of(&a) } else { Vec::new() };
            self.facts.push(facts);
            let b = self.check_bool(rhs);
            self.facts.pop();
            let b = b?;
            return Some(Expr::new(ExprKind::Binary { op: mop, lhs: Box::new(a), rhs: Box::new(b) }, bool, span));
        }
        // Literale passen sich der anderen Seite an
        let (a, b) = if is_plain_literal(lhs) && !is_plain_literal(rhs) {
            let b = self.expr(rhs, hint.filter(|_| !is_comparison(mop)))?;
            let a_hint = self.operand_hint(mop, b.ty);
            let a = self.expr(lhs, Some(a_hint))?;
            (a, b)
        } else {
            let a = self.expr(lhs, hint.filter(|_| !is_comparison(mop)))?;
            let b_hint = self.operand_hint(mop, a.ty);
            let b = self.expr(rhs, Some(b_hint))?;
            (a, b)
        };
        let (ta, tb) = (self.ty(a.ty).clone(), self.ty(b.ty).clone());
        if matches!(ta, Type::Mat { .. }) || matches!(tb, Type::Mat { .. }) {
            return self.mat_binary(mop, a, b, span);
        }
        let names = |this: &Self| (this.type_name(a.ty), this.type_name(b.ty));
        let result = match mop {
            BinaryOp::Eq | BinaryOp::Ne => {
                if !self.comparable(a.ty, b.ty) {
                    let (x, y) = names(self);
                    self.error_hint(
                        SC3,
                        span,
                        format!("`==` zwischen `{x}` und `{y}`"),
                        unit_hint(self, a.ty, b.ty, &b),
                    );
                    return None;
                }
                bool
            }
            BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => {
                let ordered = self.comparable(a.ty, b.ty)
                    && (self.is_numeric(a.ty)
                        || self.is_duration(a.ty)
                        || matches!(ta, Type::Str { .. } | Type::Line { .. } | Type::Bool));
                if !ordered {
                    let (x, y) = names(self);
                    self.error_hint(
                        SC3,
                        span,
                        format!("`<` zwischen `{x}` und `{y}`"),
                        unit_hint(self, a.ty, b.ty, &b),
                    );
                    return None;
                }
                bool
            }
            BinaryOp::Add | BinaryOp::Sub => match (&ta, &tb) {
                (Type::Duration { .. }, Type::Duration { .. }) => self.tys.duration,
                (Type::Int { width: wa, .. }, Type::Int { width: wb, .. }) => {
                    if wa != wb {
                        let (x, y) = names(self);
                        self.error_hint(
                            SC3,
                            span,
                            format!("gemischte Breiten `{x}` und `{y}` (3.10)"),
                            "mit `as` angleichen",
                        );
                        return None;
                    }
                    let (ua, ub) = (self.unit_of_type(a.ty).expect("int"), self.unit_of_type(b.ty).expect("int"));
                    self.affine_result(mop, &ua, &ub, a.ty, span)?
                }
                (Type::Float { width: wa, .. }, Type::Float { width: wb, .. }) => {
                    if wa != wb {
                        let (x, y) = names(self);
                        self.error(SC3, span, format!("`{x}` und `{y}` werden nie implizit gemischt (4.2)"));
                        return None;
                    }
                    let (ua, ub) = (self.unit_of_type(a.ty).expect("float"), self.unit_of_type(b.ty).expect("float"));
                    self.affine_result(mop, &ua, &ub, a.ty, span)?
                }
                _ => {
                    let (x, y) = names(self);
                    self.error(SC3, span, format!("`+`/`-` zwischen `{x}` und `{y}`"));
                    return None;
                }
            },
            BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem => match (&ta, &tb) {
                (Type::Duration { .. }, Type::Int { width: IntWidth::I64, .. }) if mop != BinaryOp::Rem => {
                    self.tys.duration
                }
                (Type::Int { width: IntWidth::I64, .. }, Type::Duration { .. }) if mop == BinaryOp::Mul => {
                    self.tys.duration
                }
                (Type::Duration { .. }, Type::Duration { .. }) if mop == BinaryOp::Div => self.tys.int,
                (Type::Int { width: wa, .. }, Type::Int { width: wb, .. }) => {
                    if wa != wb {
                        let (x, y) = names(self);
                        self.error_hint(
                            SC3,
                            span,
                            format!("gemischte Breiten `{x}` und `{y}` (3.10)"),
                            "mit `as` angleichen",
                        );
                        return None;
                    }
                    let (ua, ub) = (self.unit_of_type(a.ty).expect("int"), self.unit_of_type(b.ty).expect("int"));
                    if mop == BinaryOp::Rem {
                        if ua != ub {
                            self.error(SC3, span, "`%` verlangt gleiche Einheiten (3.2)");
                            return None;
                        }
                        self.base(a.ty)
                    } else {
                        self.product_type(mop, &ua, &ub, a.ty, span)?
                    }
                }
                (Type::Float { width: wa, .. }, Type::Float { width: wb, .. }) => {
                    if mop == BinaryOp::Rem {
                        self.error(SC3, span, "`%` nur auf Ganzzahlen");
                        return None;
                    }
                    if wa != wb {
                        let (x, y) = names(self);
                        self.error(SC3, span, format!("`{x}` und `{y}` werden nie implizit gemischt (4.2)"));
                        return None;
                    }
                    let (ua, ub) = (self.unit_of_type(a.ty).expect("float"), self.unit_of_type(b.ty).expect("float"));
                    self.product_type(mop, &ua, &ub, a.ty, span)?
                }
                _ => {
                    let (x, y) = names(self);
                    self.error(SC3, span, format!("`*`/`/` zwischen `{x}` und `{y}`"));
                    return None;
                }
            },
            BinaryOp::BitAnd | BinaryOp::BitOr | BinaryOp::BitXor => match (&ta, &tb) {
                (Type::Int { width: wa, .. }, Type::Int { width: wb, .. }) if wa == wb => self.base(a.ty),
                _ => {
                    let (x, y) = names(self);
                    self.error(SC3, span, format!("Bitoperation zwischen `{x}` und `{y}`"));
                    return None;
                }
            },
            BinaryOp::Shl | BinaryOp::Shr => match (&ta, &tb) {
                (Type::Int { .. }, Type::Int { .. }) => self.base(a.ty),
                _ => {
                    let (x, y) = names(self);
                    self.error(SC3, span, format!("Shift zwischen `{x}` und `{y}`"));
                    return None;
                }
            },
            BinaryOp::And | BinaryOp::Or => unreachable!(),
        };
        let shift_checked = matches!(mop, BinaryOp::Shl | BinaryOp::Shr) && !matches!(b.kind, ExprKind::Int(_));
        let e = Expr::new(ExprKind::Binary { op: mop, lhs: Box::new(a), rhs: Box::new(b) }, result, span);
        if shift_checked {
            Some(Expr::new(ExprKind::Checked { expr: Box::new(e), kind: CheckedKind::Shift }, result, span))
        } else {
            Some(e)
        }
    }

    /// Erwartung fuer den zweiten Operanden aus dem ersten.
    fn operand_hint(&mut self, op: BinaryOp, other: TypeId) -> TypeId {
        match op {
            BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem => {
                if self.is_duration(other) {
                    self.tys.int
                } else if matches!(self.ty(other), Type::Mat { .. }) {
                    self.tys.float
                } else {
                    self.without_unit(other)
                }
            }
            BinaryOp::Shl | BinaryOp::Shr => self.tys.int,
            _ => self.base(other),
        }
    }

    /// Gleich modulo Range, mit affinen Vergleichen.
    fn comparable(&mut self, a: TypeId, b: TypeId) -> bool {
        self.same_base(a, b)
    }

    /// Ergebnis von `+`/`-` mit affinen Regeln (3.2); Breite und Art
    /// kommen vom linken Operanden.
    fn affine_result(&mut self, op: BinaryOp, ua: &Unit, ub: &Unit, a_ty: TypeId, span: Span) -> Option<TypeId> {
        let (aff_a, aff_b) = (self.units.is_affine(&self.program, ua), self.units.is_affine(&self.program, ub));
        if !aff_a && !aff_b {
            if ua != ub {
                let (x, y) = (self.units.display(&self.program, ua), self.units.display(&self.program, ub));
                self.error_hint(
                    SC3,
                    span,
                    format!("Einheiten `{x}` und `{y}` passen nicht"),
                    "mit `.to(U)` wandeln (3.2)",
                );
                return None;
            }
            return Some(self.base(a_ty));
        }
        // Basiseinheit der affinen Einheit (K fuer degC)
        let base_of = |this: &Self, u: &Unit| -> Unit {
            let id = match u.factors.as_slice() {
                [(crate::units::Atom::Named(id), 1)] => *id,
                _ => return u.clone(),
            };
            let dim = this.program.units[id.index()].dimension;
            let base = this
                .program
                .units
                .iter()
                .position(|d| {
                    d.dimension == dim && d.affine_offset.is_none() && d.factor == takt_mir::types::Rational::int(1)
                })
                .map(|i| UnitId(i as u32));
            base.map_or_else(|| u.clone(), Unit::named)
        };
        match (aff_a, aff_b, op) {
            (true, true, BinaryOp::Sub) if ua == ub => {
                let k = base_of(self, ua);
                self.numeric_type(a_ty, &k, span)
            }
            (true, false, _) if base_of(self, ua) == *ub => Some(self.base(a_ty)),
            (false, true, BinaryOp::Add) if base_of(self, ub) == *ua => self.numeric_type(a_ty, ub, span),
            _ => {
                let (x, y) = (self.units.display(&self.program, ua), self.units.display(&self.program, ub));
                self.error_hint(
                    SC3,
                    span,
                    format!("`{x}` und `{y}`: affine Einheiten sind Punkte, Differenzen sind Vektoren (3.2)"),
                    "`degC - degC` ergibt `K`; `degC + K` ergibt `degC`; `degC + degC` gibt es nicht",
                );
                None
            }
        }
    }

    /// Ergebnis von `*`/`/`: Einheiten kombinieren; affine Einheiten sind
    /// in Produkten ausgeschlossen (3.2).
    fn product_type(&mut self, op: BinaryOp, ua: &Unit, ub: &Unit, a_ty: TypeId, span: Span) -> Option<TypeId> {
        if self.units.is_affine(&self.program, ua) || self.units.is_affine(&self.program, ub) {
            self.error_hint(SC3, span, "affine Einheit in einem Produkt (3.2)", "Differenzen in `K` rechnen");
            return None;
        }
        let unit = if op == BinaryOp::Mul { ua.mul(ub) } else { ua.div(ub) };
        self.numeric_type(a_ty, &unit, span)
    }

    /// `.to(U)` verlangt gleiche Dimension (3.2).
    fn check_dimension(&mut self, src: &Unit, dst: &Unit, span: Span) -> Option<()> {
        if self.units.dimension(&self.program, src) == self.units.dimension(&self.program, dst) {
            return Some(());
        }
        let (a, c) = (self.units.display(&self.program, src), self.units.display(&self.program, dst));
        self.error(SC3, span, format!("`.to({c})`: `{a}` hat eine andere Dimension"));
        None
    }

    /// Faktor von `src` nach `dst`, wenn er ganzzahlig ist (Pruefung 38).
    fn integral_factor(&self, src: &Unit, dst: &Unit) -> Option<i128> {
        let (fs, fd) = (self.units.factor(&self.program, src)?, self.units.factor(&self.program, dst)?);
        let num = i128::from(fs.num) * i128::from(fd.den);
        let den = u128::from(fs.den) * u128::from(fd.num.unsigned_abs());
        let r = crate::units::rat_reduce(num, den)?;
        (r.den == 1 && r.num > 0).then_some(i128::from(r.num))
    }
}

/// Wrapper-Zugriffe (2.5, 3.5, 3.8).
fn is_wrapper(name: &str) -> bool {
    matches!(name, "valid" | "suspect" | "stale" | "age" | "reason" | "or" | "ok" | "err")
}

fn is_plain_literal(e: &ast::Expr) -> bool {
    matches!(e.kind, ast::ExprKind::Number { unit: None, .. })
}

fn is_comparison(op: BinaryOp) -> bool {
    matches!(op, BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge | BinaryOp::Eq | BinaryOp::Ne)
}

fn unit_hint(this: &Lowerer<'_>, a: TypeId, b: TypeId, rhs: &Expr) -> String {
    let unit_of = |t: &Type| match t {
        Type::Float { unit, .. } | Type::Int { unit, .. } => Some(*unit),
        _ => None,
    };
    match (unit_of(this.ty(a)), unit_of(this.ty(b))) {
        (Some(Some(u)), Some(None)) if is_literal(rhs) => {
            let text = match &rhs.kind {
                ExprKind::Int(i) => i.to_string(),
                ExprKind::Float(f) => format!("{f}"),
                _ => "…".into(),
            };
            format!("meinst du `{text} {}`?", this.program.units[u.index()].name)
        }
        // 3.2: „Es gibt keine implizite Konversion" — bei gleicher Dimension
        // ist `.to(U)` die Umrechnung, die der Techniker sehen soll.
        (Some(Some(x)), Some(Some(y))) if same_dimension(this, x, y) => {
            format!("`.to({})` umrechnen (3.2)", this.program.units[x.index()].name)
        }
        _ => "beide Seiten muessen denselben Typ haben".into(),
    }
}

/// Haben zwei Einheiten dieselbe Dimension? Dann trennt sie nur ein Faktor.
fn same_dimension(this: &Lowerer<'_>, a: takt_mir::UnitId, b: takt_mir::UnitId) -> bool {
    if a == b {
        return false;
    }
    let (ua, ub) = (this.units.unit_of(a), this.units.unit_of(b));
    this.units.dimension(&this.program, &ua) == this.units.dimension(&this.program, &ub)
}

/// Ganzzahltext in jeder Schreibweise.
pub fn parse_int(text: &str) -> Option<i128> {
    let t = text.replace('_', "");
    let (negative, t) = match t.strip_prefix('-') {
        Some(rest) => (true, rest.to_string()),
        None => (false, t),
    };
    let magnitude = if let Some(h) = t.strip_prefix("0x") {
        i128::from_str_radix(h, 16).ok()
    } else if let Some(b) = t.strip_prefix("0b") {
        i128::from_str_radix(b, 2).ok()
    } else if let Some(o) = t.strip_prefix("0o") {
        i128::from_str_radix(o, 8).ok()
    } else {
        t.parse().ok()
    }?;
    Some(if negative { -magnitude } else { magnitude })
}

/// Fliesskommatext in Programmbreite korrekt gerundet.
pub fn parse_float(text: &str, width: FloatWidth) -> Option<f64> {
    if let Some(i) = parse_int(text) {
        return Some(match width {
            FloatWidth::F32 => f64::from(i as f32),
            FloatWidth::F64 => i as f64,
        });
    }
    match width {
        FloatWidth::F32 => text.parse::<f32>().ok().map(f64::from),
        FloatWidth::F64 => text.parse::<f64>().ok(),
    }
    .filter(|f| f.is_finite())
}

fn is_zero(text: &str) -> bool {
    parse_int(text).map_or_else(|| text.parse::<f64>().ok() == Some(0.0), |i| i == 0)
}

/// Konstante als Literal.
pub fn const_expr(c: takt_mir::types::Const, ty: TypeId, span: Span) -> Expr {
    use takt_mir::types::Const;
    let kind = match c {
        Const::Int(i) => ExprKind::Int(i),
        Const::Float(f) => ExprKind::Float(f),
        Const::Duration(d) => ExprKind::Duration(d),
        Const::Bool(b) => ExprKind::Bool(b),
    };
    Expr::new(kind, ty, span)
}

/// Einheitenliteral-Vorschlag ist in `coerce` und `binary` erklaert; hier
/// bleibt nur der Bezug auf den Namen der Pruefung fuer Leser.
pub const NAME_CHECK: &str = SC2;

/// Liest der Ausdruck einen Input-Channel, gegebenenfalls ein Element eines
/// Channel-Arrays (8.1)? Nur dann tragen `.suspect`, `.stale`, `.age` und
/// `.reason` eine Bedeutung (3.5).
fn is_channel_read(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Input { .. } => true,
        ExprKind::Index { base, .. } => is_channel_read(base),
        _ => false,
    }
}

/// Index und Typ eines Recordfelds; `None`, wenn der Typ kein Record ist
/// oder das Feld fehlt.
/// Die Namen der Bitfelder eines Traegerfelds, falls es welche hat (3.7).
fn bitfield_names(lo: &Lowerer<'_>, base: &Expr) -> Option<Vec<String>> {
    let ExprKind::Field { base: owner, field } = &base.kind else { return None };
    let Type::Record(r) = lo.program.types.list.get(owner.ty.index())? else { return None };
    let def = lo.program.records[r.index()].fields.get(*field as usize)?;
    if def.bits.is_empty() {
        return None;
    }
    Some(def.bits.iter().map(|b| b.name.clone()).collect())
}

/// Positionen und Typ eines benannten Bitfelds (3.7), wenn `base` das
/// Traegerfeld eines Records ist und dieses ein Bitfeld `name` deklariert.
pub(crate) fn bitfield_of(lo: &Lowerer<'_>, base: &Expr, name: &str) -> Option<takt_mir::types::BitfieldDef> {
    let ExprKind::Field { base: owner, field } = &base.kind else { return None };
    let Type::Record(r) = lo.program.types.list.get(owner.ty.index())? else { return None };
    let def = lo.program.records[r.index()].fields.get(*field as usize)?;
    def.bits.iter().find(|b| b.name == name).cloned()
}

fn record_field(lo: &Lowerer<'_>, ty: TypeId, name: &str) -> Option<(u32, TypeId)> {
    let Type::Record(r) = lo.program.types.list.get(ty.index())? else { return None };
    let f = lo.program.records[r.index()].fields.iter().position(|f| f.name == name)?;
    Some((f as u32, lo.program.records[r.index()].fields[f].ty))
}

/// Die Funktion der Mathematikbibliothek zu einer Primitive.
///
/// `None` heisst: Die Primitive rechnet nicht mit `libtaktm` — Integer-
/// und Vergleichsoperationen sind exakt definiert und brauchen keine.
fn libtaktm_fun(op: Intrinsic) -> Option<libtaktm::Fun> {
    Some(match op {
        Intrinsic::Sqrt => libtaktm::Fun::Sqrt,
        Intrinsic::Fma => libtaktm::Fun::Fma,
        Intrinsic::Sin => libtaktm::Fun::Sin,
        Intrinsic::Cos => libtaktm::Fun::Cos,
        Intrinsic::Tan => libtaktm::Fun::Tan,
        Intrinsic::Asin => libtaktm::Fun::Asin,
        Intrinsic::Acos => libtaktm::Fun::Acos,
        Intrinsic::Atan => libtaktm::Fun::Atan,
        Intrinsic::Atan2 => libtaktm::Fun::Atan2,
        Intrinsic::Exp => libtaktm::Fun::Exp,
        Intrinsic::Log => libtaktm::Fun::Log,
        Intrinsic::Pow => libtaktm::Fun::Pow,
        // `round`, `floor` und `ceil` liefern in Takt einen Integer (4.1);
        // die Rundung selbst ist exakt, die Range-Pruefung macht der
        // Interpreter. Sie brauchen die Bibliothek nicht.
        Intrinsic::Abs
        | Intrinsic::Min
        | Intrinsic::Max
        | Intrinsic::Round
        | Intrinsic::Floor
        | Intrinsic::Ceil
        | Intrinsic::Rotl
        | Intrinsic::Rotr
        | Intrinsic::WrappingAdd
        | Intrinsic::WrappingSub
        | Intrinsic::WrappingMul
        | Intrinsic::SaturatingAdd
        | Intrinsic::SaturatingSub
        | Intrinsic::Interp => return None,
    })
}
