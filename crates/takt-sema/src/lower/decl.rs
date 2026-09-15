//! Deklarationen auf Dateiebene (plan/m1.md 3.2): Konfiguration, Typen,
//! Einheiten, Konstanten, Parameter, Profile, Channels, Commands, Streams,
//! Funktionen, Natives, Bloecke.

use takt_diag::{Diagnostic, Span, Stage};
use takt_mir::expr::{Expr, ExprKind};
use takt_mir::fns::{BlockDef, CostVec, Fn, FnParam, Native, NativeKind};
use takt_mir::machine::{VarDef, VarScope};
use takt_mir::pattern::Address;
use takt_mir::program::*;
use takt_mir::stmt::{Block, Stmt, StmtKind};
use takt_mir::types::*;
use takt_mir::*;
use takt_syntax::ast;
use takt_syntax::subtext::address_text;

use super::{BlockKind, FnCtx, Lowerer, SC2, SC3};
use crate::symbols::Entity;
use crate::units::{Unit, rational_from_text};

/// Code der Single-Writer- und Bindungsregeln.
pub const SC7: &str = "SC-7";

/// Konfiguration aus dem `system:`-Block; Default `tick = 1 ms`.
pub fn config_from(file: &ast::File, edition: u32, diags: &mut Vec<Diagnostic>) -> Config {
    let mut config = Config::new(edition, 1_000_000);
    let mut seen_tick = false;
    for item in &file.items {
        let ast::Item::System(sys) = item else { continue };
        for entry in &sys.items {
            match entry {
                ast::SystemItem::Tick(d) => {
                    if d.ns <= 0 {
                        diags.push(Diagnostic::error(SC3, d.span, "`tick` muss positiv sein"));
                    } else {
                        config.tick = d.ns;
                        seen_tick = true;
                    }
                }
                ast::SystemItem::OutputTiming(t) => {
                    config.output_timing = match t {
                        ast::OutputTiming::Asap => OutputTiming::Asap,
                        ast::OutputTiming::Boundary => OutputTiming::Boundary,
                    }
                }
                ast::SystemItem::FaultIsFail(b) => config.fault_is_fail = *b,
                ast::SystemItem::TickSource(addr) => match address_text(&addr.value) {
                    Ok(segments) => config.tick_source = Some(to_address(segments)),
                    Err(d) => diags.push(shift(d, addr.span)),
                },
                ast::SystemItem::TickTolerance { value, ticks } => {
                    let pct = match &value.kind {
                        ast::ExprKind::Number { value: ast::Number::Int(i), .. } => {
                            i.text.replace('_', "").parse::<f64>().ok()
                        }
                        ast::ExprKind::Number { value: ast::Number::Float(f), .. } => {
                            f.text.replace('_', "").parse::<f64>().ok()
                        }
                        _ => None,
                    };
                    match pct {
                        Some(pct) => {
                            let ticks =
                                ticks.as_ref().and_then(|t| t.text.replace('_', "").parse::<u32>().ok()).unwrap_or(10);
                            config.tick_tolerance = Some(TickTolerance { pct, ticks });
                        }
                        None => diags.push(Diagnostic::error(
                            SC3,
                            value.span,
                            "`tick_tolerance` verlangt eine Zahl in `pct`",
                        )),
                    }
                }
                ast::SystemItem::Target(t) => config.target = Some(t.name.clone()),
                ast::SystemItem::Float(w) => {
                    config.float_width = match w {
                        ast::FloatWidth::F32 => FloatWidth::F32,
                        ast::FloatWidth::F64 => FloatWidth::F64,
                    }
                }
                ast::SystemItem::Language(_) => {}
            }
        }
    }
    let _ = seen_tick;
    config
}

fn to_address(segments: Vec<takt_syntax::subtext::AddressSegment>) -> Address {
    Address {
        segments: segments
            .into_iter()
            .map(|s| takt_mir::pattern::AddressSegment {
                name: s.name,
                range: s.range.and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?))),
            })
            .collect(),
    }
}

fn shift(mut d: Diagnostic, lit: Span) -> Diagnostic {
    d.span = Span { file: lit.file, start: lit.start + 1 + d.span.start, end: lit.start + 1 + d.span.end };
    d.code = SC3;
    d
}

