//! Generics und Vorlagen (plan/m1.md 1.2, 3.9): generische Pruefung gegen
//! eine Programmkopie, sequentielles Loesen der Einheitenvariablen aus den
//! Argumenten, Instanziierung mit Memo.

use takt_diag::{Span, Stage};
use takt_mir::expr::{Expr, ExprKind};
use takt_mir::fns::{BlockDef, Fn, GenericArgVal, GenericOrigin};
use takt_mir::machine::{VarDef, VarScope};
use takt_mir::stmt::Block;
use takt_mir::types::{RecordDef, Type};
use takt_mir::*;
use takt_syntax::ast;

use super::{Binding, BlockKind, Env, FnCtx, GenericVar, Lowerer, Memo, SC3};
use crate::symbols::Entity;
use crate::units::{Atom, Unit};

/// Code der Generics-Regeln.
pub const SC52: &str = "SC-52";

impl Lowerer<'_> {
    /// Generische Variablen einer Vorlage: Einheiten (v1) und Konstanten
    /// mit Range (v1.1); Typvariablen sind v1.2.
    pub fn generic_names(&mut self, gvars: &[ast::GenericVar]) -> Option<Vec<GenericVar>> {
        let mut out: Vec<GenericVar> = Vec::new();
        for g in gvars {
            let (name, var) = match g {
                ast::GenericVar::Unit(name) => (name, GenericVar::Unit(name.name.clone())),
                ast::GenericVar::Type { name, .. } => {
                    self.stage(name.span, "Typvariablen", Stage::V1_2);
                    return None;
                }
                ast::GenericVar::Const { name, range } => {
                    let (lo, hi) = match range {
                        Some(r) => (self.const_int(&r.from)?, self.const_int(&r.to)?),
                        None => (i64::MIN, i64::MAX),
                    };
                    if lo > hi {
                        self.error(SC52, name.span, "Untergrenze groesser als Obergrenze");
                        return None;
                    }
                    (name, GenericVar::Const { name: name.name.clone(), lo, hi })
                }
            };
            if out.iter().any(|v| v.name() == name.name) {
                self.error(SC52, name.span, format!("Variable `{}` doppelt", name.name));
                return None;
            }
            out.push(var);
        }
        Some(out)
    }

    /// Fuehrt `f` mit gebundener generischer Umgebung und sauberem Kontext
    /// aus: keine Maschine, keine Fakten, und als Sichtbereich die Ebene
    /// der Vorlage — das Prelude fuer eine Vorlage der Bibliothek, sonst
    /// die Datei.
    pub fn with_env<T>(&mut self, env: Env, prelude: bool, f: impl FnOnce(&mut Self) -> T) -> T {
        let saved_env = std::mem::replace(&mut self.env, env);
        let saved_prelude = std::mem::replace(&mut self.prelude, prelude);
        let saved_m = self.mctx.take();
        let saved_facts = std::mem::take(&mut self.facts);
        let keep = if prelude { 1 } else { 2 };
        let inner = self.scopes.detach_inner(keep);
        let r = f(self);
        self.scopes.attach_inner(keep, inner);
        self.facts = saved_facts;
        self.mctx = saved_m;
        self.prelude = saved_prelude;
        self.env = saved_env;
        r
    }

    /// Diagnosen aus dem Rumpf einer Bibliotheksvorlage, die Nutzercode
    /// instanziiert hat: Warnungen gehen den Nutzer nichts an, ein Fehler
    /// ist einer der Bibliothek und steht an der Aufrufstelle.
    fn library_diags(&mut self, prelude: bool, from: usize, name: &str, span: Span) {
        if !prelude || self.prelude {
            return;
        }
        let first = self.diags.drain(from..).find(|d| d.is_error()).map(|d| d.message);
        if let Some(msg) = first {
            self.error(
                SC3,
                span,
                format!("interner Fehler: `{name}` aus der Standardbibliothek uebersetzt nicht (`{msg}`)"),
            );
        }
    }

    /// Fuehrt `f` gegen eine Kopie des Programms aus und verwirft sie;
    /// Diagnosen bleiben (generische Pruefung).
    pub fn in_scratch(&mut self, f: impl FnOnce(&mut Self)) {
        let program = self.program.clone();
        let units = self.units.clone_shallow();
        let memo = self.memo.clone();
        let records = self.block_records.clone();
        let state_enums = self.state_enums.clone();
        self.checking += 1;
        f(self);
        self.checking -= 1;
        self.program = program;
        self.units = units;
        self.memo = memo;
        self.block_records = records;
        self.state_enums = state_enums;
    }

    /// Prueft alle Vorlagen generisch (offene Einheitenvariablen).
    pub fn check_templates(&mut self) {
        for i in 0..self.templates.fns.len() {
            let t = self.templates.fns[i].clone();
            if t.prelude != self.prelude {
                continue;
            }
            let env = Env::open(&t.generics);
            self.in_scratch(|this| {
                this.with_env(env, t.prelude, |this| {
                    let name = format!("{}[?]", t.decl.name.name);
                    this.instantiate_fn(&t.decl, name);
                });
            });
        }
        for i in 0..self.templates.blocks.len() {
            let t = self.templates.blocks[i].clone();
            if t.prelude != self.prelude {
                continue;
            }
            let env = Env::open(&t.generics);
            self.in_scratch(|this| {
                this.with_env(env, t.prelude, |this| {
                    let name = format!("{}[?]", t.decl.name.name);
                    this.instantiate_block(&t.decl, name);
                });
            });
        }
        for i in 0..self.templates.machines.len() {
            let t = self.templates.machines[i].clone();
            if t.prelude != self.prelude {
                continue;
            }
            self.in_scratch(|this| {
                this.with_env(Env::default(), t.prelude, |this| {
                    this.check_machine_template(&t);
                });
            });
        }
    }

    /// Aufruf einer generischen Funktion: Einheiten loesen, Instanz holen, Argumente pruefen.
    pub fn call_fn_template(
        &mut self,
        idx: usize,
        generics: &[ast::GenericArg],
        args: &[ast::Arg],
        span: Span,
    ) -> Option<Expr> {
        let t = self.templates.fns[idx].clone();
        let bound = self.solve_call(&t.generics, &t.decl.params, generics, args, &t.decl.name.name, span)?;
        let key = self.memo_key("fn", &t.decl.name.name, &bound);
        let id = match self.memo.get(&key) {
            Some(Memo::Fn(id)) => *id,
            _ => {
                let env = Env::bound(&t.generics, &bound);
                let name = self.instance_name(&t.decl.name.name, &bound);
                let before = self.diags.len();
                let id = self.with_env(env, t.prelude, |this| this.instantiate_fn(&t.decl, name));
                self.library_diags(t.prelude, before, &t.decl.name.name, span);
                let id = id?;
                self.memo.insert(key, Memo::Fn(id));
                id
            }
        };
        let f = &self.program.fns[id.index()];
        let params: Vec<(String, TypeId, Option<Expr>)> =
            f.params.iter().map(|p| (p.name.clone(), p.ty, p.default.clone())).collect();
        let ret = f.ret.unwrap_or(self.tys.bool);
        let args = self.args(&params, args, span)?;
        Some(Expr::new(ExprKind::Call { callee: id, args }, ret, span))
    }

    /// Instanz einer generischen Blockvorlage.
    pub fn instantiate_block_template(
        &mut self,
        idx: usize,
        generics: &[ast::GenericArg],
        args: &[ast::Arg],
        span: Span,
    ) -> Option<BlockId> {
        let t = self.templates.blocks[idx].clone();
        let bound = self.solve_call(&t.generics, &t.decl.params, generics, args, &t.decl.name.name, span)?;
        let key = self.memo_key("block", &t.decl.name.name, &bound);
        if let Some(Memo::Block(id)) = self.memo.get(&key) {
            return Some(*id);
        }
        let env = Env::bound(&t.generics, &bound);
        let name = self.instance_name(&t.decl.name.name, &bound);
        let before = self.diags.len();
        let id = self.with_env(env, t.prelude, |this| this.instantiate_block(&t.decl, name));
        self.library_diags(t.prelude, before, &t.decl.name.name, span);
        let id = id?;
        self.memo.insert(key, Memo::Block(id));
        Some(id)
    }

    fn memo_key(&self, kind: &str, name: &str, bound: &[Binding]) -> String {
        format!("{kind}:{name}[{}]", self.binding_names(bound).join(","))
    }

    fn instance_name(&self, name: &str, bound: &[Binding]) -> String {
        format!("{name}[{}]", self.binding_names(bound).join(", "))
    }

    fn binding_names(&self, bound: &[Binding]) -> Vec<String> {
        bound
            .iter()
            .map(|b| match b {
                Binding::Unit(u) => self.units.display(&self.program, u),
                Binding::Const(n) => n.to_string(),
            })
            .collect()
    }

    /// Sequentielles Loesen (3.12, Festlegung 1): explizite Argumente zuerst,
    /// dann Konstanten aus den Kapazitaeten der Argumente, dann jeder
    /// Parameter mit genau einer offenen Einheitenvariablen (Exponent ±1).
    fn solve_call(
        &mut self,
        vars: &[GenericVar],
        params: &[ast::Param],
        explicit: &[ast::GenericArg],
        args: &[ast::Arg],
        fname: &str,
        span: Span,
    ) -> Option<Vec<Binding>> {
        let mut bound: Vec<Option<Binding>> = vec![None; vars.len()];
        if explicit.len() > vars.len() {
            self.error(SC3, span, format!("`{fname}` hat {} generische Variablen", vars.len()));
            return None;
        }
        for (i, g) in explicit.iter().enumerate() {
            bound[i] = Some(match (&vars[i], g) {
                (GenericVar::Unit(_), ast::GenericArg::Unit(u)) => Binding::Unit(self.unit_expr(u)?),
                (GenericVar::Const { .. }, ast::GenericArg::Const(e)) => Binding::Const(self.const_int(e)?),
                // `[SIZE]` liest der Parser als Einheit; an einer
                // Konstantenvariablen ist es der Name einer Konstanten.
                (GenericVar::Const { .. }, ast::GenericArg::Unit(u))
                    if u.rest.is_empty()
                        && u.first.exponent.is_none()
                        && u.first.name.as_ref().is_some_and(|n| n.name.starts_with(char::is_uppercase)) =>
                {
                    let name = u.first.name.clone().expect("Name");
                    let e =
                        ast::Expr { kind: ast::ExprKind::Upper { name: name.clone(), args: None }, span: name.span };
                    Binding::Const(self.const_int(&e)?)
                }
                (_, ast::GenericArg::Type(t)) => {
                    self.stage(t.span, "Typargumente", Stage::V1_2);
                    return None;
                }
                (var, _) => {
                    let what = match var {
                        GenericVar::Unit(_) => "eine Einheit",
                        GenericVar::Const { .. } => "eine Konstante",
                    };
                    self.error(SC52, span, format!("`{}` von `{fname}` verlangt {what} (3.12)", var.name()));
                    return None;
                }
            });
        }
        // Muster der Parameter mit offenen Variablen
        let saved = std::mem::replace(&mut self.env, Env::open(vars));
        let mut patterns: Vec<Option<Unit>> = Vec::new();
        let mut const_patterns: Vec<Option<u32>> = Vec::new();
        for p in params {
            patterns.push(self.type_unit_pattern(&p.ty));
            const_patterns.push(self.type_const_pattern(&p.ty));
        }
        self.env = saved;
        // Einheiten und Kapazitaeten der Argumente (positional oder benannt)
        let mut arg_units: Vec<Option<Unit>> = vec![None; params.len()];
        let mut arg_caps: Vec<Option<i64>> = vec![None; params.len()];
        let start = self.diags.len();
        for (i, a) in args.iter().enumerate() {
            let idx = match &a.name {
                None => i,
                Some(n) => match params.iter().position(|p| p.name.name == n.name) {
                    Some(idx) => idx,
                    None => continue,
                },
            };
            if idx >= params.len() {
                continue;
            }
            if matches!(a.value.kind, ast::ExprKind::Number { unit: None, .. }) {
                continue;
            }
            if let Some(e) = self.expr(&a.value, None) {
                arg_units[idx] = self.unit_of_type(e.ty);
                arg_caps[idx] = self.capacity_of(e.ty);
            }
        }
        self.diags.truncate(start);
        for (i, var) in const_patterns.iter().enumerate() {
            if let (Some(v), Some(cap)) = (var, arg_caps[i]) {
                if bound[*v as usize].is_none() {
                    bound[*v as usize] = Some(Binding::Const(cap));
                }
            }
        }
        let mut progress = true;
        while progress {
            progress = false;
            let units: Vec<Option<Unit>> =
                bound.iter().map(|b| if let Some(Binding::Unit(u)) = b { Some(u.clone()) } else { None }).collect();
            for (i, pattern) in patterns.iter().enumerate() {
                let (Some(pattern), Some(arg)) = (pattern, &arg_units[i]) else { continue };
                let current = pattern.substitute(&units);
                let open: Vec<(u32, i8)> = current
                    .factors
                    .iter()
                    .filter_map(|(a, e)| if let Atom::Var(v) = a { Some((*v, *e)) } else { None })
                    .collect();
                if let [(v, e)] = open.as_slice() {
                    if e.abs() == 1 {
                        let rest = Unit {
                            factors: current
                                .factors
                                .iter()
                                .filter(|(a, _)| !matches!(a, Atom::Var(_)))
                                .cloned()
                                .collect(),
                            overflow: current.overflow,
                        };
                        let solved = if *e == 1 { arg.div(&rest) } else { rest.div(arg) };
                        bound[*v as usize] = Some(Binding::Unit(solved));
                        progress = true;
                        break;
                    }
                }
            }
        }
        let mut out = Vec::new();
        for (i, b) in bound.into_iter().enumerate() {
            match (b, &vars[i]) {
                (Some(Binding::Const(n)), GenericVar::Const { name, lo, hi }) => {
                    // Pruefung 52: die Range der Konstantenvariablen.
                    if n < *lo || n > *hi {
                        self.error(SC52, span, format!("`{name} = {n}` von `{fname}` ausserhalb {lo}..{hi} (3.12)"));
                        return None;
                    }
                    out.push(Binding::Const(n));
                }
                (Some(b), _) => out.push(b),
                (None, var) => {
                    self.error_hint(
                        SC3,
                        span,
                        format!("Variable `{}` von `{fname}` ist nicht aus den Argumenten ableitbar", var.name()),
                        format!(
                            "explizit instanziieren: `{fname}[{}](…)` (3.12)",
                            vars.iter().map(|n| format!("<{}>", n.name())).collect::<Vec<_>>().join(", ")
                        ),
                    );
                    return None;
                }
            }
        }
        Some(out)
    }

    /// Konstantenvariable in einem Kapazitaetstyp (`bytes<N>`, `vec<T, N>`,
    /// `[N] T`, `line<N>`, `samples<T, N>`): ihr Index.
    fn type_const_pattern(&self, t: &ast::Type) -> Option<u32> {
        let e = match &t.kind {
            ast::TypeKind::Bytes(e) | ast::TypeKind::Line(e) => e,
            ast::TypeKind::Vec { len, .. } | ast::TypeKind::Samples { len, .. } | ast::TypeKind::Array { len, .. } => {
                len
            }
            _ => return None,
        };
        let ast::ExprKind::Upper { name, args: None } = &e.kind else { return None };
        let i = self.env.index(&name.name)?;
        matches!(self.env.vars[i as usize], GenericVar::Const { .. }).then_some(i)
    }

    /// Kapazitaet eines Puffertyps (fuer die Bindung einer Konstantenvariablen).
    fn capacity_of(&self, ty: TypeId) -> Option<i64> {
        Some(match self.ty(ty) {
            Type::Bytes { cap } | Type::Vec { cap, .. } | Type::Line { cap } => i64::from(*cap),
            Type::Array { len, .. } | Type::Samples { len, .. } => i64::from(*len),
            _ => return None,
        })
    }

    /// Einheit eines Parametertyps als Muster (nur numerische Skalare).
    fn type_unit_pattern(&mut self, t: &ast::Type) -> Option<Unit> {
        match &t.kind {
            ast::TypeKind::Scalar { scalar: ast::ScalarType::Float { unit: Some(u), .. }, .. }
            | ast::TypeKind::Scalar { scalar: ast::ScalarType::Int { unit: Some(u), .. }, .. } => {
                let start = self.diags.len();
                let r = self.unit_expr(u);
                self.diags.truncate(start);
                r
            }
            ast::TypeKind::Scalar { scalar: ast::ScalarType::Float { unit: None, .. }, .. } => Some(Unit::one()),
            _ => None,
        }
    }

    /// Lowert eine Funktionsvorlage unter der aktuellen Umgebung.
    pub fn instantiate_fn(&mut self, decl: &ast::FnDecl, name: String) -> Option<FnId> {
        let params = self.params(&decl.params)?;
        let ret = match &decl.ret {
            Some(t) => Some(self.resolve_type(t)?),
            None => params.iter().find(|p| p.inout).map(|p| p.ty),
        };
        if ret.is_none() {
            self.error(SC3, decl.span, "Funktion ohne Rueckgabetyp");
            return None;
        }
        let id = FnId(self.program.fns.len() as u32);
        let origin = GenericOrigin { template: decl.name.name.clone(), args: self.generic_args() };
        self.program.fns.push(Fn {
            name,
            params: params.clone(),
            ret,
            locals: Vec::new(),
            body: Block::default(),
            cost: None,
            stack: None,
            origin: Some(origin),
            span: decl.span,
        });
        let (locals, body) = self.lower_body(&params, ret, &decl.body, 0, None, decl.span)?;
        let f = &mut self.program.fns[id.index()];
        f.locals = locals;
        f.body = body;
        Some(id)
    }

    /// Argumente der aktuellen Instanziierung als `GenericArgVal`.
    fn generic_args(&mut self) -> Vec<GenericArgVal> {
        let env = self.env.clone();
        env.vars
            .iter()
            .enumerate()
            .map(|(i, v)| match v {
                GenericVar::Const { .. } => GenericArgVal::Const(env.consts[i].unwrap_or(0)),
                GenericVar::Unit(_) => {
                    let id = env.units[i]
                        .as_ref()
                        .filter(|u| !u.has_vars())
                        .and_then(|u| self.units.intern(&mut self.program, u).ok());
                    GenericArgVal::Unit(id.unwrap_or(UnitId(0)))
                }
            })
            .collect()
    }

    /// Lowert eine Blockvorlage unter der aktuellen Umgebung.
    pub fn instantiate_block(&mut self, decl: &ast::BlockDecl, name: String) -> Option<BlockId> {
        let id = BlockId(self.program.blocks.len() as u32);
        let origin = GenericOrigin { template: decl.name.name.clone(), args: self.generic_args() };
        self.program.blocks.push(BlockDef {
            name: name.clone(),
            params: Vec::new(),
            state_vars: Vec::new(),
            step: None,
            methods: Vec::new(),
            origin: Some(origin),
            span: decl.span,
        });
        let record = RecordId(self.program.records.len() as u32);
        self.program.records.push(RecordDef {
            name: format!("block:{name}"),
            fields: Vec::new(),
            layout: None,
            builtin: true,
            span: decl.span,
            wire_size: None,
        });
        self.block_records.insert(id, record);
        let before = self.diags.iter().filter(|d| d.is_error()).count();
        self.block_body(decl, id, &name);
        let after = self.diags.iter().filter(|d| d.is_error()).count();
        if after > before && self.checking == 0 {
            return None;
        }
        Some(id)
    }

    /// Generische Pruefung einer Maschinenvorlage mit Platzhalter-Channels.
    fn check_machine_template(&mut self, t: &super::MachineTemplate) {
        let mut bindings = std::collections::HashMap::new();
        for p in &t.decl.params {
            let Some(ty) = self.resolve_type(&p.ty) else { return };
            match p.dir {
                Some(dir) => {
                    let dir = match dir {
                        ast::Direction::Input => takt_mir::program::Direction::Input,
                        ast::Direction::Output => takt_mir::program::Direction::Output,
                    };
                    let id = ChannelId(self.program.channels.len() as u32);
                    self.program.channels.push(takt_mir::program::Channel {
                        dir,
                        name: format!("{}.{}", t.decl.name.name, p.name.name),
                        ty,
                        binding: takt_mir::program::Binding::None,
                        attrs: takt_mir::program::ChannelAttrs::default(),
                        meta: takt_mir::program::Meta::default(),
                        owner: None,
                        span: p.span,
                    });
                    bindings.insert(p.name.name.clone(), super::machine::Bound::Channel(id));
                }
                None => {
                    let init = match &p.default {
                        Some(d) => self.check(d, ty),
                        None => Some(Expr::new(ExprKind::Default, ty, p.span)),
                    };
                    let Some(init) = init else { return };
                    bindings.insert(p.name.name.clone(), super::machine::Bound::Value(init));
                }
            }
        }
        let id = MachineId(self.program.machines.len() as u32);
        self.program.machines.push(takt_mir::machine::Machine::new(format!("{}[?]", t.decl.name.name)));
        self.state_enums.insert(id, t.state_enum);
        self.lower_machine(&t.decl, id, takt_mir::machine::MachineKind::Template, &bindings, None);
    }

    /// Variablen einer Vorlage als Parameter der Instanz.
    pub fn param_var(name: &str, ty: TypeId, init: Expr, span: Span) -> VarDef {
        VarDef { name: name.to_string(), ty, init: Some(init), scope: VarScope::Param, public: false, span }
    }

    /// Nur fuer Aufrufer, die einen Rahmen ohne Rumpf brauchen.
    pub fn empty_fn_ctx(base: u32) -> FnCtx {
        FnCtx { locals: Vec::new(), base, ret: None, block: None }
    }

    /// Blocktyp aus dem Zustandsrecord.
    pub fn block_of_type(&self, ty: TypeId) -> Option<BlockId> {
        let Type::Record(r) = self.ty(ty) else { return None };
        self.block_records.iter().find(|(_, rec)| **rec == *r).map(|(b, _)| *b)
    }

    /// Blockkontext fuer Aufrufer ausserhalb.
    pub const BLOCK_KIND: BlockKind = BlockKind::Fn;
}

impl Entity {
    /// Vorlagenindex, falls die Entitaet eine ist.
    pub fn template_index(&self) -> Option<usize> {
        match self {
            Entity::FnTemplate(i) | Entity::BlockTemplate(i) | Entity::MachineTemplate(i) => Some(*i),
            _ => None,
        }
    }
}
