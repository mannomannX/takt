//! Phasen 1 und 3 (plan/m1.md 3.2): Tabellen aus Deklarationen, dann die
//! Ruempfe; das Prelude durchlaeuft dieselben Phasen zuerst.

use takt_diag::Stage;
use takt_mir::expr::{Builtin, Intrinsic};
use takt_mir::types::Type;
use takt_syntax::ast;

use crate::lower::{BlockTemplate, FnTemplate, Lowerer, SC2, SC3};
use crate::symbols::Entity;

impl Lowerer<'_> {
    /// Eingebaute Bezeichner und Primitive im Dateibereich.
    pub fn declare_builtins(&mut self) {
        let prelude = self.prelude;
        self.prelude = true;
        for (name, b) in [
            ("now", Builtin::Now),
            ("tick", Builtin::Tick),
            ("time_in_state", Builtin::TimeInState),
            ("last_fault", Builtin::LastFault),
            ("event", Builtin::Event),
        ] {
            let ident = ast::Ident { name: name.into(), span: Default::default() };
            self.declare(&ident, Entity::Builtin(b));
        }
        for op in Intrinsic::ALL {
            let ident = ast::Ident { name: op.name().into(), span: Default::default() };
            self.declare(&ident, Entity::Intrinsic(op));
        }
        self.prelude = prelude;
    }

    /// Phase 1: Tabellen ohne Ruempfe.
    pub fn collect(&mut self, file: &ast::File) {
        for item in &file.items {
            match item {
                ast::Item::Enum(e) => self.register_enum(e),
                ast::Item::Record(r) => self.register_record(r),
                _ => {}
            }
        }
        for item in &file.items {
            match item {
                ast::Item::Import(ast::Import::Module { span, .. }) => {
                    self.error_hint(
                        SC2,
                        *span,
                        "Module werden noch nicht unterstuetzt",
                        "Deklarationen in eine Datei legen (M7)",
                    );
                }
                ast::Item::Import(ast::Import::Channels { span, .. }) => {
                    self.stage(*span, "`import channels`", Stage::V1_1)
                }
                ast::Item::System(_) => {}
                ast::Item::Type(t) => self.type_alias(t),
                ast::Item::Unitvec(u) => self.stage(u.span, "`unitvec`", Stage::V1_1),
                ast::Item::Enum(e) => self.enum_body(e),
                ast::Item::Record(r) => self.record_body(r),
                ast::Item::Unit(u) => self.unit_decl(u),
                ast::Item::Stream(s) => self.stream_decl(s),
                ast::Item::Port(p) => self.stage(p.span, "`port`", Stage::V1_2),
                ast::Item::Node(n) => self.node_decl(n),
                ast::Item::Property(_) => {}
                ast::Item::Const(c) => self.const_decl(c),
                ast::Item::Param(p) => self.param_decl(p),
                ast::Item::Profile(_) => {}
                ast::Item::Channel(c) => self.channel_decl(c),
                ast::Item::Command(c) => self.command_decl(c),
                ast::Item::Fn(f) => {
                    if f.generics.is_empty() {
                        self.register_fn(f);
                    } else if let Some(generics) = self.generic_names(&f.generics) {
                        let idx = self.templates.fns.len();
                        self.templates.fns.push(FnTemplate { decl: f.clone(), generics, prelude: self.prelude });
                        self.declare(&f.name, Entity::FnTemplate(idx));
                    }
                }
                ast::Item::Native(n) => self.native_decl(n),
                ast::Item::Block(b) => {
                    if b.generics.is_empty() {
                        self.register_block(b);
                    } else if let Some(generics) = self.generic_names(&b.generics) {
                        let idx = self.templates.blocks.len();
                        self.templates.blocks.push(BlockTemplate { decl: b.clone(), generics, prelude: self.prelude });
                        self.declare(&b.name, Entity::BlockTemplate(idx));
                    }
                }
                ast::Item::Machine(m) => self.register_machine(m),
                ast::Item::Instance(_) | ast::Item::Scenario(_) | ast::Item::Campaign(_) | ast::Item::Trigger(_) => {}
            }
        }
        // Byteplan der `layout`-Records (3.7, Pruefung 46). Als Nachlauf,
        // weil ein verschachtelter Record die Groesse des inneren braucht;
        // die Deklarationsreihenfolge loest das auf.
        for i in 0..self.program.records.len() {
            self.wire_layout(takt_mir::RecordId(i as u32));
        }
        if self.prelude {
            self.builtins_from_prelude();
        }
    }

    /// Eingebaute Typen aus dem Prelude: `FaultKind`, `LastFault`.
    fn builtins_from_prelude(&mut self) {
        if let Some(Entity::Enum(e)) = self.peek("FaultKind").cloned() {
            self.tys.fault_kind = e;
        }
        if let Some(Entity::Record(r)) = self.peek("LastFault").cloned() {
            self.tys.last_fault = r;
            self.tys.last_fault_ty = self.intern(Type::Record(r));
        }
    }

    /// Phase 3: Ruempfe, Instanzen, Profile, Eigenschaften, Kampagnen.
    pub fn lower_bodies(&mut self, file: &ast::File) {
        for item in &file.items {
            match item {
                ast::Item::Fn(f) if f.generics.is_empty() => {
                    if let Some(Entity::Fn(id)) = self.peek(&f.name.name).cloned() {
                        self.fn_body(f, id);
                    }
                }
                ast::Item::Block(b) if b.generics.is_empty() => {
                    if let Some(Entity::Block(id)) = self.peek(&b.name.name).cloned() {
                        let name = b.name.name.clone();
                        self.block_body(b, id, &name);
                    }
                }
                _ => {}
            }
        }
        self.check_templates();
        for item in &file.items {
            match item {
                ast::Item::Machine(m) if m.params.is_empty() => {
                    if let Some(Entity::Machine(id)) = self.peek(&m.name.name).cloned() {
                        self.lower_machine(m, id, takt_mir::machine::MachineKind::Regular, &Default::default(), None);
                    }
                }
                ast::Item::Machine(m) => {
                    if let Some(Entity::MachineTemplate(idx)) = self.peek(&m.name.name).cloned() {
                        if let Some(id) = self.templates.machines[idx].id {
                            let params = self.template_params(m);
                            self.program.machines[id.index()].params = params;
                        }
                    }
                }
                _ => {}
            }
        }
        for item in &file.items {
            match item {
                ast::Item::Instance(i) => self.instance_decl(i),
                ast::Item::Scenario(s) => self.scenario_decl(s),
                ast::Item::Profile(p) => self.profile_decl(p),
                ast::Item::Property(p) => {
                    let what = match p.kind {
                        ast::PropertyKind::Property => "`property`",
                        ast::PropertyKind::Assumption => "`assumption`",
                    };
                    self.stage(p.span, what, Stage::V1_1);
                }
                ast::Item::Trigger(t) => self.stage(t.span, "`trigger`", Stage::V1_2),
                _ => {}
            }
        }
        // Kampagnen zuletzt: Sie nennen Profile und Parameter (13.7).
        for item in &file.items {
            if let ast::Item::Campaign(c) = item {
                self.campaign_decl(c);
            }
        }
        let _ = SC3;
    }
}