impl Lowerer<'_> {
    // ------------------------------------------------------------ Typen

    /// `type Name = T`
    pub fn type_alias(&mut self, decl: &ast::TypeDecl) {
        if let Some(ty) = self.resolve_type(&decl.ty) {
            self.declare(&decl.name, Entity::Type(ty));
        }
    }

    /// Registriert Enum und Record mit Namen, damit Felder vorwaerts verweisen.
    pub fn register_enum(&mut self, decl: &ast::EnumDecl) {
        let id = EnumId(self.program.enums.len() as u32);
        self.program.enums.push(EnumDef {
            name: decl.name.name.clone(),
            variants: Vec::new(),
            layout: decl.layout.map(int_width),
            open: decl.open,
            builtin: self.prelude,
            span: decl.span,
        });
        self.declare(&decl.name, Entity::Enum(id));
    }

    /// Registriert einen Record.
    pub fn register_record(&mut self, decl: &ast::RecordDecl) {
        let id = RecordId(self.program.records.len() as u32);
        self.program.records.push(RecordDef {
            name: decl.name.name.clone(),
            fields: Vec::new(),
            layout: decl.layout.as_ref().map(|l| WireLayout {
                endian: match l.endian {
                    ast::Endian::Little => Endian::Little,
                    ast::Endian::Big => Endian::Big,
                },
                align: l.align.as_ref().and_then(|a| a.text.replace('_', "").parse().ok()),
            }),
            builtin: self.prelude,
            span: decl.span,
            wire_size: None,
        });
        self.declare(&decl.name, Entity::Record(id));
    }

    /// Fuellt die Varianten eines registrierten Enums.
    pub fn enum_body(&mut self, decl: &ast::EnumDecl) {
        let Some(Entity::Enum(id)) = self.peek(&decl.name.name).cloned() else { return };
        let mut next: i64 = 0;
        let mut variants = Vec::new();
        for v in &decl.variants {
            let discriminant = match &v.discriminant {
                Some(d) => match super::expr::parse_int(&d.text).and_then(|x| i64::try_from(x).ok()) {
                    Some(x) => x,
                    None => {
                        // `as i64` schnitt vorher stillschweigend ab, und die
                        // Fortzaehlung ab `i64::MAX` brach den Compiler ab.
                        self.error(SC3, d.span, "Diskriminante nicht als i64 darstellbar");
                        continue;
                    }
                },
                None => next,
            };
            next = match discriminant.checked_add(1) {
                Some(n) => n,
                None => {
                    self.error(SC3, v.name.span, "Diskriminante laeuft ueber `i64::MAX` hinaus");
                    continue;
                }
            };
            if variants.iter().any(|x: &VariantDef| x.name == v.name.name) {
                self.error(SC2, v.name.span, format!("Variante `{}` doppelt", v.name.name));
                continue;
            }
            let fields = self.fields(&v.fields);
            variants.push(VariantDef { name: v.name.name.clone(), discriminant, fields, span: v.span });
        }
        self.program.enums[id.index()].variants = variants;
    }

    /// Fuellt die Felder eines registrierten Records.
    pub fn record_body(&mut self, decl: &ast::RecordDecl) {
        let Some(Entity::Record(id)) = self.peek(&decl.name.name).cloned() else { return };
        let mut fields = Vec::new();
        for f in &decl.fields {
            match f {
                ast::RecordField::Plain(field) => {
                    let mut lowered = self.fields_after(&fields, std::slice::from_ref(field));
                    fields.append(&mut lowered);
                }
                ast::RecordField::Bits { name, ty, bits, span } => {
                    let width = int_width(*ty);
                    let carrier = self.intern(Type::Int { width, unit: None, range: None });
                    let mut out = Vec::new();
                    for b in bits {
                        let bty = match &b.ty {
                            ast::BitType::Bool => self.tys.bool,
                            ast::BitType::Int(w) => {
                                self.intern(Type::Int { width: int_width(*w), unit: None, range: None })
                            }
                        };
                        let lo = b.from.text.replace('_', "").parse::<u8>().ok();
                        let hi = match &b.to {
                            Some(t) => t.text.replace('_', "").parse::<u8>().ok(),
                            None => lo,
                        };
                        let (Some(lo), Some(hi)) = (lo, hi) else {
                            self.error(SC3, b.span, "Bitposition nicht darstellbar");
                            continue;
                        };
                        if hi < lo || u32::from(hi) >= width.bits() {
                            self.error("SC-46", b.span, format!("Bitfeld {lo}..{hi} ausserhalb des Traegerfelds"));
                            continue;
                        }
                        out.push(BitfieldDef { name: b.name.name.clone(), ty: bty, lo, hi, span: b.span });
                    }
                    fields.push(FieldDef {
                        name: name.name.clone(),
                        ty: carrier,
                        const_value: None,
                        offset: None,
                        len_field: None,
                        bits: out,
                        span: *span,
                    });
                }
            }
        }
        let mut seen = std::collections::HashSet::new();
        for f in &fields {
            if f.name != "_" && !seen.insert(f.name.clone()) {
                self.error(SC2, f.span, format!("Feld `{}` doppelt", f.name));
            }
        }
        self.program.records[id.index()].fields = fields;
    }

    fn fields(&mut self, fields: &[ast::Field]) -> Vec<FieldDef> {
        self.fields_after(&[], fields)
    }

    /// Wie `fields`, aber mit den schon gesenkten Feldern als Kontext: nur so
    /// findet `len = feld` sein Ziel (3.7, Pruefung 37).
    fn fields_after(&mut self, before: &[FieldDef], fields: &[ast::Field]) -> Vec<FieldDef> {
        let mut out = Vec::new();
        for f in fields {
            let Some(ty) = self.resolve_type(&f.ty) else { continue };
            let const_value = match &f.value {
                Some(v) => {
                    let e = self.check(v, ty).and_then(|e| self.fold(e));
                    match e {
                        Some(e) => super::types::const_of(&e),
                        None => None,
                    }
                }
                None => None,
            };
            let offset = f.offset.as_ref().and_then(|o| o.text.replace('_', "").parse().ok());
            let known: Vec<FieldDef> = before.iter().chain(out.iter()).cloned().collect();
            let len_field = f.len_field.as_ref().and_then(|lf| self.len_field_index(&known, lf, f.span));
            out.push(FieldDef {
                name: f.name.name.clone(),
                ty,
                const_value,
                offset,
                len_field,
                bits: Vec::new(),
                span: f.span,
            });
        }
        out
    }

    /// Pruefung 37 (3.7): `len = len_field` verweist auf ein *vorangehendes*
    /// Integer-Feld. Ein Verweis nach vorn waere beim Dekodieren nicht
    /// lesbar, ein Verweis auf ein Nicht-Integer nicht als Laenge deutbar.
    fn len_field_index(&mut self, done: &[FieldDef], name: &ast::Ident, span: Span) -> Option<u32> {
        let Some(i) = done.iter().position(|x| x.name == name.name) else {
            self.error_hint(
                crate::checks::SC37,
                name.span,
                format!("`len_field` nennt kein vorangehendes Feld: `{}`", name.name),
                "das Laengenfeld steht vor dem Feld, dessen Laenge es traegt (3.7)",
            );
            return None;
        };
        if !matches!(self.ty(done[i].ty), Type::Int { .. }) {
            let n = self.type_name(done[i].ty);
            self.error_hint(
                crate::checks::SC37,
                name.span,
                format!("`len_field` verweist auf `{}` vom Typ `{n}`", name.name),
                "ein Laengenfeld ist ganzzahlig (3.7)",
            );
            return None;
        }
        let _ = span;
        Some(i as u32)
    }

    // ------------------------------------------------------------ Einheiten

    /// `unit psi = 6894.76 Pa`, `unit degC = affine(K, 273.15)`, `unit raw = 1`.
    pub fn unit_decl(&mut self, decl: &ast::UnitDecl) {
        let (name, span) = match decl {
            ast::UnitDecl::Scaled { name, span, .. } | ast::UnitDecl::Affine { name, span, .. } => (name, *span),
        };
        if self.units.get(&name.name).is_some() {
            self.error(SC2, name.span, format!("Einheit `{}` ist schon definiert (3.2)", name.name));
            return;
        }
        let result = match decl {
            ast::UnitDecl::Scaled { factor, unit, .. } => {
                let text = match factor {
                    ast::Number::Int(i) => i.text.clone(),
                    ast::Number::Float(f) => f.text.clone(),
                };
                let Some(scale) = rational_from_text(&text) else {
                    self.error(SC3, span, "Faktor nicht exakt darstellbar");
                    return;
                };
                // 3.2 verlangt einen rationalen Skalierungsfaktor. Mit null
                // verlor `.to(...)` in der einen Richtung stillschweigend den
                // Wert und faultete in der anderen mit `NonFinite`.
                if scale.num == 0 {
                    self.error_hint(
                        SC3,
                        span,
                        format!("Einheit `{}` haette den Faktor null (3.2)", name.name),
                        "einen Faktor ungleich null angeben",
                    );
                    return;
                }
                let base = match unit {
                    Some(u) => match self.unit_expr(u) {
                        Some(b) => b,
                        None => return,
                    },
                    None => Unit::one(),
                };
                let Some(base_factor) = self.units.factor(&self.program, &base) else {
                    self.error(SC3, span, "Faktor nicht darstellbar");
                    return;
                };
                let Some(factor) = crate::units::rat_mul(scale, base_factor) else {
                    self.error(SC3, span, "Faktor nicht darstellbar");
                    return;
                };
                let dim = self.units.dimension(&self.program, &base);
                let prefixable = self.prelude && !self.units.is_affine(&self.program, &base);
                self.units.declare(&mut self.program, &name.name, dim, factor, None, prefixable, self.prelude, span)
            }
            ast::UnitDecl::Affine { base, offset, .. } => {
                let Some(b) = self.unit_expr(base) else { return };
                let text = match offset {
                    ast::Number::Int(i) => i.text.clone(),
                    ast::Number::Float(f) => f.text.clone(),
                };
                let Some(off) = rational_from_text(&text) else {
                    self.error(SC3, span, "Verschiebung nicht exakt darstellbar");
                    return;
                };
                let Some(factor) = self.units.factor(&self.program, &b) else {
                    self.error(SC3, span, "Faktor nicht darstellbar");
                    return;
                };
                let dim = self.units.dimension(&self.program, &b);
                self.units.declare(&mut self.program, &name.name, dim, factor, Some(off), false, self.prelude, span)
            }
        };
        if let Err(msg) = result {
            self.error(SC3, span, msg);
        }
    }

    // ------------------------------------------------------------ Konstanten, Parameter

    /// `const NAME [: T] = e`: gefaltet.
    pub fn const_decl(&mut self, decl: &ast::ConstDecl) {
        let value = match &decl.ty {
            Some(t) => {
                let Some(ty) = self.resolve_type(t) else { return };
                self.check(&decl.value, ty)
            }
            None => self.expr(&decl.value, None),
        };
        let Some(value) = value else { return };
        let Some(folded) = self.fold(value) else { return };
        if matches!(folded.kind, ExprKind::BlockInit { .. }) {
            self.error(SC3, decl.span, "Konstante kann keine Blockinstanz sein");
            return;
        }
        self.declare(&decl.name, Entity::Const(folded));
    }

    /// `param NAME : T = default`, `tunable param …`.
    pub fn param_decl(&mut self, decl: &ast::ParamDecl) {
        let Some(ty) = self.resolve_type(&decl.ty) else { return };
        let Some(default) = self.check(&decl.value, ty) else { return };
        let Some(default) = self.fold(default) else { return };
        if decl.tunable {
            self.stage(decl.span, "`tunable param`", Stage::V1_1);
        }
        let (_, meta) = self.attrs(&decl.attrs, ty, None, decl.span);
        let id = ParamId(self.program.params.len() as u32);
        self.program.params.push(Param {
            name: decl.name.name.clone(),
            ty,
            default,
            tunable: decl.tunable,
            meta,
            span: decl.span,
        });
        self.declare(&decl.name, Entity::Param(id, ty));
    }

    /// `profile NAME:` mit Belegungen.
    pub fn profile_decl(&mut self, decl: &ast::ProfileDecl) {
        let mut assignments = Vec::new();
        for (name, value) in &decl.entries {
            let Some(Entity::Param(p, ty)) = self.lookup(name) else {
                self.error(SC3, name.span, format!("`{}` ist kein Parameter", name.name));
                continue;
            };
            let Some(v) = self.check(value, ty) else { continue };
            let Some(v) = self.fold(v) else { continue };
            assignments.push((p, v));
        }
        let id = ProfileId(self.program.profiles.len() as u32);
        self.program.profiles.push(Profile { name: decl.name.name.clone(), assignments, span: decl.span });
        self.declare(&decl.name, Entity::Profile(id));
    }

    // ------------------------------------------------------------ Channels

    /// `input`/`output`.
    pub fn channel_decl(&mut self, decl: &ast::ChannelDecl) {
        let Some(ty) = self.resolve_type(&decl.ty) else { return };
        let dir = match decl.dir {
            ast::Direction::Input => Direction::Input,
            ast::Direction::Output => Direction::Output,
        };
        let binding = match &decl.binding {
            ast::Binding::None => Binding::None,
            ast::Binding::Hw(a) | ast::Binding::Sim(a) => match address_text(&a.value) {
                Ok(segs) => {
                    let addr = to_address(segs);
                    if matches!(decl.binding, ast::Binding::Hw(_)) { Binding::Hw(addr) } else { Binding::Sim(addr) }
                }
                Err(d) => {
                    self.diags.push(shift(d, a.span));
                    return;
                }
            },
        };
        if let Binding::Hw(addr) = &binding {
            if let Some(other) =
                self.program.channels.iter().find(|c| matches!(&c.binding, Binding::Hw(a) if a == addr && c.dir == dir))
            {
                self.diags.push(
                    Diagnostic::error(SC7, decl.span, format!("Adresse schon an `{}` gebunden", other.name))
                        .with_note(other.span, "hier gebunden")
                        .with_suggestion("jede hw-Adresse hoechstens einmal je Richtung"),
                );
                return;
            }
        }
        let (attrs, meta) = self.attrs(&decl.attrs, ty, Some(dir), decl.span);
        let needs_safe =
            dir == Direction::Output && !matches!(binding, Binding::Sim(_)) && !matches!(self.ty(ty), Type::Stream(_));
        if needs_safe && attrs.safe.is_none() {
            self.error_hint(
                SC3,
                decl.span,
                format!("Output `{}` ohne `safe`", decl.name.name),
                "`with safe = …` deklarieren (8.1)",
            );
            return;
        }
        let id = ChannelId(self.program.channels.len() as u32);
        self.program.channels.push(Channel {
            dir,
            name: decl.name.name.clone(),
            ty,
            binding,
            attrs,
            meta,
            owner: None,
            span: decl.span,
        });
        self.declare(&decl.name, Entity::Channel(id));
    }

    /// Attribute nach `with` (8.1, 8.6, 3.5, 2.5).
    pub fn attrs(
        &mut self,
        attrs: &[ast::Attr],
        ty: TypeId,
        dir: Option<Direction>,
        span: Span,
    ) -> (ChannelAttrs, Meta) {
        let mut out = ChannelAttrs::default();
        let mut meta = Meta::default();
        for a in attrs {
            match &a.kind {
                ast::AttrKind::Safe(e) => {
                    if dir != Some(Direction::Output) {
                        self.error(SC3, a.span, "`safe` nur an Outputs");
                        continue;
                    }
                    let elem = match self.ty(ty).clone() {
                        Type::Stream(_) => {
                            self.error(SC3, a.span, "`safe` nicht an Streams");
                            continue;
                        }
                        _ => ty,
                    };
                    if let Some(v) = self.check(e, elem).and_then(|v| self.fold(v)) {
                        out.safe = Some(v);
                    }
                }
                ast::AttrKind::MaxAge(d) => out.max_age = Some(d.ns),
                ast::AttrKind::Rate(e) | ast::AttrKind::MaxRate(e) => {
                    // Raten tragen die Dimension 1/s (8.6, „Elemente pro
                    // Sekunde, Einheit Hz"); jede Einheit dieser Dimension ist
                    // erlaubt, also auch `kHz` und `1/min`.
                    let Some(v) = self.expr(e, None).and_then(|v| self.fold(v)) else { continue };
                    let ok = self
                        .unit_of_type(v.ty)
                        .is_some_and(|u| self.units.dimension(&self.program, &u) == [0, 0, -1, 0, 0, 0, 0]);
                    if !ok {
                        let n = self.type_name(v.ty);
                        self.error_hint(
                            SC3,
                            a.span,
                            format!("Rate hat den Typ `{n}`"),
                            "eine Frequenz erwartet, etwa `2000 Hz` oder `320 kHz` (8.6)",
                        );
                        continue;
                    }
                    if matches!(a.kind, ast::AttrKind::Rate(_)) {
                        out.rate = Some(v);
                    } else {
                        out.max_rate = Some(v);
                    }
                }
                ast::AttrKind::Capacity(n) => out.capacity = self.int_attr(n),
                ast::AttrKind::Framing(f) => {
                    out.framing = Some(match f {
                        ast::Framing::Raw => Framing::Raw,
                        ast::Framing::Lines => Framing::Lines,
                        ast::Framing::Cobs => Framing::Cobs,
                        ast::Framing::LengthPrefixed(w) => Framing::LengthPrefixed(match w.name.as_str() {
                            "u8" => IntWidth::U8,
                            "u32_le" | "u32" => IntWidth::U32,
                            _ => IntWidth::U16,
                        }),
                        ast::Framing::Fixed(n) => Framing::Fixed(self.int_attr(n).unwrap_or(0)),
                    })
                }
                ast::AttrKind::Overflow(o) => {
                    out.overflow = Some(match o {
                        ast::Overflow::Fault => Overflow::Fault,
                        ast::Overflow::DropOldest => Overflow::DropOldest,
                        ast::Overflow::Drop => Overflow::Drop,
                    })
                }
                ast::AttrKind::Wake(b) => out.wake = *b,
                ast::AttrKind::Jitter(d) => out.jitter = Some(d.ns),
                ast::AttrKind::MaxSlew(e) => {
                    if let Some(v) = self.expr(e, None) {
                        out.max_slew = Some(v);
                    }
                }
                ast::AttrKind::Debounce(n) => out.debounce = self.int_attr(n),
                ast::AttrKind::CapacityBytes(n) => out.capacity_bytes = self.int_attr(n),
                ast::AttrKind::ExpectLen(n) => out.expect_len = self.int_attr(n),
                ast::AttrKind::Irreversible => out.irreversible = true,
                ast::AttrKind::Label(s) => meta.label = Some(s.value.clone()),
                ast::AttrKind::Display(u) => {
                    if let Some(unit) = self.unit_expr(u) {
                        meta.display = self.unit_id(&unit, u.span);
                    }
                }
                ast::AttrKind::Group(s) => meta.group = Some(s.value.clone()),
                ast::AttrKind::Doc(s) => meta.doc = Some(s.value.clone()),
                // `budget` steht am Maschinenkopf (7.2); dort liest es
                // `declared_budget`. An einem Channel ist es ein Fehler —
                // `dir` ist genau dann gesetzt (Input oder Output).
                ast::AttrKind::Budget(_) => {
                    if dir.is_some() {
                        self.error(SC3, a.span, "`budget` nur an einer Maschine (7.2)");
                    }
                }
            }
        }
        let _ = span;
        (out, meta)
    }

    /// `with budget = {ram = …, wcet = …}` am Maschinenkopf (7.2).
    ///
    /// **`wcet` wird gelesen, nicht mehr abgelehnt.** Geprueft wird es erst
    /// mit der kalibrierten Kostentabelle (`takt-sema::calibrated`, 13.8):
    /// Ohne sie sind `B_m` und `F_m` Operationszahlen und keine Zeiten. Die
    /// Deklaration festzuhalten kostet nichts und macht sie sichtbar — wer
    /// sie schreibt, hat eine Erwartung, und die gehoert in die MIR, auch
    /// wenn das Urteil auf eine Messung wartet.
    pub fn declared_budget(&mut self, attrs: &[ast::Attr]) -> Option<takt_mir::machine::DeclaredBudget> {
        let mut out: Option<takt_mir::machine::DeclaredBudget> = None;
        for a in attrs {
            let ast::AttrKind::Budget(items) = &a.kind else { continue };
            let mut b = takt_mir::machine::DeclaredBudget { ram: None, wcet_ns: None, span: a.span };
            for it in items {
                match it.kind {
                    ast::BudgetKind::Ram => {
                        if b.ram.is_some() {
                            self.error(SC3, it.span, "`ram` doppelt im Budget");
                            continue;
                        }
                        let Some(v) = self.const_int(&it.value) else { continue };
                        if v <= 0 {
                            self.error(SC3, it.span, "`ram` verlangt eine positive Groesse");
                            continue;
                        }
                        b.ram = u64::try_from(v).ok();
                    }
                    ast::BudgetKind::Wcet => {
                        if b.wcet_ns.is_some() {
                            self.error(SC3, it.span, "`wcet` doppelt im Budget");
                            continue;
                        }
                        let dur = self.tys.duration;
                        let Some(e) = self.check(&it.value, dur) else { continue };
                        let takt_mir::expr::ExprKind::Duration(ns) = e.kind else {
                            self.error(SC3, it.span, "`wcet` verlangt eine konstante Dauer");
                            continue;
                        };
                        if ns <= 0 {
                            self.error(SC3, it.span, "`wcet` verlangt eine positive Dauer");
                            continue;
                        }
                        b.wcet_ns = Some(ns);
                    }
                }
            }
            out = Some(b);
        }
        out
    }

    fn int_attr(&mut self, n: &ast::IntLit) -> Option<u32> {
        match super::expr::parse_int(&n.text) {
            Some(v) if v >= 0 && v <= i128::from(u32::MAX) => Some(v as u32),
            _ => {
                self.error(SC3, n.span, "Attributwert ausserhalb 0..2^32");
                None
            }
        }
    }

    /// `command NAME [with wake = true]`
    pub fn command_decl(&mut self, decl: &ast::CommandDecl) {
        let bool = self.tys.bool;
        let (attrs, meta) = self.attrs(&decl.attrs, bool, None, decl.span);
        let id = CommandId(self.program.commands.len() as u32);
        self.program.commands.push(Command { name: decl.name.name.clone(), wake: attrs.wake, meta, span: decl.span });
        self.declare(&decl.name, Entity::Command(id));
    }

    /// `stream<E> name with …` (8.6, M2 in der Ausfuehrung).
    pub fn stream_decl(&mut self, decl: &ast::StreamDecl) {
        let Some(elem) = self.elem_type(&decl.elem) else { return };
        let (attrs, _) = self.attrs(&decl.attrs, elem, None, decl.span);
        let id = StreamId(self.program.streams.len() as u32);
        self.program.streams.push(Stream {
            name: decl.name.name.clone(),
            elem,
            capacity: attrs.capacity.unwrap_or(16),
            capacity_bytes: attrs.capacity_bytes,
            expect_len: attrs.expect_len,
            overflow: attrs.overflow.unwrap_or_default(),
            writer: None,
            readers: Vec::new(),
            span: decl.span,
        });
        self.declare(&decl.name, Entity::Stream(id));
    }

    /// `node NAME @ hw("…") [with tick = d]` (v2).
    pub fn node_decl(&mut self, decl: &ast::NodeDecl) {
        // 12.9: verteilte Ausfuehrung ist v2. Der Knoten wird trotzdem
        // gesammelt, damit Pruefung 58 ihn sehen kann.
        self.stage(decl.span, "Knoten", Stage::V2);
        let address = match address_text(&decl.address.value) {
            Ok(s) => to_address(s),
            Err(d) => {
                self.diags.push(shift(d, decl.address.span));
                return;
            }
        };
        let id = NodeId(self.program.nodes.len() as u32);
        self.program.nodes.push(Node {
            name: decl.name.name.clone(),
            address,
            tick: decl.tick.as_ref().map(|d| d.ns),
            span: decl.span,
        });
        self.declare(&decl.name, Entity::Node(id));
    }

    // ------------------------------------------------------------ Funktionen

    /// Parameterliste einer Signatur.
    pub fn params(&mut self, params: &[ast::Param]) -> Option<Vec<FnParam>> {
        let mut out = Vec::new();
        let mut ok = true;
        for p in params {
            let Some(ty) = self.resolve_type(&p.ty) else {
                ok = false;
                continue;
            };
            let default = match &p.default {
                Some(d) => match self.check(d, ty).and_then(|e| self.fold(e)) {
                    Some(e) => Some(e),
                    None => {
                        ok = false;
                        None
                    }
                },
                None => None,
            };
            if p.dir.is_some() {
                self.error(SC3, p.span, "Channel-Parameter nur an Maschinen");
                ok = false;
            }
            out.push(FnParam { name: p.name.name.clone(), ty, inout: p.inout, default, span: p.span });
        }
        ok.then_some(out)
    }

    /// Registriert eine nicht generische Funktion mit leerem Rumpf.
    pub fn register_fn(&mut self, decl: &ast::FnDecl) -> Option<FnId> {
        let params = self.params(&decl.params)?;
        let ret = match &decl.ret {
            Some(t) => Some(self.resolve_type(t)?),
            None => {
                let inout: Vec<&FnParam> = params.iter().filter(|p| p.inout).collect();
                match inout.as_slice() {
                    [p] => Some(p.ty),
                    [] => {
                        self.error_hint(
                            SC3,
                            decl.span,
                            "Funktion ohne Rueckgabetyp",
                            "`-> T` oder ein `inout`-Parameter (3.9)",
                        );
                        return None;
                    }
                    _ => {
                        self.error(SC3, decl.span, "hoechstens ein `inout`-Parameter (3.9)");
                        return None;
                    }
                }
            }
        };
        let id = FnId(self.program.fns.len() as u32);
        self.program.fns.push(Fn {
            name: decl.name.name.clone(),
            params,
            ret,
            locals: Vec::new(),
            body: Block::default(),
            cost: None,
            stack: None,
            origin: None,
            span: decl.span,
        });
        self.declare(&decl.name, Entity::Fn(id));
        Some(id)
    }

    /// Lowert einen Funktionsrumpf in `program.fns[id]`.
    pub fn fn_body(&mut self, decl: &ast::FnDecl, id: FnId) {
        let (params, ret) = {
            let f = &self.program.fns[id.index()];
            (f.params.clone(), f.ret)
        };
        let Some((locals, body)) = self.lower_body(&params, ret, &decl.body, 0, None, decl.span) else { return };
        let f = &mut self.program.fns[id.index()];
        f.locals = locals;
        f.body = body;
    }

    /// Rumpf mit Rahmen: Parameter als Lokale ab `base`, Rueckgabe geprueft;
    /// ein `inout`-Parameter wird am Ende zurueckgegeben.
    pub fn lower_body(
        &mut self,
        params: &[FnParam],
        ret: Option<TypeId>,
        body: &ast::Block,
        base: u32,
        block: Option<BlockId>,
        span: Span,
    ) -> Option<(Vec<VarDef>, Block)> {
        let locals: Vec<VarDef> = params
            .iter()
            .map(|p| VarDef {
                name: p.name.clone(),
                ty: p.ty,
                init: None,
                scope: VarScope::Param,
                public: false,
                span: p.span,
            })
            .collect();
        self.fn_ctx.push(FnCtx { locals, base, ret, block });
        let result = self.scoped(|this| {
            for (i, p) in params.iter().enumerate() {
                let ident = ast::Ident { name: p.name.clone(), span: p.span };
                this.declare(&ident, Entity::Var(VarId(base + i as u32), p.ty));
            }
            let before = this.diags.iter().filter(|d| d.is_error()).count();
            let mut stmts = this.stmts(&body.stmts, BlockKind::Fn);
            let body_ok = this.diags.iter().filter(|d| d.is_error()).count() == before;
            if let Some(inout) = params.iter().position(|p| p.inout) {
                let ty = params[inout].ty;
                stmts.push(Stmt::new(
                    StmtKind::Return(Expr::new(ExprKind::Var(VarId(base + inout as u32)), ty, span)),
                    span,
                ));
            } else if ret.is_some() && body_ok && !ends_with_return(&stmts) {
                // Nur bei fehlerfreiem Rumpf: sonst waere es eine Folgemeldung
                // der Anweisung, die schon gemeldet wurde.
                this.error_hint(SC3, span, "nicht jeder Pfad endet mit `return`", "`return` am Ende ergaenzen");
            }
            Block { stmts, span: body.span }
        });
        let ctx = self.fn_ctx.pop().expect("Rahmen");
        Some((ctx.locals, result))
    }

    /// `native fn`/`native job` mit Kostenvertrag (4.5).
    pub fn native_decl(&mut self, decl: &ast::NativeDecl) {
        if !decl.generics.is_empty() {
            self.stage(decl.span, "generische Natives", Stage::V1_1);
            return;
        }
        // 4.5: „v1: nur die kuratierte, mitgelieferte Menge." Eine
        // Funktion kommt hinein, *nachdem* ihre Vektoren gruen sind
        // (13.8) — dieselbe Huerde wie bei `libtaktm`. Eine Deklaration
        // ohne Implementierung waere eine Zusage, die beim Aufruf bricht.
        //
        // Ein Projekt-Native (`from "..."`) ist etwas anderes: Es *soll*
        // nicht in der Menge stehen, sondern eine eigene Implementierung
        // mitbringen. Dafuer gilt die Stufenmeldung unten (v1.1), und sie
        // ist die praezisere Auskunft.
        if decl.from.is_none() && takt_native::Native::by_name(&decl.name.name).is_none() {
            self.error_hint(
                SC3,
                decl.span,
                format!("`{}` gehoert nicht zur kuratierten Menge nativer Funktionen (4.5)", decl.name.name),
                "verfuegbar sind: crc32, crc32c, crc16, sum8 (grammar/takt-native.md);                  Projekt-Natives sind v1.1",
            );
            return;
        }
        let Some(params) = self.params(&decl.params) else { return };
        let Some(ret) = self.resolve_type(&decl.ret) else { return };
        let mut cost = CostVec::default();
        match &decl.cost {
            ast::CostSpec::Single(n) => cost.i32 = super::expr::parse_int(&n.text).unwrap_or(0) as u64,
            ast::CostSpec::Classes(list) => {
                for (class, n) in list {
                    let v = super::expr::parse_int(&n.text).unwrap_or(0) as u64;
                    match class {
                        ast::CostClass::I32 => cost.i32 = v,
                        ast::CostClass::I64 => cost.i64 = v,
                        ast::CostClass::F32 => cost.f32 = v,
                        ast::CostClass::F64 => cost.f64 = v,
                        ast::CostClass::Mem => cost.mem = v,
                        ast::CostClass::Call => cost.call = v,
                        ast::CostClass::Native => cost.native = v,
                    }
                }
            }
        }
        let stack = super::expr::parse_int(&decl.stack.text).unwrap_or(0) as u32;
        if decl.from.is_some() && !self.prelude {
            self.stage(decl.span, "Projekt-Natives", Stage::V1_1);
        }
        let id = NativeId(self.program.natives.len() as u32);
        self.program.natives.push(Native {
            kind: match decl.kind {
                ast::NativeKind::Fn => NativeKind::Fn,
                ast::NativeKind::Job => NativeKind::Job,
            },
            name: decl.name.name.clone(),
            params,
            ret,
            cost,
            stack,
            duration: decl.duration.as_ref().map(|d| d.ns),
            total: true,
            from: decl.from.as_ref().map(|s| s.value.clone()),
            origin: None,
            span: decl.span,
        });
        self.declare(&decl.name, Entity::Native(id));
    }

    // ------------------------------------------------------------ Bloecke

    /// Registriert einen nicht generischen Block mit leerer Definition und
    /// seinem Zustandsrecord (Typ der Instanzen).
    pub fn register_block(&mut self, decl: &ast::BlockDecl) -> Option<BlockId> {
        let id = BlockId(self.program.blocks.len() as u32);
        self.program.blocks.push(BlockDef {
            name: decl.name.name.clone(),
            params: Vec::new(),
            state_vars: Vec::new(),
            step: None,
            methods: Vec::new(),
            origin: None,
            span: decl.span,
        });
        let record = RecordId(self.program.records.len() as u32);
        self.program.records.push(RecordDef {
            name: format!("block:{}", decl.name.name),
            fields: Vec::new(),
            layout: None,
            builtin: true,
            span: decl.span,
            wire_size: None,
        });
        self.block_records.insert(id, record);
        self.declare(&decl.name, Entity::Block(id));
        Some(id)
    }

    /// Lowert Parameter, Zustandsvariablen, `step` und Methoden eines Blocks.
    pub fn block_body(&mut self, decl: &ast::BlockDecl, id: BlockId, name: &str) {
        let Some(params) = self.params(&decl.params) else { return };
        // Zustandsvariablen: Initialwerte sehen Parameter und fruehere Variablen
        let param_vars: Vec<VarDef> = params
            .iter()
            .map(|p| VarDef {
                name: p.name.clone(),
                ty: p.ty,
                init: None,
                scope: VarScope::Param,
                public: false,
                span: p.span,
            })
            .collect();
        self.fn_ctx.push(FnCtx { locals: param_vars, base: 0, ret: None, block: None });
        let state_vars = self.scoped(|this| {
            for (i, p) in params.iter().enumerate() {
                let ident = ast::Ident { name: p.name.clone(), span: p.span };
                this.declare(&ident, Entity::Var(VarId(i as u32), p.ty));
            }
            let mut out = Vec::new();
            for v in &decl.vars {
                let Some((ty, init)) = this.var_init(v) else { continue };
                let init = this.range_checked(init, ty, v.span);
                let id = this.new_var(VarDef {
                    name: v.name.name.clone(),
                    ty,
                    init: Some(init.clone()),
                    scope: VarScope::Local,
                    public: false,
                    span: v.span,
                });
                this.declare(&v.name, Entity::Var(id, ty));
                out.push(VarDef {
                    name: v.name.name.clone(),
                    ty,
                    init: Some(init),
                    scope: VarScope::Local,
                    public: false,
                    span: v.span,
                });
            }
            out
        });
        self.fn_ctx.pop();
        let record = self.block_records[&id];
        self.program.records[record.index()].fields = params
            .iter()
            .map(|p| FieldDef {
                name: p.name.clone(),
                ty: p.ty,
                const_value: None,
                offset: None,
                len_field: None,
                bits: Vec::new(),
                span: p.span,
            })
            .chain(state_vars.iter().map(|v| FieldDef {
                name: v.name.clone(),
                ty: v.ty,
                const_value: None,
                offset: None,
                len_field: None,
                bits: Vec::new(),
                span: v.span,
            }))
            .collect();
        let base = (params.len() + state_vars.len()) as u32;
        {
            let def = &mut self.program.blocks[id.index()];
            def.params = params.clone();
            def.state_vars = state_vars.clone();
        }
        let declare_instance_vars = |this: &mut Self| {
            for (i, p) in params.iter().enumerate() {
                let ident = ast::Ident { name: p.name.clone(), span: p.span };
                this.declare(&ident, Entity::Var(VarId(i as u32), p.ty));
            }
            for (i, v) in state_vars.iter().enumerate() {
                let ident = ast::Ident { name: v.name.clone(), span: v.span };
                this.declare(&ident, Entity::Var(VarId((params.len() + i) as u32), v.ty));
            }
        };
        if let Some(step) = &decl.step {
            let Some(sparams) = self.params(&step.params) else { return };
            let Some(ret) = self.resolve_type(&step.ret) else { return };
            let fid = FnId(self.program.fns.len() as u32);
            self.program.fns.push(Fn {
                name: format!("{name}.step"),
                params: sparams.clone(),
                ret: Some(ret),
                locals: Vec::new(),
                body: Block::default(),
                cost: None,
                stack: None,
                origin: None,
                span: step.span,
            });
            let lowered = self.scoped(|this| {
                declare_instance_vars(this);
                this.lower_body(&sparams, Some(ret), &step.body, base, Some(id), step.span)
            });
            if let Some((locals, body)) = lowered {
                let f = &mut self.program.fns[fid.index()];
                f.locals = locals;
                f.body = body;
            }
            self.program.blocks[id.index()].step = Some(fid);
        }
        for m in &decl.methods {
            let Some(mparams) = self.params(&m.params) else { continue };
            let ret = match &m.ret {
                Some(t) => match self.resolve_type(t) {
                    Some(t) => Some(t),
                    None => continue,
                },
                None => None,
            };
            let fid = FnId(self.program.fns.len() as u32);
            self.program.fns.push(Fn {
                name: format!("{name}.{}", m.name.name),
                params: mparams.clone(),
                ret,
                locals: Vec::new(),
                body: Block::default(),
                cost: None,
                stack: None,
                origin: None,
                span: m.span,
            });
            let lowered = self.scoped(|this| {
                declare_instance_vars(this);
                this.lower_body(&mparams, ret, &m.body, base, Some(id), m.span)
            });
            if let Some((locals, body)) = lowered {
                let f = &mut self.program.fns[fid.index()];
                f.locals = locals;
                f.body = body;
            }
            self.program.blocks[id.index()].methods.push(fid);
        }
    }
}

/// Endet jeder Pfad mit `return`?
pub fn ends_with_return(stmts: &[Stmt]) -> bool {
    match stmts.last().map(|s| &s.kind) {
        Some(StmtKind::Return(_)) => true,
        Some(StmtKind::If { then, otherwise, .. }) => {
            ends_with_return(&then.stmts) && ends_with_return(&otherwise.stmts)
        }
        Some(StmtKind::Match { arms, .. }) => !arms.is_empty() && arms.iter().all(|a| ends_with_return(&a.body.stmts)),
        _ => false,
    }
}

/// Breite eines `int_type`.
pub fn int_width(t: ast::IntType) -> IntWidth {
    match t {
        ast::IntType::Int | ast::IntType::I64 => IntWidth::I64,
        ast::IntType::I8 => IntWidth::I8,
        ast::IntType::I16 => IntWidth::I16,
        ast::IntType::I32 => IntWidth::I32,
        ast::IntType::U8 => IntWidth::U8,
        ast::IntType::U16 => IntWidth::U16,
        ast::IntType::U32 => IntWidth::U32,
        ast::IntType::U64 => IntWidth::U64,
    }
}
