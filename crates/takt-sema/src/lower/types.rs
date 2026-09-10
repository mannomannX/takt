//! Typauflösung (plan/m1.md 3.5): `ast::Type` → `TypeId`, Einheitenausdrücke,
//! Ranges, Typnamen in Quellschreibweise, Vergleich modulo Range.

use takt_diag::{Span, Stage};
use takt_mir::expr::{Expr, ExprKind};
use takt_mir::types::{Const, FloatWidth, IntWidth, Range, RangeOrigin, Type};
use takt_mir::{TypeId, UnitId};
use takt_syntax::ast;

use super::{Lowerer, SC3};
use crate::symbols::Entity;
use crate::units::{Atom, Unit};

impl Lowerer<'_> {
    /// Einheitenausdruck → Normalform; Namen kommen aus der generischen
    /// Umgebung oder der Einheitentabelle (mit Praefixen).
    pub fn unit_expr(&mut self, u: &ast::UnitExpr) -> Option<Unit> {
        let mut out = self.unit_term(&u.first)?;
        for (op, term) in &u.rest {
            let t = self.unit_term(term)?;
            out = match op {
                ast::UnitOp::Mul => out.mul(&t),
                ast::UnitOp::Div => out.div(&t),
            };
        }
        Some(out)
    }

    fn unit_term(&mut self, t: &ast::UnitTerm) -> Option<Unit> {
        let base = match &t.name {
            None => Unit::one(),
            Some(name) => self.unit_name(name)?,
        };
        match &t.exponent {
            None => Some(base),
            Some(e) => {
                let k: i32 = e.text.replace('_', "").parse().ok().filter(|k: &i32| k.abs() <= 8).or_else(|| {
                    self.error(SC3, e.span, "Exponent einer Einheit muss eine kleine ganze Zahl sein");
                    None
                })?;
                Some(base.pow(k))
            }
        }
    }

    /// Einheitenname: generische Variable oder benannte Einheit.
    pub fn unit_name(&mut self, name: &ast::Ident) -> Option<Unit> {
        if let Some(i) = self.env.index(&name.name) {
            return Some(match &self.env.units[i as usize] {
                Some(u) => u.clone(),
                None => Unit::var(i),
            });
        }
        match self.units.lookup(&mut self.program, &name.name) {
            Some(id) => Some(self.units.unit_of(id)),
            None => {
                self.error_hint(
                    SC3,
                    name.span,
                    format!("unbekannte Einheit `{}`", name.name),
                    "Einheit mit `unit` deklarieren oder einen Praefix einer SI-Einheit verwenden",
                );
                None
            }
        }
    }

    /// `UnitId` einer Normalform; offene Variablen sind ausserhalb der
    /// generischen Pruefung ein Fehler.
    pub fn unit_id(&mut self, unit: &Unit, span: Span) -> Option<UnitId> {
        if unit.is_one() {
            return None;
        }
        let unit = if unit.has_vars() && self.checking > 0 { self.freeze_vars(unit) } else { unit.clone() };
        match self.units.intern(&mut self.program, &unit) {
            Ok(id) => Some(id),
            Err(msg) => {
                self.error(SC3, span, msg);
                None
            }
        }
    }

    /// Ersetzt offene Variablen durch Platzhalter-Einheiten der Programmkopie
    /// (nur waehrend der generischen Pruefung).
    fn freeze_vars(&mut self, unit: &Unit) -> Unit {
        let mut out = Unit::one();
        for (atom, e) in &unit.factors {
            let term = match atom {
                Atom::Var(v) => {
                    let name = format!("?{}", self.env.names.get(*v as usize).cloned().unwrap_or_default());
                    let id = match self.units.get(&name) {
                        Some(id) => id,
                        None => self
                            .units
                            .declare(
                                &mut self.program,
                                &name,
                                [0; 7],
                                takt_mir::types::Rational::int(1),
                                None,
                                false,
                                false,
                                Span::default(),
                            )
                            .expect("Platzhalter"),
                    };
                    Unit::named(id).pow(i32::from(*e))
                }
                Atom::Named(_) => Unit { factors: vec![(*atom, *e)], overflow: false },
            };
            out = out.mul(&term);
        }
        out
    }

    /// Einheit eines numerischen Typs als Normalform.
    pub fn unit_of_type(&self, ty: TypeId) -> Option<Unit> {
        match self.ty(ty) {
            Type::Float { unit, .. } | Type::Int { unit, .. } => Some(match unit {
                Some(id) => self.units.unit_of(*id),
                None => Unit::one(),
            }),
            _ => None,
        }
    }

    /// Typ mit Einheit und Range in Normalform interniert.
    pub fn float_type(&mut self, width: FloatWidth, unit: &Unit, range: Option<Range>, span: Span) -> Option<TypeId> {
        // Ein geklemmter Exponent machte zwei nominal verschiedene Einheiten
        // identisch (3.2); die MIR traegt ihn als `i8`.
        if unit.overflow {
            self.error_hint(
                SC3,
                span,
                format!("Einheitenexponent ueberschreitet {}", Unit::MAX_EXPONENT),
                "Einheit mit kleineren Exponenten waehlen",
            );
            return None;
        }
        let unit = self.unit_id(unit, span);
        Some(self.intern(Type::Float { width, unit, range }))
    }

    /// `ast::Type` → `TypeId`.
    pub fn resolve_type(&mut self, t: &ast::Type) -> Option<TypeId> {
        let span = t.span;
        match &t.kind {
            ast::TypeKind::Scalar { scalar, range, wrap } => {
                let base = self.scalar_type(scalar, span)?;
                let ranged = match range {
                    Some(r) => {
                        let (rg, effective) = self.range(r, base)?;
                        self.with_range(effective, Some(rg))
                    }
                    None => base,
                };
                Some(self.wrap(ranged, wrap.as_ref(), span))
            }
            ast::TypeKind::Array { len, elem } => {
                let n = self.const_int(len)?;
                let elem = self.resolve_type(elem)?;
                if n <= 0 || n > 1 << 20 {
                    self.error(SC3, len.span, format!("Array-Laenge {n} ausserhalb 1..2^20"));
                    return None;
                }
                Some(self.intern(Type::Array { elem, len: n as u32 }))
            }
            ast::TypeKind::Named { name, wrap } => {
                let base = match self.lookup(name)? {
                    Entity::Type(t) => t,
                    Entity::Enum(e) => self.intern(Type::Enum(e)),
                    Entity::Record(r) => self.intern(Type::Record(r)),
                    _ => {
                        self.error(SC3, name.span, format!("`{}` ist kein Typ", name.name));
                        return None;
                    }
                };
                Some(self.wrap(base, wrap.as_ref(), span))
            }
            ast::TypeKind::Bytes(n) => {
                let cap = self.const_cap(n)?;
                Some(self.intern(Type::Bytes { cap }))
            }
            ast::TypeKind::Vec { elem, len } => {
                let elem = self.resolve_type(elem)?;
                let cap = self.const_cap(len)?;
                Some(self.intern(Type::Vec { elem, cap }))
            }
            ast::TypeKind::Line(n) => {
                let cap = self.const_cap(n)?;
                Some(self.intern(Type::Line { cap }))
            }
            ast::TypeKind::Stream(elem) => {
                let elem = self.elem_type(elem)?;
                Some(self.intern(Type::Stream(elem)))
            }
            ast::TypeKind::Samples { elem, len } => {
                let elem = self.resolve_type(elem)?;
                let len = self.const_cap(len)?;
                Some(self.intern(Type::Samples { elem, len }))
            }
            ast::TypeKind::Table { key, value } => {
                let key = self.resolve_type(key)?;
                let value = self.resolve_type(value)?;
                Some(self.intern(Type::Table { key, value }))
            }
            ast::TypeKind::Mat { rows, cols, unit } => {
                let rows = self.const_cap(rows)?;
                let cols = self.const_cap(cols)?;
                let units = match unit {
                    Some(u) => {
                        let unit = self.unit_expr(u)?;
                        takt_mir::types::MatUnits::Uniform(self.unit_id(&unit, u.span))
                    }
                    None => takt_mir::types::MatUnits::Uniform(None),
                };
                Some(self.intern(Type::Mat { rows, cols, units }))
            }
            ast::TypeKind::MatDim { .. } | ast::TypeKind::VecDim(_) => {
                self.stage(span, "dimensionierte Matrizen", Stage::V1_1);
                None
            }
            ast::TypeKind::Map { key, value, len } => {
                let key = self.resolve_type(key)?;
                let value = self.resolve_type(value)?;
                let cap = self.const_cap(len)?;
                Some(self.intern(Type::Map { key, value, cap }))
            }
            ast::TypeKind::TypeVar { name, .. } => {
                self.stage(name.span, "Typvariablen", Stage::V1_2);
                None
            }
        }
    }

    /// Elementtyp eines Streams.
    pub fn elem_type(&mut self, e: &ast::ElemType) -> Option<TypeId> {
        match e {
            ast::ElemType::U8 => Some(self.intern(Type::Int { width: IntWidth::U8, unit: None, range: None })),
            ast::ElemType::Bytes(n) => {
                let cap = self.const_cap(n)?;
                Some(self.intern(Type::Bytes { cap }))
            }
            ast::ElemType::Line(n) => {
                let cap = self.const_cap(n)?;
                Some(self.intern(Type::Line { cap }))
            }
            ast::ElemType::Edge => match self.peek("Edge").cloned() {
                Some(Entity::Record(r)) => Some(self.intern(Type::Record(r))),
                _ => None,
            },
            ast::ElemType::Named(name) => match self.lookup(name)? {
                Entity::Record(r) => Some(self.intern(Type::Record(r))),
                Entity::Enum(e) => Some(self.intern(Type::Enum(e))),
                Entity::Type(t) => Some(t),
                _ => {
                    self.error(SC3, name.span, format!("`{}` ist kein Elementtyp", name.name));
                    None
                }
            },
            ast::ElemType::Capture { elem, len } => {
                let elem = self.resolve_type(elem)?;
                let len = self.const_cap(len)?;
                Some(self.intern(Type::Capture { elem, len }))
            }
        }
    }

    fn const_cap(&mut self, e: &ast::Expr) -> Option<u32> {
        let n = self.const_int(e)?;
        if !(0..=1 << 24).contains(&n) {
            self.error(SC3, e.span, format!("Kapazitaet {n} ausserhalb 0..2^24"));
            return None;
        }
        Some(n as u32)
    }

    fn scalar_type(&mut self, s: &ast::ScalarType, span: Span) -> Option<TypeId> {
        match s {
            ast::ScalarType::Bool => Some(self.tys.bool),
            ast::ScalarType::Int { ty, unit } => {
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
                let unit = match unit {
                    Some(u) => {
                        self.stage(u.span, "Einheiten auf Ganzzahlen", Stage::V1_1);
                        return None;
                    }
                    None => None,
                };
                Some(self.intern(Type::Int { width, unit, range: None }))
            }
            ast::ScalarType::Float { width, unit } => {
                let width = match width {
                    Some(ast::FloatWidth::F32) => FloatWidth::F32,
                    Some(ast::FloatWidth::F64) => FloatWidth::F64,
                    None => self.float_width(),
                };
                let unit = match unit {
                    Some(u) => {
                        let unit = self.unit_expr(u)?;
                        self.unit_id(&unit, u.span)
                    }
                    None => None,
                };
                Some(self.intern(Type::Float { width, unit, range: None }))
            }
            ast::ScalarType::Duration => Some(self.tys.duration),
            ast::ScalarType::Str(n) => {
                let cap = self.const_cap(n)?;
                let _ = span;
                Some(self.intern(Type::Str { cap }))
            }
        }
    }

    /// `T?` oder `T!E`.
    fn wrap(&mut self, inner: TypeId, wrap: Option<&ast::Wrap>, span: Span) -> TypeId {
        match wrap {
            None => inner,
            Some(ast::Wrap::Optional) => self.intern(Type::Optional(inner)),
            Some(ast::Wrap::Result(err)) => match self.peek(&err.name).cloned() {
                Some(Entity::Enum(e)) => self.intern(Type::Result { ok: inner, err: e }),
                _ => {
                    self.error(SC3, span, format!("`{}` ist kein Enum fuer `!E`", err.name));
                    inner
                }
            },
        }
    }

    /// Range `lo..hi` (3.4): die Obergrenze bestimmt die Einheit, die
    /// Untergrenze erbt sie („die einzige Ausnahme von 3.6"); beide Grenzen
    /// sind Konstanten. Ergebnis ist die Range mit dem Typ, den sie festlegt.
    pub fn range(&mut self, r: &ast::Range, base: TypeId) -> Option<(Range, TypeId)> {
        let hi = self.expr(&r.to, Some(base))?;
        // Eine Einheit an der Obergrenze gilt fuer den ganzen Typ, auch wenn
        // der Basistyp einheitenlos geschrieben ist (`float in 0..100 bar`).
        let effective = if self.same_base(hi.ty, base) { base } else { self.unify_range_type(base, hi.ty, r.span)? };
        let hi = self.coerce(hi, effective)?;
        let saved = std::mem::replace(&mut self.in_range_bound, true);
        let lo = self.expr(&r.from, Some(effective));
        self.in_range_bound = saved;
        let lo = self.coerce(lo?, effective)?;
        let lo = self.fold(lo)?;
        let hi = self.fold(hi)?;
        let lo_c = const_of(&lo)?;
        let hi_c = const_of(&hi)?;
        if !const_le(&lo_c, &hi_c) {
            self.error(SC3, r.span, "Untergrenze groesser als Obergrenze");
            return None;
        }
        Some((Range { lo: lo_c, hi: hi_c, origin: RangeOrigin::Declared }, effective))
    }

    /// Basistyp und Typ der Obergrenze zusammenfuehren: nur eine Einheit darf
    /// hinzukommen, alles andere ist ein Fehler (3.4).
    fn unify_range_type(&mut self, base: TypeId, bound: TypeId, span: Span) -> Option<TypeId> {
        let base_unit = self.unit_of_type(base);
        let bound_unit = self.unit_of_type(bound);
        match (self.ty(base).clone(), self.ty(bound).clone()) {
            (Type::Float { width: a, .. }, Type::Float { width: b, .. })
                if a == b && base_unit.is_some_and(|u| u.is_one()) && bound_unit.is_some() =>
            {
                Some(self.base(bound))
            }
            _ => {
                let (x, y) = (self.type_name(base), self.type_name(bound));
                self.error_hint(
                    SC3,
                    span,
                    format!("Range-Grenze vom Typ `{y}` passt nicht zu `{x}`"),
                    "beide Grenzen tragen die Einheit der Obergrenze (3.4)",
                );
                None
            }
        }
    }

    /// Typ mit anderer Range.
    pub fn with_range(&mut self, base: TypeId, range: Option<Range>) -> TypeId {
        let t = match self.ty(base).clone() {
            Type::Int { width, unit, .. } => Type::Int { width, unit, range },
            Type::Float { width, unit, .. } => Type::Float { width, unit, range },
            Type::Duration { .. } => Type::Duration { range },
            other => other,
        };
        self.intern(t)
    }

    /// Range eines Typs.
    pub fn range_of(&self, ty: TypeId) -> Option<Range> {
        match self.ty(ty) {
            Type::Int { range, .. } | Type::Float { range, .. } | Type::Duration { range } => *range,
            _ => None,
        }
    }

    /// Typ ohne Range (Basis fuer Vergleiche).
    pub fn base(&mut self, ty: TypeId) -> TypeId {
        if self.range_of(ty).is_some() { self.with_range(ty, None) } else { ty }
    }

    /// Gleicher Typ modulo Range?
    pub fn same_base(&mut self, a: TypeId, b: TypeId) -> bool {
        if a == b {
            return true;
        }
        let (a, b) = (self.base(a), self.base(b));
        if a == b {
            return true;
        }
        match (self.ty(a).clone(), self.ty(b).clone()) {
            (Type::Array { elem: x, len: n }, Type::Array { elem: y, len: m }) => n == m && self.same_base(x, y),
            (Type::Optional(x), Type::Optional(y)) => self.same_base(x, y),
            (Type::Vec { elem: x, cap: n }, Type::Vec { elem: y, cap: m }) => n == m && self.same_base(x, y),
            (Type::Samples { elem: x, len: n }, Type::Samples { elem: y, len: m }) => n == m && self.same_base(x, y),
            (Type::Str { cap: n }, Type::Str { cap: m }) => n == m,
            _ => false,
        }
    }

    /// Typname in Quellschreibweise.
    pub fn type_name(&self, ty: TypeId) -> String {
        let unit_name = |u: &Option<UnitId>| match u {
            Some(id) => format!("[{}]", self.program.units[id.index()].name),
            None => String::new(),
        };
        let range_name = |r: &Option<Range>| match r {
            Some(r) => format!(" in {}..{}", const_name(&r.lo), const_name(&r.hi)),
            None => String::new(),
        };
        match self.ty(ty) {
            Type::Bool => "bool".into(),
            Type::Int { width, unit, range } => {
                let w = match width {
                    IntWidth::I64 => "int".to_string(),
                    other => takt_interp::arith::name(*other).to_string(),
                };
                format!("{w}{}{}", unit_name(unit), range_name(range))
            }
            Type::Float { width, unit, range } => {
                let w = if *width == self.float_width() {
                    "float"
                } else if *width == FloatWidth::F32 {
                    "f32"
                } else {
                    "f64"
                };
                format!("{w}{}{}", unit_name(unit), range_name(range))
            }
            Type::Duration { range } => format!("Duration{}", range_name(range)),
            Type::Enum(e) => self.program.enums[e.index()].name.clone(),
            Type::Record(r) => self.program.records[r.index()].name.clone(),
            Type::Array { elem, len } => format!("[{len}] {}", self.type_name(*elem)),
            Type::Bytes { cap } => format!("bytes<{cap}>"),
            Type::Vec { elem, cap } => format!("vec<{}, {cap}>", self.type_name(*elem)),
            Type::Str { cap } => format!("str<{cap}>"),
            Type::Line { cap } => format!("line<{cap}>"),
            Type::Samples { elem, len } => format!("samples<{}, {len}>", self.type_name(*elem)),
            Type::Table { key, value } => format!("table<{}, {}>", self.type_name(*key), self.type_name(*value)),
            Type::Mat { rows, cols, .. } => format!("mat<{rows}, {cols}>"),
            Type::Map { key, value, cap } => {
                format!("map<{}, {}, {cap}>", self.type_name(*key), self.type_name(*value))
            }
            Type::Optional(t) => format!("{}?", self.type_name(*t)),
            Type::Result { ok, err } => format!("{}!{}", self.type_name(*ok), self.program.enums[err.index()].name),
            Type::Stream(e) => format!("stream<{}>", self.type_name(*e)),
            Type::Capture { elem, len } => format!("capture<{}, {len}>", self.type_name(*elem)),
            Type::Handle(_) => "Handle".into(),
        }
    }

    /// Ganzzahltyp?
    pub fn is_int(&self, ty: TypeId) -> bool {
        matches!(self.ty(ty), Type::Int { .. })
    }

    /// Fliesskommatyp?
    pub fn is_float(&self, ty: TypeId) -> bool {
        matches!(self.ty(ty), Type::Float { .. })
    }

    /// Zahl (Ganzzahl oder Fliesskomma)?
    pub fn is_numeric(&self, ty: TypeId) -> bool {
        self.is_int(ty) || self.is_float(ty)
    }

    /// Dauer?
    pub fn is_duration(&self, ty: TypeId) -> bool {
        matches!(self.ty(ty), Type::Duration { .. })
    }

    /// Bool?
    pub fn is_bool(&self, ty: TypeId) -> bool {
        ty == self.tys.bool
    }

    /// Typ ohne Einheit (fuer Skalare in Produkten).
    pub fn without_unit(&mut self, ty: TypeId) -> TypeId {
        match self.ty(ty).clone() {
            Type::Float { width, .. } => self.intern(Type::Float { width, unit: None, range: None }),
            Type::Int { width, .. } => self.intern(Type::Int { width, unit: None, range: None }),
            _ => ty,
        }
    }
}

/// Vergleicht zwei Konstanten gleicher Art.
fn const_le(a: &Const, b: &Const) -> bool {
    match (a, b) {
        (Const::Int(x), Const::Int(y)) => x <= y,
        (Const::Float(x), Const::Float(y)) => x <= y,
        (Const::Duration(x), Const::Duration(y)) => x <= y,
        _ => true,
    }
}

/// Konstante eines Literals.
pub fn const_of(e: &Expr) -> Option<Const> {
    match &e.kind {
        ExprKind::Int(i) => Some(Const::Int(*i)),
        ExprKind::Float(f) => Some(Const::Float(*f)),
        ExprKind::Duration(d) => Some(Const::Duration(*d)),
        ExprKind::Bool(b) => Some(Const::Bool(*b)),
        _ => None,
    }
}

fn const_name(c: &Const) -> String {
    match c {
        Const::Int(i) => i.to_string(),
        Const::Float(f) => format!("{f}"),
        Const::Duration(d) => takt_mir::dump::duration(*d),
        Const::Bool(b) => b.to_string(),
    }
}
