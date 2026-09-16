//! Maschinen (plan/m1.md 3.8): Zustandsbaum, Variablen, Uebergaenge,
//! Sequenz-Oberflaeche mit gehobenen Variablen, Vorlagen und Instanzen,
//! Szenarien, Zustandstyp je Maschine.

use std::collections::HashMap;

use takt_diag::{Diagnostic, Span, Stage};
use takt_mir::expr::{Expr, ExprKind};
use takt_mir::fns::FnParam;
use takt_mir::machine::*;
use takt_mir::stmt::{Block, Stmt, StmtKind};
use takt_mir::types::{Const, EnumDef, IntWidth, Range, RangeOrigin, Type, VariantDef};
use takt_mir::*;
use takt_syntax::ast;

use super::stmt::SC14;
use super::{BlockKind, Lowerer, MachineCtx, MachineTemplate, SC2, SC3, SC8};
use crate::symbols::Entity;

/// Gebundener Parameter einer Instanz.
#[derive(Clone, Debug)]
pub enum Bound {
    /// Channel-Parameter.
    Channel(ChannelId),
    /// Wertparameter (Initialwert).
    Value(Expr),
}

impl Lowerer<'_> {
    /// Registriert eine Maschine: Singleton mit Eintrag, Vorlage mit
    /// Parametern; beide mit Zustandstyp und veroeffentlichter Schnittstelle.
    pub fn register_machine(&mut self, decl: &ast::MachineDecl) {
        let state_enum = self.state_enum(&decl.name.name, &decl.body);
        if decl.params.is_empty() {
            let id = MachineId(self.program.machines.len() as u32);
            let mut m = Machine::new(decl.name.name.clone());
            self.declare_interface(&mut m, &decl.body);
            self.program.machines.push(m);
            self.state_enums.insert(id, state_enum);
            self.declare(&decl.name, Entity::Machine(id));
        } else {
            let id = if self.prelude { None } else { Some(self.template_entry(decl, state_enum)) };
            let idx = self.templates.machines.len();
            self.templates.machines.push(MachineTemplate { decl: decl.clone(), id, state_enum, prelude: self.prelude });
            self.declare(&decl.name, Entity::MachineTemplate(idx));
        }
    }

    /// Der Eintrag `MachineKind::Template` einer Vorlage.
    fn template_entry(&mut self, decl: &ast::MachineDecl, state_enum: EnumId) -> MachineId {
        let id = MachineId(self.program.machines.len() as u32);
        let mut m = Machine::new(decl.name.name.clone());
        m.kind = MachineKind::Template;
        m.span = decl.span;
        self.declare_interface(&mut m, &decl.body);
        self.program.machines.push(m);
        self.state_enums.insert(id, state_enum);
        id
    }

    /// Traegt `pub var` und Signale einer Maschine ein, bevor irgendein
    /// Rumpf gesenkt wird.
    ///
    /// Kommunikation zwischen Maschinen laeuft mit Unit-Delay, und
    /// „dadurch ist die Ausfuehrungsreihenfolge im Tick beweisbar
    /// irrelevant" (Grundentscheidung 6, Satz 9.4.1). Waere die
    /// *Deklarations*reihenfolge dennoch massgeblich, waere die Zusage an
    /// einer sichtbaren Stelle durchbrochen: Zwei Maschinen, die einander
    /// lesen, sind nicht beide zuerst deklarierbar — und das ist bei
    /// Gegenkopplung der Normalfall.
    ///
    /// Eingetragen werden Name, Typ und Sichtbarkeit; der Initialwert
    /// bleibt offen, weil er auf andere Maschinen verweisen darf und die
    /// Aufloesung damit zyklisch wuerde. Er entsteht beim Senken des
    /// Rumpfs, das diesen Eintrag vollstaendig ersetzt.
    ///
    /// Die Liste entsteht deckungsgleich zur spaeteren: `VarId` ist der
    /// Index in `machine.vars` (so liest ihn der Interpreter), und eine
    /// Luecke verschoebe jede aufgeloeste Referenz um eins. Das faellt
    /// nicht als Fehler auf, sondern als falscher Wert. Darum bekommt
    /// jede Variable ihren Platz — auch die nicht-oeffentliche und die,
    /// deren Typ hier noch nicht steht; ein `var` ohne Annotation zoege
    /// ihn aus dem Initialwert, und genau der bleibt offen. Ein solcher
    /// Platzhalter ist nur nicht auffindbar, und sein Typfehler wird
    /// gemeldet, wo er hingehoert: beim Senken des Rumpfs.
    fn declare_interface(&mut self, m: &mut Machine, body: &ast::MachineBody) {
        for item in &body.prelude {
            match item {
                ast::MachinePrelude::Var(v) => {
                    let ty = v.ty.as_ref().and_then(|t| {
                        let before = self.diags.len();
                        let ty = self.resolve_type(t);
                        self.diags.truncate(before);
                        ty
                    });
                    m.vars.push(VarDef {
                        name: v.name.name.clone(),
                        ty: ty.unwrap_or(self.tys.bool),
                        init: None,
                        scope: VarScope::Machine,
                        // Ohne Typ ist der Platz reserviert, aber nicht
                        // benutzbar: Ein Zugriff darauf saehe sonst einen
                        // Typ, den das Programm nicht genannt hat.
                        public: v.public && ty.is_some(),
                        span: v.span,
                    });
                }
                ast::MachinePrelude::Signal(name) => {
                    m.signals.push(SignalDef { name: name.name.clone(), span: name.span });
                }
                _ => {}
            }
        }
    }

    /// Zustandstyp `NAME.State` mit allen Zustandsnamen und `FAULTED`.
    fn state_enum(&mut self, machine: &str, body: &ast::MachineBody) -> EnumId {
        let mut names = Vec::new();
        collect_state_names(&body.states, &mut names);
        // Erst filtern, dann zaehlen: mit einem ausdruecklichen `state FAULTED:`
        // (5.3 erlaubt ihn) verschoben sich sonst die Indizes, und die
        // angehaengte Variante `FAULTED` traf einen schon vergebenen Wert.
        let mut variants: Vec<VariantDef> = names
            .iter()
            .filter(|(n, _)| n != "FAULTED")
            .enumerate()
            .map(|(i, (n, span))| VariantDef {
                name: n.clone(),
                discriminant: i as i64,
                fields: Vec::new(),
                span: *span,
            })
            .collect();
        let next = variants.len() as i64;
        variants.push(VariantDef {
            name: "FAULTED".into(),
            discriminant: next,
            fields: Vec::new(),
            span: Span::default(),
        });
        let id = EnumId(self.program.enums.len() as u32);
        self.program.enums.push(EnumDef {
            name: format!("{machine}.State"),
            variants,
            layout: None,
            open: false,
            builtin: true,
            span: body.span,
        });
        id
    }

    /// Lowert eine Maschine in `program.machines[id]`.
    pub fn lower_machine(
        &mut self,
        decl: &ast::MachineDecl,
        id: MachineId,
        kind: MachineKind,
        bindings: &HashMap<String, Bound>,
        template: Option<usize>,
    ) {
        let mut m = Machine::new(self.program.machines[id.index()].name.clone());
        m.kind = kind;
        m.driver = decl.driver;
        m.span = decl.span;
        let (_, meta) = self.attrs(&decl.attrs, self.tys.bool, None, decl.span);
        m.meta = meta;
        m.declared_budget = self.declared_budget(&decl.attrs);
        let tick = self.program.config.tick;
        if let Some(every) = &decl.every {
            if every.ns <= 0 {
                self.error(SC3, every.span, "Periode muss positiv sein");
            } else {
                if every.ns % tick != 0 {
                    let eff = super::stmt::div_ceil(every.ns, tick) * tick;
                    self.warn(
                        SC14,
                        every.span,
                        format!(
                            "Periode {} ist kein Vielfaches des Ticks; wirkt als {}",
                            takt_mir::dump::duration(every.ns),
                            takt_mir::dump::duration(eff)
                        ),
                    );
                }
                // `as u32` wickelte den Quotienten um: `every 10 s` bei
                // `tick = 1 ns` ergab 1410065408 statt 10^10, ohne Diagnose.
                let ticks = super::stmt::div_ceil(every.ns, tick).max(1);
                match u32::try_from(ticks) {
                    Ok(n) => m.period = n,
                    Err(_) => {
                        self.error_hint(
                            SC3,
                            every.span,
                            format!(
                                "Periode {} sind {ticks} Ticks, mehr als ein Aktivierungszaehler fasst",
                                takt_mir::dump::duration(every.ns)
                            ),
                            "Basis-Tick vergroessern oder Periode verkleinern (7.2)",
                        );
                        m.period = 1;
                    }
                }
            }
        }
        if let Some(phase) = &decl.phase {
            let ticks = (phase.ns / tick).max(0);
            match u32::try_from(ticks) {
                Ok(n) => m.phase = n,
                Err(_) => {
                    self.error_hint(
                        SC3,
                        phase.span,
                        format!(
                            "Phase {} sind {ticks} Ticks, mehr als ein Aktivierungszaehler fasst",
                            takt_mir::dump::duration(phase.ns)
                        ),
                        "Basis-Tick vergroessern oder Phase verkleinern (7.2)",
                    );
                    m.phase = 0;
                }
            }
        }
        // 7.2: `follows` nennt Maschinen; die Kanten prueft Pruefung 33.
        for f in &decl.follows {
            match self.lookup(f) {
                Some(Entity::Machine(other)) if other == id => {
                    self.error(SC3, f.span, "eine Maschine folgt nicht sich selbst (7.2)");
                }
                Some(Entity::Machine(other)) => {
                    if !m.follows.contains(&other) {
                        m.follows.push(other);
                    }
                }
                Some(_) => {
                    self.error(SC3, f.span, format!("`follows {}`: keine Maschine (7.2)", f.name));
                }
                None => {}
            }
        }
        if let Some(n) = &decl.node {
            self.stage(n.span, "Knotenplatzierung", Stage::V2);
        }
        let state_enum = self.state_enums[&id];
        // Zustandsnamen sammeln, Baum anlegen
        let mut states: HashMap<String, StateId> = HashMap::new();
        let mut faulted_decl: Option<&ast::StateDecl> = None;
        for s in &decl.body.states {
            if s.name.name == "FAULTED" {
                faulted_decl = Some(s);
            }
        }
        self.add_states(&mut m, &decl.body.states, None, &mut states);
        let ctx = MachineCtx { machine: m, id, state_enum, states, current: None, lift_to: None, template };
        self.mctx = Some(ctx);
        self.scopes.push();
        self.facts.push(Vec::new());
        // Parameter der Vorlage
        for p in &decl.params {
            match bindings.get(&p.name.name) {
                Some(Bound::Channel(c)) => {
                    self.declare(&p.name, Entity::Channel(*c));
                }
                Some(Bound::Value(v)) => {
                    let ty = v.ty;
                    let init = v.clone();
                    let vid = self.new_var(VarDef {
                        name: p.name.name.clone(),
                        ty,
                        init: Some(init),
                        scope: VarScope::Param,
                        public: false,
                        span: p.span,
                    });
                    self.declare(&p.name, Entity::Var(vid, ty));
                }
                None => {
                    self.error(SC3, p.span, format!("Parameter `{}` nicht gebunden", p.name.name));
                }
            }
        }
        // Vorspann: Variablen, Signale, Fault-Ziel
        for item in &decl.body.prelude {
            match item {
                ast::MachinePrelude::Var(v) => {
                    self.machine_var(v, VarScope::Machine);
                }
                ast::MachinePrelude::Persist(p) => {
                    self.persist_var(p);
                }
                ast::MachinePrelude::Signal(name) => {
                    let m = self.mctx.as_mut().expect("Maschine");
                    let sid = SignalId(m.machine.signals.len() as u32);
                    m.machine.signals.push(SignalDef { name: name.name.clone(), span: name.span });
                    self.declare(name, Entity::Signal(sid));
                }
                ast::MachinePrelude::Fault(target) => {
                    if let Some(t) = self.fault_target(target) {
                        self.mctx.as_mut().expect("Maschine").machine.fault_target = t;
                    }
                }
            }
        }
        // initial
        let initial_name = &decl.body.initial;
        match self.mctx.as_ref().expect("Maschine").states.get(&initial_name.name).copied() {
            Some(s) => {
                let nested = self.mctx.as_ref().expect("Maschine").machine.states[s.index()].parent.is_some();
                if nested {
                    self.error(
                        SC8,
                        initial_name.span,
                        format!("`initial {}` ist kein Zustand der obersten Ebene", initial_name.name),
                    );
                }
                self.mctx.as_mut().expect("Maschine").machine.initial = s;
            }
            None => {
                self.error(SC8, initial_name.span, format!("unbekannter Zustand `{}`", initial_name.name));
            }
        }
        // maschinenweiter loop, Handler
        if let Some(l) = &decl.body.loop_block {
            let block = self.block(l, BlockKind::Loop);
            self.mctx.as_mut().expect("Maschine").machine.loop_block = block;
        }
        for h in &decl.body.handlers {
            if let Some(handler) = self.handler(h) {
                self.mctx.as_mut().expect("Maschine").machine.states[id.index()].handlers.push(handler);
            }
        }
        // Zustaende
        for s in &decl.body.states {
            if s.name.name == "FAULTED" {
                continue;
            }
            self.state_body(s);
        }
        if let Some(f) = faulted_decl {
            self.faulted_state(f);
        }
        // Timer je Zustand
        {
            let m = self.mctx.as_mut().expect("Maschine");
            m.machine.layout.timers =
                (0..m.machine.states.len()).map(|i| Timer { state: StateId(i as u32), width: IntWidth::U32 }).collect();
        }
        self.facts.pop();
        self.scopes.pop();
        let ctx = self.mctx.take().expect("Maschine");
        let mut machine = ctx.machine;
        machine.name = self.program.machines[id.index()].name.clone();
        self.program.machines[id.index()] = machine;
    }

    fn add_states(
        &mut self,
        m: &mut Machine,
        states: &[ast::StateDecl],
        parent: Option<StateId>,
        map: &mut HashMap<String, StateId>,
    ) {
        for s in states {
            if s.name.name == "FAULTED" && parent.is_none() {
                continue;
            }
            if map.contains_key(&s.name.name) || s.name.name == "FAULTED" {
                self.error_hint(
                    SC8,
                    s.name.span,
                    format!("Zustand `{}` doppelt", s.name.name),
                    "Zustandsnamen sind maschinenweit eindeutig (5.1)",
                );
                continue;
            }
            let mut state = State::new(s.name.name.clone(), parent);
            state.idle = s.idle;
            state.resume = s.resume;
            state.span = s.span;
            let (_, meta) = self.attrs(&s.attrs, self.tys.bool, None, s.span);
            state.meta = meta;
            let id = m.add_state(state);
            map.insert(s.name.name.clone(), id);
            self.add_states(m, &s.body.states, Some(id), map);
        }
    }

    /// Variable der Maschine oder eines Zustands (Initialwert bei Eintritt).
    fn machine_var(&mut self, v: &ast::VarDecl, scope: VarScope) -> Option<VarId> {
        let (ty, init) = self.var_init(v)?;
        let init = self.range_checked(init, ty, v.span);
        if let ExprKind::BlockInit { block, count, .. } = &init.kind {
            let (block, count) = (*block, count.unwrap_or(1));
            let m = self.mctx.as_mut().expect("Maschine");
            let var = VarId(m.machine.vars.len() as u32);
            m.machine.layout.block_instances.push(BlockInstance { var, block, count });
        }
        let id = self.new_var(VarDef {
            name: v.name.name.clone(),
            ty,
            init: Some(init),
            scope,
            public: v.public,
            span: v.span,
        });
        if !self.declare(&v.name, Entity::Var(id, ty)) {
            return None;
        }
        Some(id)
    }

    /// `persist var` (5.9): eine gewoehnliche Maschinenvariable plus
    /// Eintrag in `machine.persist`. Der Default steht in `init`; geladene
    /// Werte ueberschreiben ihn beim Start (9.10).
    fn persist_var(&mut self, p: &ast::PersistDecl) {
        let Some(ty) = self.resolve_type(&p.ty) else { return };
        let Some(value) = self.check(&p.value, ty) else { return };
        let init = self.range_checked(value, ty, p.span);
        let id = self.new_var(VarDef {
            name: p.name.name.clone(),
            ty,
            init: Some(init),
            scope: VarScope::Machine,
            public: false,
            span: p.span,
        });
        if !self.declare(&p.name, Entity::Var(id, ty)) {
            return;
        }
        // Instanznamen tragen ihren Index schon (`cells[0]`), der Scope ist
        // deshalb leer; gescopte Instanzen (5.11) gibt es noch nicht.
        let name = self.mctx.as_ref().expect("Maschine").machine.name.clone();
        let type_hash = takt_mir::persist::type_hash(&self.program, &name, "", &p.name.name, ty);
        let m = self.mctx.as_mut().expect("Maschine");
        m.machine.persist.push(PersistVar { var: id, min_interval: p.min_interval.as_ref().map(|d| d.ns), type_hash });
    }

    /// Rumpf eines Zustands.
    fn state_body(&mut self, decl: &ast::StateDecl) {
        let Some(id) = self.mctx.as_ref().expect("Maschine").states.get(&decl.name.name).copied() else { return };
        let saved_current = self.mctx.as_ref().expect("Maschine").current;
        self.mctx.as_mut().expect("Maschine").current = Some(id);
        self.scopes.push();
        self.facts.push(Vec::new());
        if decl.resume {
            self.stage(decl.span, "`resume`", Stage::V1_2);
        }
        for item in &decl.body.prelude {
            match item {
                ast::StatePrelude::Fault(target) => {
                    if let Some(t) = self.fault_target(target) {
                        if t == FaultTarget::State(id) {
                            self.error(SC8, target.span, "Fault-Ziel darf nicht der Zustand selbst sein (5.3)");
                        } else {
                            self.mctx.as_mut().expect("Maschine").machine.states[id.index()].fault_target = Some(t);
                        }
                    }
                }
                ast::StatePrelude::Var(v) => {
                    self.machine_var(v, VarScope::State(id));
                }
                ast::StatePrelude::Instance(i) => {
                    self.stage(i.span, "gescopte Instanzen", Stage::V1_2);
                }
            }
        }
        if let Some(init) = &decl.body.initial {
            let states = self.mctx.as_ref().expect("Maschine").states.clone();
            match states.get(&init.name).copied() {
                Some(child) => {
                    let wrong = self.mctx.as_ref().expect("Maschine").machine.states[child.index()].parent != Some(id);
                    if wrong {
                        self.error(
                            SC8,
                            init.span,
                            format!("`initial {}` ist kein Kind von `{}`", init.name, decl.name.name),
                        );
                    } else {
                        self.mctx.as_mut().expect("Maschine").machine.states[id.index()].initial = Some(child);
                    }
                }
                None => {
                    self.error(SC8, init.span, format!("unbekannter Zustand `{}`", init.name));
                }
            }
        } else if !decl.body.states.is_empty() {
            self.error_hint(
                SC8,
                decl.span,
                format!("Zustand `{}` hat Kinder, aber kein `initial`", decl.name.name),
                "`initial KIND` vor den Kindzustaenden",
            );
        }
        if let Some(e) = &decl.body.enter {
            let b = self.block(e, BlockKind::EnterExit);
            self.mctx.as_mut().expect("Maschine").machine.states[id.index()].enter = b;
        }
        if let Some(l) = &decl.body.loop_block {
            let b = self.block(l, BlockKind::Loop);
            self.mctx.as_mut().expect("Maschine").machine.states[id.index()].loop_block = b;
        }
        for h in &decl.body.handlers {
            if let Some(handler) = self.handler(h) {
                self.mctx.as_mut().expect("Maschine").machine.states[id.index()].handlers.push(handler);
            }
        }
        if let Some(seq) = &decl.body.sequence {
            if !decl.body.states.is_empty() {
                self.error(SC8, decl.span, "Zustand mit Sequenz und Kindzustaenden (6.2)");
            } else {
                self.seq_timeout = decl.body.sequence_timeout.clone();
                let s = self.sequence(seq, id, decl.span);
                self.seq_timeout = None;
                self.mctx.as_mut().expect("Maschine").machine.states[id.index()].sequence = Some(s);
            }
        }
        for t in &decl.body.transitions {
            if let Some(tr) = self.transition(t) {
                self.mctx.as_mut().expect("Maschine").machine.states[id.index()].transitions.push(tr);
            }
        }
        if let Some(e) = &decl.body.exit {
            let b = self.block(e, BlockKind::EnterExit);
            self.mctx.as_mut().expect("Maschine").machine.states[id.index()].exit = b;
        }
        for child in &decl.body.states {
            self.state_body(child);
        }
        self.facts.pop();
        self.scopes.pop();
        self.mctx.as_mut().expect("Maschine").current = saved_current;
    }

    /// `state FAULTED:` erweitert den impliziten Zustand um Uebergaenge (5.3).
    fn faulted_state(&mut self, decl: &ast::StateDecl) {
        let b = &decl.body;
        if !b.prelude.is_empty()
            || b.enter.is_some()
            || b.loop_block.is_some()
            || b.exit.is_some()
            || b.sequence.is_some()
            || !b.states.is_empty()
            || !b.handlers.is_empty()
        {
            self.error_hint(
                SC8,
                decl.span,
                "`FAULTED` fuehrt keinen Nutzercode aus (5.3)",
                "nur `when …: -> ZIEL` erlaubt",
            );
            return;
        }
        for t in &b.transitions {
            if let Some(tr) = self.transition(t) {
                self.mctx.as_mut().expect("Maschine").machine.faulted.transitions.push(tr);
            }
        }
    }

    /// Uebergang `when g:`/`after d:` mit Aktionen und Ziel.
    fn transition(&mut self, t: &ast::Transition) -> Option<Transition> {
        self.scopes.push();
        self.facts.push(Vec::new());
        let result = (|| {
            let trigger = match &t.trigger {
                ast::Trigger::When(g) => TransTrigger::When(self.guard(g)?),
                ast::Trigger::After(d) => {
                    let dur = self.tys.duration;
                    let e = self.check(d, dur)?;
                    self.check_period_multiple(&e, d.span);
                    TransTrigger::After(e)
                }
            };
            if let TransTrigger::When(Guard::Expr(e)) = &trigger {
                let facts = Self::facts_of(e);
                self.facts.last_mut().expect("Fakten").extend(facts);
            }
            let stmts = self.stmts(&t.actions, BlockKind::Transition);
            let target = self.target(&t.target)?;
            Some(Transition {
                trigger,
                actions: Block { stmts, span: t.span },
                target,
                kind: TransKind::Weak,
                span: t.span,
            })
        })();
        self.facts.pop();
        self.scopes.pop();
        result
    }

    /// Guard: Bool-Ausdruck, Musterguard oder Stream-Guard (5.2, 8.7).
    pub fn guard(&mut self, g: &ast::Guard) -> Option<Guard> {
        match g {
            ast::Guard::Expr(e) => {
                if let ast::ExprKind::Match { subject, kind, pattern, binding } = &e.kind {
                    return self.match_guard(subject, *kind, pattern, binding.as_ref(), e.span);
                }
                Some(Guard::Expr(self.check_bool(e)?))
            }
            ast::Guard::Next { subject, binding } => self.stream_guard(subject, Some(binding)),
        }
    }

    /// Sequenz-Oberflaeche mit gehobenen Variablen (6.2).
    fn sequence(&mut self, items: &[ast::SeqItem], state: StateId, span: Span) -> Sequence {
        let bool = self.tys.bool;
        let done = self.new_var(VarDef {
            name: format!("{}.done", self.mctx.as_ref().expect("Maschine").machine.states[state.index()].name),
            ty: bool,
            init: Some(Expr::new(ExprKind::Bool(false), bool, span)),
            scope: VarScope::Lifted(state),
            public: false,
            span,
        });
        let saved = self.mctx.as_mut().expect("Maschine").lift_to.replace(state);
        let items = self.seq_items(items);
        self.mctx.as_mut().expect("Maschine").lift_to = saved;
        Sequence { items, done, span }
    }

    fn seq_items(&mut self, items: &[ast::SeqItem]) -> Vec<SeqItem> {
        let mut out = Vec::new();
        for item in items {
            if let ast::SeqItem::Stmt(s) = item {
                if let ast::StmtKind::Pulse { output, value, duration } = &s.kind {
                    if let Some(pair) = self.pulse(output, value, duration, s.span) {
                        out.extend(pair.into_iter().map(SeqItem::Stmt));
                    }
                    continue;
                }
            }
            let was = self.in_stmt;
            self.in_stmt = matches!(item, ast::SeqItem::Stmt(_));
            let i = self.seq_item(item);
            self.in_stmt = was;
            out.extend(self.pending.drain(..).map(SeqItem::Stmt));
            if let Some(i) = i {
                out.push(i);
            }
        }
        out
    }

    /// Der `else`-Zweig eines `until` (6.2, Pruefung 6): Er laeuft, *weil*
    /// das Muster nicht getroffen hat — die Bindung des Guards traegt dort
    /// keinen Wert. Fuer die Dauer des Zweigs gilt sie als ungebunden; wer
    /// sie liest, bekommt einen Fehler statt eines stillen Defaults.
    fn else_block(&mut self, guard: &ast::Guard, b: &ast::Block) -> takt_mir::stmt::Block {
        let name = guard_binding(guard);
        if let Some(n) = &name {
            self.unbound.push(n.clone());
        }
        let block = self.block(b, BlockKind::Else);
        if name.is_some() {
            self.unbound.pop();
        }
        block
    }

    fn seq_item(&mut self, item: &ast::SeqItem) -> Option<SeqItem> {
        let dur = self.tys.duration;
        match item {
            ast::SeqItem::Stmt(s) => Some(SeqItem::Stmt(self.stmt(s, BlockKind::Sequence)?)),
            ast::SeqItem::Wait(d) => {
                let e = self.check(d, dur)?;
                self.check_period_multiple(&e, d.span);
                Some(SeqItem::Wait(e))
            }
            ast::SeqItem::Until { guard, timeout, span } => {
                let g = self.guard(guard)?;
                // FB-13: ohne eigenen Timeout gilt der des Segments.
                let timeout = timeout.clone().or_else(|| self.seq_timeout.clone());
                let timeout = match &timeout {
                    None => {
                        self.warn_hint(
                            SC14,
                            *span,
                            "`until` ohne Timeout: unbegrenztes Warten",
                            "`timeout d` anfuegen, wenn kein Bedienereingriff gemeint ist (8.5)",
                        );
                        None
                    }
                    Some(t) => {
                        let d = self.check(&t.duration, dur)?;
                        self.check_period_multiple(&d, t.duration.span);
                        let action = match &t.action {
                            ast::TimeoutAction::Fault => TimeoutAction::Fault,
                            ast::TimeoutAction::Goto(x) => TimeoutAction::Goto(self.target(x)?),
                            ast::TimeoutAction::Else(b) => TimeoutAction::Else(self.else_block(guard, b)),
                        };
                        Some(Timeout { duration: d, action })
                    }
                };
                Some(SeqItem::Until { guard: g, timeout, span: *span })
            }
            ast::SeqItem::Expect { cond, message, span } => {
                let cond = self.check_bool(cond)?;
                let message = match message {
                    Some(m) => Some(self.format(m)?),
                    None => None,
                };
                Some(SeqItem::Expect { cond, message, span: *span })
            }
            ast::SeqItem::Repeat { count, body, span } => {
                let int = self.tys.int;
                let n = self.check(count, int)?;
                let range = match n.kind {
                    ExprKind::Int(k) if k > 0 => {
                        Some(Range { lo: Const::Int(0), hi: Const::Int(k), origin: RangeOrigin::Declared })
                    }
                    ExprKind::Int(_) => {
                        self.error(SC3, count.span, "`repeat` verlangt eine positive Zahl");
                        return None;
                    }
                    _ => None,
                };
                let ty = self.intern(Type::Int { width: IntWidth::I64, unit: None, range });
                let state = self.mctx.as_ref().expect("Maschine").lift_to.expect("Sequenz");
                let counter_name = format!("repeat_{}", self.next_counter());
                let counter = self.new_var(VarDef {
                    name: counter_name,
                    ty,
                    init: Some(Expr::new(ExprKind::Int(0), ty, *span)),
                    scope: VarScope::Lifted(state),
                    public: false,
                    span: *span,
                });
                let body = self.seq_items(body);
                Some(SeqItem::Repeat { count: n, counter, body, span: *span })
            }
            ast::SeqItem::Step { name, body, span } => {
                let body = self.seq_items(body);
                Some(SeqItem::Step { name: name.value.clone(), body, span: *span })
            }
        }
    }

    // ------------------------------------------------------------ Instanzen

    /// `instance NAME[i in a..b] = tmpl(args)`.
    pub fn instance_decl(&mut self, decl: &ast::InstanceDecl) {
        let Some(Entity::MachineTemplate(idx)) = self.lookup(&decl.template) else {
            self.error(SC3, decl.template.span, format!("`{}` ist keine Maschinenvorlage", decl.template.name));
            return;
        };
        if decl.resume {
            self.stage(decl.span, "`resume`", Stage::V1_2);
        }
        let t = self.templates.machines[idx].clone();
        let template = match t.id {
            Some(id) => id,
            None => self.with_env(super::Env::default(), t.prelude, |this| {
                let id = this.template_entry(&t.decl, t.state_enum);
                this.program.machines[id.index()].params = this.template_params(&t.decl);
                this.templates.machines[idx].id = Some(id);
                id
            }),
        };
        let indices: Vec<(i64, i64)> = match &decl.index {
            None => vec![(0, 1)],
            Some((_, range)) => {
                let (Some(lo), Some(hi)) = (self.const_int(&range.from), self.const_int(&range.to)) else { return };
                if lo < 0 || hi <= lo || hi - lo > 1024 {
                    self.error(SC3, range.span, "Indexbereich ausserhalb 0..1024");
                    return;
                }
                (lo..hi).map(|i| (i, hi - lo)).collect()
            }
        };
        let first = MachineId(self.program.machines.len() as u32);
        for (i, len) in &indices {
            let name = match &decl.index {
                None => decl.name.name.clone(),
                Some(_) => format!("{}[{i}]", decl.name.name),
            };
            let id = MachineId(self.program.machines.len() as u32);
            self.program.machines.push(Machine::new(name));
            self.state_enums.insert(id, t.state_enum);
            let bindings = self.scoped(|this| {
                if let Some((var, _)) = &decl.index {
                    let int = this.tys.int;
                    this.declare(var, Entity::Const(Expr::new(ExprKind::Int(*i), int, var.span)));
                }
                this.instance_bindings(&t.decl, &decl.args, decl.span)
            });
            let Some((bindings, args)) = bindings else { continue };
            let kind = MachineKind::Instance(InstanceInfo {
                template,
                args,
                array: decl.index.as_ref().map(|_| ((*i - indices[0].0) as u32, *len as u32)),
            });
            self.with_env(super::Env::default(), t.prelude, |this| {
                this.lower_machine(&t.decl, id, kind, &bindings, Some(idx))
            });
        }
        let entity = match &decl.index {
            None => Entity::Machine(first),
            Some(_) => Entity::MachineArray(first, indices.len() as u32),
        };
        self.declare(&decl.name, entity);
    }

    /// Ein Instanzargument: konstant oder ein `param` — eine Laufkonstante,
    /// die eine Kampagne sweept (13.7); ein `tunable` nicht, die Instanz
    /// wird einmal gebildet (8.4).
    fn run_constant(&mut self, v: Expr) -> Option<Expr> {
        match &v.kind {
            ExprKind::Param(id) if !self.program.params[id.index()].tunable => Some(v),
            ExprKind::Param(_) => {
                self.error(SC3, v.span, "ein `tunable param` ist kein Instanzargument (8.4)");
                None
            }
            _ => self.fold(v),
        }
    }

    /// Bindet Argumente an die Parameter einer Vorlage.
    fn instance_bindings(
        &mut self,
        tmpl: &ast::MachineDecl,
        args: &[ast::Arg],
        span: Span,
    ) -> Option<(HashMap<String, Bound>, Vec<Expr>)> {
        let mut bindings = HashMap::new();
        let mut values = Vec::new();
        let mut used = vec![false; tmpl.params.len()];
        for (i, a) in args.iter().enumerate() {
            let idx = match &a.name {
                None => i,
                Some(n) => match tmpl.params.iter().position(|p| p.name.name == n.name) {
                    Some(idx) => idx,
                    None => {
                        self.error(SC3, n.span, format!("unbekannter Parameter `{}`", n.name));
                        return None;
                    }
                },
            };
            let Some(p) = tmpl.params.get(idx) else {
                self.error(SC3, a.span, "zu viele Argumente");
                return None;
            };
            if used[idx] {
                self.error(SC3, a.span, format!("Argument `{}` doppelt", p.name.name));
                return None;
            }
            used[idx] = true;
            let ty = self.resolve_type(&p.ty)?;
            match p.dir {
                Some(dir) => {
                    let ast::ExprKind::Ident(cname) = &a.value.kind else {
                        self.error(SC3, a.value.span, format!("Parameter `{}` verlangt einen Channel", p.name.name));
                        return None;
                    };
                    let Some(Entity::Channel(c)) = self.lookup(cname) else {
                        self.error(SC3, a.value.span, format!("`{}` ist kein Channel", cname.name));
                        return None;
                    };
                    let ch = &self.program.channels[c.index()];
                    let want = match dir {
                        ast::Direction::Input => takt_mir::program::Direction::Input,
                        ast::Direction::Output => takt_mir::program::Direction::Output,
                    };
                    if ch.dir != want {
                        self.error(
                            SC3,
                            a.value.span,
                            format!("`{}` hat die falsche Richtung fuer `{}`", cname.name, p.name.name),
                        );
                        return None;
                    }
                    let cty = ch.ty;
                    if !self.same_base(cty, ty) {
                        let (x, y) = (self.type_name(cty), self.type_name(ty));
                        self.error(
                            SC3,
                            a.value.span,
                            format!(
                                "Channel `{}` hat Typ `{x}`, Parameter `{}` verlangt `{y}`",
                                cname.name, p.name.name
                            ),
                        );
                        return None;
                    }
                    bindings.insert(p.name.name.clone(), Bound::Channel(c));
                    values.push(Expr::new(ExprKind::Input { channel: c, dominated: true }, cty, a.span));
                }
                None => {
                    let v = self.check(&a.value, ty)?;
                    let v = self.run_constant(v)?;
                    values.push(v.clone());
                    bindings.insert(p.name.name.clone(), Bound::Value(v));
                }
            }
        }
        for (i, p) in tmpl.params.iter().enumerate() {
            if used[i] {
                continue;
            }
            match (&p.default, p.dir) {
                (Some(d), None) => {
                    let ty = self.resolve_type(&p.ty)?;
                    let v = self.check(d, ty)?;
                    values.push(v.clone());
                    bindings.insert(p.name.name.clone(), Bound::Value(v));
                }
                _ => {
                    self.error(SC3, span, format!("Argument `{}` fehlt", p.name.name));
                    return None;
                }
            }
        }
        Some((bindings, values))
    }

    /// `scenario "name" [every d]:` als Maschine (13.6).
    pub fn scenario_decl(&mut self, decl: &ast::ScenarioDecl) {
        let id = MachineId(self.program.machines.len() as u32);
        let name = decl.name.value.clone();
        self.program.machines.push(Machine::new(name.clone()));
        let state_enum = self.state_enum(&name, &decl.body);
        self.state_enums.insert(id, state_enum);
        let machine = ast::MachineDecl {
            driver: false,
            name: ast::Ident { name: name.clone(), span: decl.name.span },
            params: Vec::new(),
            follows: Vec::new(),
            node: None,
            every: decl.every.clone(),
            phase: None,
            attrs: Vec::new(),
            body: decl.body.clone(),
            span: decl.span,
        };
        self.lower_machine(&machine, id, MachineKind::Scenario, &HashMap::new(), None);
    }

    /// Parameter der Vorlage als `FnParam` (Signatur des Template-Eintrags).
    pub fn template_params(&mut self, decl: &ast::MachineDecl) -> Vec<FnParam> {
        let mut out = Vec::new();
        for p in &decl.params {
            let Some(ty) = self.resolve_type(&p.ty) else { continue };
            out.push(FnParam { name: p.name.name.clone(), ty, inout: false, default: None, span: p.span });
        }
        out
    }
}

fn collect_state_names(states: &[ast::StateDecl], out: &mut Vec<(String, Span)>) {
    for s in states {
        out.push((s.name.name.clone(), s.name.span));
        collect_state_names(&s.body.states, out);
    }
}

/// Erzeugt eine Diagnose fuer doppelte Namen in Maschinen.
pub fn duplicate(name: &str, span: Span) -> Diagnostic {
    Diagnostic::error(SC2, span, format!("`{name}` ist schon definiert"))
}

/// Sichtbarkeit von Anweisungen fuer Aufrufer ausserhalb.
pub fn stmt_kind_name(s: &Stmt) -> &'static str {
    match &s.kind {
        StmtKind::Assign { .. } => "Zuweisung",
        StmtKind::Check { .. } => "check",
        _ => "Anweisung",
    }
}

/// Der Name, den ein Guard bindet (8.7).
fn guard_binding(g: &ast::Guard) -> Option<String> {
    match g {
        ast::Guard::Next { binding, .. } => Some(binding.name.clone()),
        ast::Guard::Expr(e) => match &e.kind {
            ast::ExprKind::Match { binding: Some(b), .. } => Some(b.name.clone()),
            _ => None,
        },
    }
}
