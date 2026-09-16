//! Drucker: eine Funktion je Produktion (`fmt_<produktion>`), die den Baum in
//! Quelltextreihenfolge ablaeuft und die Tokens mit den Leerraumregeln aus
//! `grammar/format.md` in den Emitter schreibt. Die Vorrangkette der
//! Ausdruecke wird ueber den Baum gedruckt (`fmt_expr`), nicht je Stufe.

use super::emit::{Emitter, Kind};
use crate::ast::*;
use crate::token::TokenKind;

impl Emitter<'_, '_> {
    /// Name oder Literal mit Leerzeichen davor.
    fn name(&mut self) {
        self.any(true);
    }

    fn at_newline(&self) -> bool {
        self.at_kind(TokenKind::Newline)
    }

    // ------------------------------------------------------------ Datei

    /// `file`
    pub(super) fn fmt_file(&mut self, f: &File) {
        for item in &f.items {
            self.fmt_item(item);
        }
        self.finish();
    }

    /// Schnipsel (Testeinstieg, siehe `parse_snippet`).
    pub(super) fn fmt_snippet(&mut self, items: &[SnippetItem]) {
        for item in items {
            match item {
                SnippetItem::Item(i) => self.fmt_item(i),
                SnippetItem::MachinePrelude(m) => self.fmt_machine_prelude(m),
                SnippetItem::Initial(_) => {
                    self.sp("initial");
                    self.name();
                    self.newline();
                }
                SnippetItem::Enter(b) => self.fmt_enter_block(b),
                SnippetItem::Exit(b) => self.fmt_exit_block(b),
                SnippetItem::Loop(b) => self.fmt_loop_block(b),
                SnippetItem::On(h) => self.fmt_on_handler(h),
                SnippetItem::Sequence(items) => self.fmt_sequence_block(None, items),
                SnippetItem::Transition(t) => self.fmt_transition(t),
                SnippetItem::State(s) => self.fmt_state_decl(s),
                SnippetItem::Step(s) => self.fmt_step_decl(s),
                SnippetItem::Seq(s) => self.fmt_seq_item(s),
                SnippetItem::Instance(i) => self.fmt_instance_decl(i),
            }
        }
        self.finish();
    }

    fn fmt_item(&mut self, item: &Item) {
        match item {
            Item::Import(i) => self.fmt_import(i),
            Item::System(s) => self.fmt_system_decl(s),
            Item::Type(t) => self.fmt_type_decl(t),
            Item::Unitvec(u) => self.fmt_unitvec_decl(u),
            Item::Enum(e) => self.fmt_enum_decl(e),
            Item::Record(r) => self.fmt_record_decl(r),
            Item::Unit(u) => self.fmt_unit_decl(u),
            Item::Stream(s) => self.fmt_stream_decl(s),
            Item::Port(p) => self.fmt_port_decl(p),
            Item::Node(n) => self.fmt_node_decl(n),
            Item::Property(p) => match p.kind {
                PropertyKind::Property => self.fmt_property_decl(p),
                PropertyKind::Assumption => self.fmt_assumption_decl(p),
            },
            Item::Const(c) => self.fmt_const_decl(c),
            Item::Param(p) => self.fmt_param_decl(p),
            Item::Profile(p) => self.fmt_profile_decl(p),
            Item::Channel(c) => self.fmt_channel_decl(c),
            Item::Command(c) => self.fmt_command_decl(c),
            Item::Fn(f) => self.fmt_fn_decl(f),
            Item::Native(n) => self.fmt_native_decl(n),
            Item::Block(b) => self.fmt_block_decl(b),
            Item::Machine(m) => self.fmt_machine_decl(m),
            Item::Instance(i) => self.fmt_instance_decl(i),
            Item::Scenario(s) => self.fmt_scenario_decl(s),
            Item::Campaign(c) => self.fmt_campaign_decl(c),
            Item::Trigger(t) => self.fmt_trigger_decl(t),
        }
    }

    /// `import`
    fn fmt_import(&mut self, i: &Import) {
        self.sp("import");
        match i {
            Import::Module { path, alias, .. } => {
                self.name();
                for _ in 1..path.len() {
                    self.op(".");
                    self.glue();
                    self.name();
                }
                if alias.is_some() {
                    self.sp("as");
                    self.name();
                }
            }
            Import::Channels { .. } => {
                self.sp("channels");
                self.sp("from");
                self.name();
            }
        }
        self.newline();
    }

    /// `system_decl`
    fn fmt_system_decl(&mut self, s: &SystemDecl) {
        self.sp("system");
        self.op(":");
        self.newline();
        self.indent();
        for item in &s.items {
            self.fmt_system_item(item);
        }
        self.dedent();
    }

    /// `system_item`
    fn fmt_system_item(&mut self, item: &SystemItem) {
        self.kind(Kind::SystemItem);
        self.name();
        self.cell();
        self.op("=");
        match item {
            SystemItem::Tick(_) | SystemItem::Target(_) | SystemItem::Language(_) => self.name(),
            SystemItem::OutputTiming(_) | SystemItem::FaultIsFail(_) | SystemItem::Float(_) => self.name(),
            SystemItem::TickSource(_) => {
                self.sp("hw");
                self.op("(");
                self.glue();
                self.name();
                self.op(")");
            }
            SystemItem::TickTolerance { value, ticks } => {
                self.fmt_const_expr(value);
                if ticks.is_some() {
                    self.sp("for");
                    self.fmt_int_lit();
                    self.sp("ticks");
                }
            }
        }
        self.newline();
    }

    // ------------------------------------------------------------ Typen und Einheiten

    /// `type_decl`
    fn fmt_type_decl(&mut self, t: &TypeDecl) {
        self.kind(Kind::Alias);
        self.sp("type");
        self.name();
        self.cell();
        self.op("=");
        self.fmt_type(&t.ty);
        self.newline();
    }

    /// `unitvec_decl`
    fn fmt_unitvec_decl(&mut self, u: &UnitvecDecl) {
        self.sp("unitvec");
        self.name();
        self.sp("=");
        self.sp("(");
        self.glue();
        for (i, unit) in u.units.iter().enumerate() {
            if i > 0 {
                self.op(",");
            }
            self.fmt_unit_expr(unit);
        }
        self.op(")");
        self.newline();
    }

    /// `enum_decl`
    fn fmt_enum_decl(&mut self, e: &EnumDecl) {
        self.sp("enum");
        self.name();
        if e.layout.is_some() {
            self.sp("layout");
            self.fmt_int_type();
        }
        if e.open {
            self.sp("open");
        }
        self.op(":");
        if self.at_newline() {
            self.newline();
            self.indent();
            for v in &e.variants {
                self.kind(Kind::Variant);
                self.fmt_variant(v, true);
                self.newline();
            }
            self.dedent();
        } else {
            for (i, v) in e.variants.iter().enumerate() {
                if i > 0 {
                    self.op(",");
                }
                self.fmt_variant(v, false);
            }
            self.newline();
        }
    }

    /// `variant`; `cells`: als Tabellenzeile mit Ausrichtungspunkten.
    fn fmt_variant(&mut self, v: &Variant, cells: bool) {
        self.name();
        if v.discriminant.is_some() {
            if cells {
                self.cell();
            }
            self.sp("=");
            self.fmt_int_lit();
        }
        if !v.fields.is_empty() {
            self.op("(");
            self.glue();
            for (i, f) in v.fields.iter().enumerate() {
                if i > 0 {
                    self.op(",");
                }
                self.fmt_field(f, false);
            }
            self.op(")");
        }
    }

    /// `record_decl`
    fn fmt_record_decl(&mut self, r: &RecordDecl) {
        self.sp("record");
        self.name();
        if let Some(layout) = &r.layout {
            self.sp("layout");
            self.name();
            if layout.align.is_some() {
                self.op(",");
                self.sp("align");
                self.sp("=");
                self.fmt_int_lit();
            }
        }
        self.op(":");
        self.newline();
        self.indent();
        for f in &r.fields {
            self.fmt_record_field(f);
        }
        self.dedent();
    }

    /// `record_field`
    fn fmt_record_field(&mut self, f: &RecordField) {
        match f {
            RecordField::Plain(f) => {
                self.kind(Kind::Field);
                self.fmt_field(f, true);
                self.newline();
            }
            RecordField::Bits { bits, .. } => {
                self.kind(Kind::Field);
                self.name();
                self.cell();
                self.op(":");
                self.fmt_int_type();
                self.sp("with");
                self.sp("bits");
                self.op(":");
                self.newline();
                self.indent();
                for b in bits {
                    self.fmt_bitfield(b);
                    self.newline();
                }
                self.dedent();
            }
        }
    }

    /// `field`; als Tabellenzeile (`cells`) oder inline in einer Variante.
    fn fmt_field(&mut self, f: &Field, cells: bool) {
        self.name();
        if cells {
            self.cell();
            self.op(":");
        } else {
            self.op(":");
        }
        self.fmt_type(&f.ty);
        if let Some(v) = &f.value {
            self.sp("=");
            self.fmt_const_expr(v);
        }
        if f.offset.is_some() {
            self.sp("offset");
            self.sp("=");
            self.fmt_int_lit();
        }
        if f.len_field.is_some() {
            self.sp("with");
            self.sp("len");
            self.sp("=");
            self.name();
        }
    }

    /// `bitfield`
    fn fmt_bitfield(&mut self, b: &Bitfield) {
        self.kind(Kind::Bitfield);
        self.name();
        self.cell();
        self.op(":");
        match b.ty {
            BitType::Bool => self.sp("bool"),
            BitType::Int(_) => self.fmt_int_type(),
        }
        self.sp("at");
        self.fmt_int_lit();
        if b.to.is_some() {
            self.op("..");
            self.glue();
            self.fmt_int_lit();
        }
    }

    /// `unit_decl`
    fn fmt_unit_decl(&mut self, u: &UnitDecl) {
        self.kind(Kind::Alias);
        self.sp("unit");
        self.name();
        self.cell();
        self.op("=");
        match u {
            UnitDecl::Scaled { unit, .. } => {
                self.fmt_number();
                if let Some(unit) = unit {
                    self.fmt_unit_lit(unit);
                }
            }
            UnitDecl::Affine { base, .. } => {
                self.sp("affine");
                self.op("(");
                self.glue();
                self.fmt_unit_expr(base);
                self.op(",");
                self.fmt_number();
                self.op(")");
            }
        }
        self.newline();
    }

    // ------------------------------------------------------------ Konstanten, Parameter, Channels

    /// `const_decl`
    fn fmt_const_decl(&mut self, c: &ConstDecl) {
        self.kind(Kind::Param);
        self.sp("const");
        self.name();
        self.cell();
        if let Some(ty) = &c.ty {
            self.op(":");
            self.fmt_type(ty);
        }
        self.sp("=");
        self.fmt_const_expr(&c.value);
        self.newline();
    }

    /// `param_decl`
    fn fmt_param_decl(&mut self, p: &ParamDecl) {
        self.kind(Kind::Param);
        if p.tunable {
            self.sp("tunable");
        }
        self.sp("param");
        self.name();
        self.cell();
        self.op(":");
        self.fmt_type(&p.ty);
        self.sp("=");
        self.fmt_const_expr(&p.value);
        self.fmt_attrs(&p.attrs, false);
        self.newline();
    }

    /// `profile_decl`
    fn fmt_profile_decl(&mut self, p: &ProfileDecl) {
        self.sp("profile");
        self.name();
        self.op(":");
        self.newline();
        self.indent();
        for (_, value) in &p.entries {
            self.kind(Kind::ProfileEntry);
            self.name();
            self.cell();
            self.op("=");
            self.fmt_const_expr(value);
            self.newline();
        }
        self.dedent();
    }

    /// `channel_decl`
    fn fmt_channel_decl(&mut self, c: &ChannelDecl) {
        self.kind(Kind::Channel);
        self.name();
        self.pad("output".len()); // `input ` steht immer auf der Breite von `output`
        self.cell();
        self.name();
        self.cell();
        self.op(":");
        self.fmt_type(&c.ty);
        self.cell();
        self.op("@");
        self.fmt_binding(&c.binding);
        self.fmt_attrs(&c.attrs, true);
        self.newline();
    }

    /// `stream_decl`
    fn fmt_stream_decl(&mut self, s: &StreamDecl) {
        self.sp("stream");
        self.op("<");
        self.glue();
        self.fmt_elem_type(&s.elem);
        self.op(">");
        self.name();
        self.fmt_attrs(&s.attrs, false);
        self.newline();
    }

    /// `port_decl`
    fn fmt_port_decl(&mut self, _p: &PortDecl) {
        self.sp("port");
        self.name();
        self.sp(":");
        self.name();
        self.sp("@");
        self.sp("mmio");
        self.op("(");
        self.glue();
        self.name();
        self.op(")");
        self.newline();
    }

    /// `binding`
    fn fmt_binding(&mut self, b: &Binding) {
        match b {
            Binding::Hw(_) | Binding::Sim(_) => {
                self.name();
                self.op("(");
                self.glue();
                self.name();
                self.op(")");
            }
            Binding::None => self.sp("none"),
        }
    }

    /// `[ "with" attr { "," attr } ]`; `cell`: eigene Ausrichtungszelle.
    fn fmt_attrs(&mut self, attrs: &[Attr], cell: bool) {
        if attrs.is_empty() {
            return;
        }
        if cell {
            self.cell();
        }
        self.sp("with");
        for (i, a) in attrs.iter().enumerate() {
            if i > 0 {
                self.op(",");
            }
            self.fmt_attr(a);
        }
    }

    /// `attr`
    fn fmt_attr(&mut self, a: &Attr) {
        self.name();
        self.sp("=");
        match &a.kind {
            AttrKind::Safe(e) | AttrKind::Rate(e) | AttrKind::MaxRate(e) | AttrKind::MaxSlew(e) => {
                self.fmt_const_expr(e)
            }
            AttrKind::MaxAge(_) | AttrKind::Jitter(_) => self.fmt_duration_lit(),
            AttrKind::Capacity(_) | AttrKind::Debounce(_) | AttrKind::CapacityBytes(_) | AttrKind::ExpectLen(_) => {
                self.fmt_int_lit()
            }
            AttrKind::Framing(f) => self.fmt_framing(f),
            AttrKind::Overflow(_) | AttrKind::Wake(_) | AttrKind::Irreversible => self.name(),
            AttrKind::Label(_) | AttrKind::Group(_) | AttrKind::Doc(_) => self.name(),
            AttrKind::Display(u) => self.fmt_unit_expr(u),
            AttrKind::Budget(items) => {
                // `{` folgt dem `=` mit Leerzeichen wie ein gewoehnlicher
                // Wert; direkt dahinter beginnt der erste Posten, und das
                // `}` schliesst ohne Leerzeichen an den letzten an.
                self.sp("{");
                self.glue();
                for (i, it) in items.iter().enumerate() {
                    if i > 0 {
                        self.op(",");
                    }
                    self.fmt_budget_item(it);
                }
                self.op("}");
            }
        }
    }

    /// `budget_item`
    fn fmt_budget_item(&mut self, it: &BudgetItem) {
        self.name();
        self.sp("=");
        self.fmt_const_expr(&it.value);
    }

    /// `framing`
    fn fmt_framing(&mut self, f: &Framing) {
        self.name();
        if matches!(f, Framing::LengthPrefixed(_) | Framing::Fixed(_)) {
            self.op("(");
            self.glue();
            self.name();
            self.op(")");
        }
    }

    /// `command_decl`
    fn fmt_command_decl(&mut self, c: &CommandDecl) {
        self.sp("command");
        self.name();
        self.fmt_attrs(&c.attrs, false);
        self.newline();
    }

    // ------------------------------------------------------------ Funktionen, Natives, Bloecke

    /// `fn_decl`
    fn fmt_fn_decl(&mut self, f: &FnDecl) {
        self.sp("fn");
        self.name();
        self.fmt_generic_vars(&f.generics);
        self.fmt_params(&f.params);
        if let Some(ret) = &f.ret {
            self.sp("->");
            self.fmt_type(ret);
        }
        self.op(":");
        self.fmt_block(&f.body);
    }

    /// `native_decl`
    fn fmt_native_decl(&mut self, n: &NativeDecl) {
        self.sp("native");
        self.name();
        self.name();
        self.fmt_generic_vars(&n.generics);
        self.fmt_params(&n.params);
        self.sp("->");
        self.fmt_type(&n.ret);
        if n.from.is_some() {
            self.sp("from");
            self.name();
        }
        self.sp("with");
        self.sp("cost");
        self.sp("=");
        self.fmt_cost_spec(&n.cost);
        self.op(",");
        self.sp("stack");
        self.sp("=");
        self.name();
        if n.duration.is_some() {
            self.op(",");
            self.sp("duration");
            self.sp("=");
            self.fmt_duration_lit();
        }
        self.op(",");
        self.sp("total");
        self.newline();
    }

    /// `cost_spec`
    fn fmt_cost_spec(&mut self, c: &CostSpec) {
        match c {
            CostSpec::Single(_) => self.fmt_int_lit(),
            CostSpec::Classes(classes) => {
                self.sp("{");
                self.glue();
                for i in 0..classes.len() {
                    if i > 0 {
                        self.op(",");
                    }
                    self.fmt_cost_class();
                    self.op(":");
                    self.fmt_int_lit();
                }
                self.op("}");
            }
        }
    }

    /// `cost_class`
    fn fmt_cost_class(&mut self) {
        self.name();
    }

    /// `block_decl`
    fn fmt_block_decl(&mut self, b: &BlockDecl) {
        self.sp("block");
        self.name();
        self.fmt_generic_vars(&b.generics);
        self.fmt_params(&b.params);
        self.op(":");
        self.newline();
        self.indent();
        for v in &b.vars {
            self.fmt_var_decl(v);
            self.newline();
        }
        if let Some(s) = &b.step {
            self.fmt_step_decl(s);
        }
        for m in &b.methods {
            self.fmt_method_decl(m);
        }
        self.dedent();
    }

    /// `step_decl`
    fn fmt_step_decl(&mut self, s: &StepDecl) {
        self.sp("step");
        self.fmt_params(&s.params);
        self.sp("->");
        self.fmt_type(&s.ret);
        self.op(":");
        self.fmt_block(&s.body);
    }

    /// `method_decl`
    fn fmt_method_decl(&mut self, m: &MethodDecl) {
        self.name();
        self.fmt_params(&m.params);
        if let Some(ret) = &m.ret {
            self.sp("->");
            self.fmt_type(ret);
        }
        self.op(":");
        self.fmt_block(&m.body);
    }

    /// `generic_vars`
    fn fmt_generic_vars(&mut self, vars: &[GenericVar]) {
        if vars.is_empty() {
            return;
        }
        self.op("[");
        self.glue();
        for (i, v) in vars.iter().enumerate() {
            if i > 0 {
                self.op(",");
            }
            self.fmt_gvar(v);
        }
        self.op("]");
    }

    /// `gvar`
    fn fmt_gvar(&mut self, v: &GenericVar) {
        match v {
            GenericVar::Unit(_) => self.name(),
            GenericVar::Type { capability, .. } => {
                self.sp("type");
                self.name();
                if capability.is_some() {
                    self.op(":");
                    self.fmt_capability();
                }
            }
            GenericVar::Const { range, .. } => {
                self.sp("const");
                self.name();
                if let Some(r) = range {
                    self.sp("in");
                    self.fmt_range(r);
                }
            }
        }
    }

    /// `capability`
    fn fmt_capability(&mut self) {
        self.name();
    }

    /// `"(" [ params ] ")"`
    fn fmt_params(&mut self, params: &[Param]) {
        self.op("(");
        self.glue();
        for (i, p) in params.iter().enumerate() {
            if i > 0 {
                self.op(",");
            }
            self.fmt_param(p);
        }
        self.op(")");
    }

    /// `param`
    fn fmt_param(&mut self, p: &Param) {
        if p.inout {
            self.sp("inout");
        }
        self.name();
        self.op(":");
        if p.dir.is_some() {
            self.name();
        }
        self.fmt_type(&p.ty);
        if let Some(d) = &p.default {
            self.sp("=");
            self.fmt_const_expr(d);
        }
    }

    // ------------------------------------------------------------ Maschinen

    /// `machine_decl`
    fn fmt_machine_decl(&mut self, m: &MachineDecl) {
        if m.driver {
            self.sp("driver");
        }
        self.sp("machine");
        self.name();
        if self.at_text("(") {
            self.fmt_params(&m.params);
        }
        if !m.follows.is_empty() {
            self.sp("follows");
            for i in 0..m.follows.len() {
                if i > 0 {
                    self.op(",");
                }
                self.name();
            }
        }
        if m.node.is_some() {
            self.sp("node");
            self.name();
        }
        if m.every.is_some() {
            self.sp("every");
            self.fmt_duration_lit();
        }
        if m.phase.is_some() {
            self.sp("phase");
            self.fmt_duration_lit();
        }
        self.fmt_attrs(&m.attrs, false);
        self.op(":");
        self.newline();
        self.indent();
        self.fmt_machine_body(&m.body);
        self.dedent();
    }

    /// `machine_body`
    fn fmt_machine_body(&mut self, b: &MachineBody) {
        for m in &b.prelude {
            self.fmt_machine_prelude(m);
        }
        self.sp("initial");
        self.name();
        self.newline();
        if let Some(l) = &b.loop_block {
            self.fmt_loop_block(l);
        }
        for h in &b.handlers {
            self.fmt_on_handler(h);
        }
        for s in &b.states {
            self.fmt_state_decl(s);
        }
    }

    fn fmt_machine_prelude(&mut self, m: &MachinePrelude) {
        match m {
            MachinePrelude::Var(v) => {
                self.fmt_var_decl(v);
                self.newline();
            }
            MachinePrelude::Persist(p) => self.fmt_persist_decl(p),
            MachinePrelude::Signal(_) => self.fmt_signal_decl(),
            MachinePrelude::Fault(_) => self.fmt_fault_clause(),
        }
    }

    /// `persist_decl`
    fn fmt_persist_decl(&mut self, p: &PersistDecl) {
        self.kind(Kind::Var);
        self.sp("persist");
        self.sp("var");
        self.name();
        self.cell();
        self.op(":");
        self.fmt_type(&p.ty);
        self.sp("=");
        self.fmt_const_expr(&p.value);
        if p.min_interval.is_some() {
            self.sp("with");
            self.sp("min_interval");
            self.sp("=");
            self.fmt_duration_lit();
        }
        self.newline();
    }

    /// `signal_decl`
    fn fmt_signal_decl(&mut self) {
        self.sp("signal");
        self.name();
        self.newline();
    }

    /// `state_decl`
    fn fmt_state_decl(&mut self, s: &StateDecl) {
        self.sp("state");
        self.name();
        if s.idle {
            self.sp("idle");
        }
        if s.resume {
            self.sp("resume");
        }
        self.fmt_attrs(&s.attrs, false);
        self.op(":");
        self.newline();
        self.indent();
        self.fmt_state_body(&s.body);
        self.dedent();
    }

    /// `state_body`
    fn fmt_state_body(&mut self, b: &StateBody) {
        for p in &b.prelude {
            match p {
                StatePrelude::Fault(_) => self.fmt_fault_clause(),
                StatePrelude::Var(v) => {
                    self.fmt_var_decl(v);
                    self.newline();
                }
                StatePrelude::Instance(i) => self.fmt_instance_decl(i),
            }
        }
        if b.initial.is_some() {
            self.sp("initial");
            self.name();
            self.newline();
        }
        if let Some(e) = &b.enter {
            self.fmt_enter_block(e);
        }
        if let Some(l) = &b.loop_block {
            self.fmt_loop_block(l);
        }
        for h in &b.handlers {
            self.fmt_on_handler(h);
        }
        if let Some(seq) = &b.sequence {
            self.fmt_sequence_block(b.sequence_timeout.as_ref(), seq);
        }
        for t in &b.transitions {
            self.fmt_transition(t);
        }
        if let Some(e) = &b.exit {
            self.fmt_exit_block(e);
        }
        for s in &b.states {
            self.fmt_state_decl(s);
        }
    }

    /// `fault_clause`
    fn fmt_fault_clause(&mut self) {
        self.sp("fault");
        self.sp("->");
        self.name();
        self.newline();
    }

    /// `enter_block`
    fn fmt_enter_block(&mut self, b: &Block) {
        self.sp("enter");
        self.op(":");
        self.fmt_action_block(b);
    }

    /// `exit_block`
    fn fmt_exit_block(&mut self, b: &Block) {
        self.sp("exit");
        self.op(":");
        self.fmt_action_block(b);
    }

    /// `loop_block`
    fn fmt_loop_block(&mut self, b: &Block) {
        self.sp("loop");
        self.op(":");
        self.fmt_block(b);
    }

    /// `on_handler`
    fn fmt_on_handler(&mut self, h: &OnHandler) {
        self.sp("on");
        self.name();
        if let Some((_, pattern)) = &h.pattern {
            self.name();
            self.fmt_pattern(pattern);
        }
        if h.binding.is_some() {
            self.sp("as");
            self.name();
        }
        if let Some(g) = &h.guard {
            self.sp("when");
            self.fmt_expr(g);
        }
        self.op(":");
        self.fmt_block(&h.body);
    }

    /// `transition`
    fn fmt_transition(&mut self, t: &Transition) {
        match &t.trigger {
            Trigger::When(g) => {
                self.sp("when");
                self.fmt_guard(g);
            }
            Trigger::After(d) => {
                self.sp("after");
                self.fmt_duration_expr(d);
            }
        }
        self.op(":");
        self.fmt_trans_block(&t.actions);
    }

    /// `guard`
    fn fmt_guard(&mut self, g: &Guard) {
        match g {
            Guard::Expr(e) => self.fmt_expr(e),
            Guard::Next { subject, .. } => {
                self.fmt_expr(subject);
                self.sp("as");
                self.name();
            }
        }
    }

    /// `trans_block`
    fn fmt_trans_block(&mut self, actions: &[Stmt]) {
        if self.at_newline() {
            self.newline();
            self.indent();
            for s in actions {
                self.fmt_stmt(s);
            }
            self.fmt_goto_stmt();
            self.newline();
            self.dedent();
        } else {
            self.fmt_goto_stmt();
            self.newline();
        }
    }

    /// `sequence_block`
    fn fmt_sequence_block(&mut self, timeout: Option<&Timeout>, items: &[SeqItem]) {
        self.sp("sequence");
        if let Some(Timeout { duration, action }) = timeout {
            self.sp("with");
            self.sp("timeout");
            self.op("=");
            self.fmt_duration_expr(duration);
            if let TimeoutAction::Goto(_) = action {
                self.fmt_goto_stmt();
            }
        }
        self.op(":");
        self.fmt_seq_items(items);
    }

    fn fmt_seq_items(&mut self, items: &[SeqItem]) {
        self.newline();
        self.indent();
        for item in items {
            self.fmt_seq_item(item);
        }
        self.dedent();
    }

    /// `seq_item`
    fn fmt_seq_item(&mut self, item: &SeqItem) {
        match item {
            SeqItem::Stmt(s) => self.fmt_stmt(s),
            SeqItem::Wait(d) => {
                self.sp("wait");
                self.fmt_duration_expr(d);
                self.newline();
            }
            SeqItem::Until { guard, timeout, .. } => {
                self.sp("until");
                self.fmt_guard(guard);
                match timeout {
                    None => self.newline(),
                    Some(Timeout { duration, action }) => {
                        self.sp("timeout");
                        self.fmt_duration_expr(duration);
                        match action {
                            TimeoutAction::Fault => self.newline(),
                            TimeoutAction::Goto(_) => {
                                self.fmt_goto_stmt();
                                self.newline();
                            }
                            TimeoutAction::Else(b) => {
                                self.sp("else");
                                self.op(":");
                                self.fmt_action_block(b);
                            }
                        }
                    }
                }
            }
            SeqItem::Expect { cond, message, .. } => {
                self.sp("expect");
                self.fmt_expr(cond);
                if message.is_some() {
                    self.op(",");
                    self.name();
                }
                self.newline();
            }
            SeqItem::Repeat { count, body, .. } => {
                self.sp("repeat");
                self.fmt_const_expr(count);
                self.op(":");
                self.fmt_seq_items(body);
            }
            SeqItem::Step { body, .. } => {
                self.sp("step");
                self.name();
                self.op(":");
                self.fmt_seq_items(body);
            }
        }
    }

    /// `instance_decl`
    fn fmt_instance_decl(&mut self, i: &InstanceDecl) {
        self.sp("instance");
        self.name();
        if let Some((_, range)) = &i.index {
            self.op("[");
            self.glue();
            self.name();
            self.sp("in");
            self.fmt_range(range);
            self.op("]");
        }
        if i.resume {
            self.sp("resume");
        }
        self.sp("=");
        self.name();
        self.fmt_arg_list(&i.args);
        self.newline();
    }

    /// `node_decl`
    fn fmt_node_decl(&mut self, n: &NodeDecl) {
        self.sp("node");
        self.name();
        self.sp("@");
        self.sp("hw");
        self.op("(");
        self.glue();
        self.name();
        self.op(")");
        if n.tick.is_some() {
            self.sp("with");
            self.sp("tick");
            self.sp("=");
            self.fmt_duration_lit();
        }
        self.newline();
    }

    /// `property_decl`
    fn fmt_property_decl(&mut self, p: &PropertyDecl) {
        self.fmt_property_like(p)
    }

    /// `assumption_decl` — dieselbe Form wie `property_decl` (13.3).
    fn fmt_assumption_decl(&mut self, p: &PropertyDecl) {
        self.fmt_property_like(p)
    }

    fn fmt_property_like(&mut self, p: &PropertyDecl) {
        self.sp(p.kind.word());
        self.name();
        self.op(":");
        self.fmt_tprop(&p.prop);
        if p.monitor {
            self.sp("with");
            self.sp("monitor");
            self.sp("=");
            self.sp("true");
        }
        self.newline();
    }

    /// `tprop` (Implikation und Temporaloperatoren stecken im Ausdrucksbaum)
    fn fmt_tprop(&mut self, e: &Expr) {
        self.fmt_expr(e);
    }

    /// `scenario_decl`
    fn fmt_scenario_decl(&mut self, s: &ScenarioDecl) {
        self.sp("scenario");
        self.name();
        if s.every.is_some() {
            self.sp("every");
            self.fmt_duration_lit();
        }
        self.op(":");
        self.newline();
        self.indent();
        self.fmt_machine_body(&s.body);
        self.dedent();
    }

    /// `campaign_decl`
    fn fmt_campaign_decl(&mut self, c: &CampaignDecl) {
        self.sp("campaign");
        self.name();
        self.op(":");
        self.newline();
        self.indent();
        for item in &c.items {
            self.fmt_campaign_item(item);
        }
        self.dedent();
    }

    /// `campaign_item`
    fn fmt_campaign_item(&mut self, item: &CampaignItem) {
        self.name();
        match item {
            CampaignItem::Program(_) | CampaignItem::Profile(_) | CampaignItem::StopOn(_) => self.name(),
            CampaignItem::Repeat(_) => self.fmt_int_lit(),
            CampaignItem::SweepRange { from, to, step, .. } => {
                self.name();
                self.sp("=");
                self.fmt_const_expr(from);
                self.op("..");
                self.glue();
                self.fmt_const_expr(to);
                self.sp("step");
                self.fmt_const_expr(step);
            }
            CampaignItem::SweepList { values, .. } => {
                self.name();
                self.sp("=");
                self.sp("[");
                self.glue();
                for (i, v) in values.iter().enumerate() {
                    if i > 0 {
                        self.op(",");
                    }
                    self.fmt_const_expr(v);
                }
                self.op("]");
            }
        }
        self.newline();
    }

    /// `trigger_decl`
    fn fmt_trigger_decl(&mut self, t: &TriggerDecl) {
        self.sp("trigger");
        self.name();
        if t.node.is_some() {
            self.sp("node");
            self.name();
        }
        self.op(":");
        self.newline();
        self.indent();
        self.sp("when");
        self.fmt_guard(&t.when);
        self.newline();
        self.sp("then");
        self.fmt_at_stmt(&t.then);
        self.sp("bound");
        self.fmt_duration_lit();
        self.newline();
        self.dedent();
    }

    // ------------------------------------------------------------ Anweisungen

    /// `block`
    fn fmt_block(&mut self, b: &Block) {
        if self.at_newline() {
            self.newline();
            self.indent();
            for s in &b.stmts {
                self.fmt_stmt(s);
            }
            self.dedent();
        } else if let Some(s) = b.stmts.first() {
            self.fmt_simple_stmt(s);
            self.newline();
        }
    }

    /// `action_block`
    fn fmt_action_block(&mut self, b: &Block) {
        self.fmt_block(b);
    }

    /// `stmt`
    fn fmt_stmt(&mut self, s: &Stmt) {
        match &s.kind {
            StmtKind::If { branches, otherwise } => self.fmt_if_stmt(branches, otherwise.as_ref()),
            StmtKind::For { target, iter, body } => self.fmt_for_stmt(target, iter, body),
            StmtKind::Match { subject, cases } => self.fmt_match_stmt(subject, cases),
            StmtKind::At(a) => self.fmt_at_stmt(a),
            StmtKind::Every { period, body } => self.fmt_every_stmt(period, body),
            _ => {
                self.fmt_simple_stmt(s);
                self.newline();
            }
        }
    }

    /// `simple_stmt`
    fn fmt_simple_stmt(&mut self, s: &Stmt) {
        match &s.kind {
            StmtKind::Assign { target, op, value } => self.fmt_assign(target, *op, value),
            StmtKind::Var(v) => self.fmt_var_decl(v),
            StmtKind::Job { args, .. } => self.fmt_job_stmt(args),
            StmtKind::Arm { .. } => self.fmt_arm_stmt(),
            StmtKind::Check { cond, message, confirm, within, target, req } => self.fmt_check_stmt(
                cond,
                message.is_some(),
                confirm.as_ref(),
                within.as_ref(),
                target.is_some(),
                req.is_some(),
            ),
            StmtKind::Alert { cond, confirm, .. } => self.fmt_alert_stmt(cond, confirm.as_ref()),
            StmtKind::Log(_) => self.fmt_log_stmt(),
            StmtKind::Goto(_) => self.fmt_goto_stmt(),
            StmtKind::Abort(m) => self.fmt_abort_stmt(m.is_some()),
            StmtKind::Return(e) => self.fmt_return_stmt(e),
            StmtKind::Send { value, .. } => self.fmt_send_stmt(value),
            StmtKind::Pulse { value, duration, .. } => self.fmt_pulse_stmt(value, duration),
            StmtKind::Cancel(_) => self.fmt_cancel_stmt(),
            StmtKind::Measure { value, .. } => self.fmt_measure_stmt(value),
            StmtKind::Verify { cond, req, .. } => self.fmt_verify_stmt(cond, req.is_some()),
            StmtKind::Verdict { message, .. } => self.fmt_verdict_stmt(message.is_some()),
            StmtKind::Raise(_) => self.fmt_raise_stmt(),
            StmtKind::Break => self.sp("break"),
            StmtKind::Pass => self.sp("pass"),
            StmtKind::Expr(e) => self.fmt_expr(e),
            StmtKind::If { .. }
            | StmtKind::For { .. }
            | StmtKind::Match { .. }
            | StmtKind::At(_)
            | StmtKind::Every { .. } => self.fmt_stmt(s),
        }
    }

    /// `assign`
    fn fmt_assign(&mut self, target: &Expr, op: AssignOp, value: &Expr) {
        self.fmt_lvalue(target);
        self.sp(match op {
            AssignOp::Set => "=",
            AssignOp::Add => "+=",
            AssignOp::Sub => "-=",
            AssignOp::Mul => "*=",
            AssignOp::Div => "/=",
        });
        self.fmt_expr(value);
    }

    /// `lvalue`
    fn fmt_lvalue(&mut self, target: &Expr) {
        self.fmt_expr(target);
    }

    /// `var_decl` (ohne Zeilenende)
    fn fmt_var_decl(&mut self, v: &VarDecl) {
        self.kind(Kind::Var);
        if v.public {
            self.sp("pub");
        }
        self.sp("var");
        self.name();
        self.cell();
        if let Some(ty) = &v.ty {
            self.op(":");
            self.fmt_type(ty);
        }
        self.sp("=");
        self.fmt_expr(&v.value);
    }

    /// `if_stmt`
    fn fmt_if_stmt(&mut self, branches: &[(Expr, Block)], otherwise: Option<&Block>) {
        for (i, (cond, body)) in branches.iter().enumerate() {
            self.sp(if i == 0 { "if" } else { "elif" });
            self.fmt_expr(cond);
            self.op(":");
            self.fmt_block(body);
        }
        if let Some(b) = otherwise {
            self.sp("else");
            self.op(":");
            self.fmt_block(b);
        }
    }

    /// `for_stmt`
    fn fmt_for_stmt(&mut self, target: &ForTarget, iter: &ForIter, body: &Block) {
        self.sp("for");
        match target {
            ForTarget::One(_) => self.name(),
            ForTarget::Pair(..) => {
                self.sp("(");
                self.glue();
                self.name();
                self.op(",");
                self.name();
                self.op(")");
            }
        }
        self.sp("in");
        match iter {
            ForIter::Range(n) => {
                self.sp("range");
                self.op("(");
                self.glue();
                self.fmt_const_expr(n);
                self.op(")");
            }
            ForIter::Expr(e) => self.fmt_expr(e),
        }
        self.op(":");
        self.fmt_block(body);
    }

    /// `match_stmt`
    fn fmt_match_stmt(&mut self, subject: &Expr, cases: &[Case]) {
        self.sp("match");
        self.fmt_expr(subject);
        self.op(":");
        self.newline();
        self.indent();
        for c in cases {
            self.sp("case");
            self.fmt_case_pattern(&c.pattern);
            self.op(":");
            self.fmt_block(&c.body);
        }
        self.dedent();
    }

    /// `case_pattern`
    fn fmt_case_pattern(&mut self, p: &CasePattern) {
        match p {
            CasePattern::Wild => self.name(),
            CasePattern::Variant { fields, .. } => {
                self.name();
                if !fields.is_empty() {
                    self.op("(");
                    self.glue();
                    for i in 0..fields.len() {
                        if i > 0 {
                            self.op(",");
                        }
                        self.name();
                    }
                    self.op(")");
                }
            }
            CasePattern::Values(values) => {
                for (i, v) in values.iter().enumerate() {
                    if i > 0 {
                        self.op(",");
                    }
                    self.fmt_const_expr(&v.from);
                    if let Some(to) = &v.to {
                        self.op("..");
                        self.glue();
                        self.fmt_const_expr(to);
                    }
                }
            }
        }
    }

    /// `at_stmt`
    fn fmt_at_stmt(&mut self, a: &AtStmt) {
        self.sp("at");
        self.fmt_duration_expr(&a.time);
        self.op(":");
        self.fmt_action_block(&a.body);
    }

    /// `every_stmt`
    fn fmt_every_stmt(&mut self, period: &Expr, body: &Block) {
        self.sp("every");
        self.fmt_duration_expr(period);
        self.op(":");
        self.fmt_block(body);
    }

    /// `check_stmt`
    fn fmt_check_stmt(
        &mut self,
        cond: &Expr,
        message: bool,
        confirm: Option<&Expr>,
        within: Option<&Expr>,
        target: bool,
        req: bool,
    ) {
        self.sp("check");
        self.fmt_expr(cond);
        if message {
            self.op(",");
            self.name();
        }
        if let Some(c) = confirm {
            self.sp("for");
            self.fmt_duration_expr(c);
        }
        if let Some(w) = within {
            self.sp("within");
            self.fmt_duration_expr(w);
        }
        if target {
            self.fmt_goto_stmt();
        }
        if req {
            self.sp("req");
            self.name();
        }
    }

    /// `alert_stmt`
    fn fmt_alert_stmt(&mut self, cond: &Expr, confirm: Option<&Expr>) {
        self.sp("alert");
        self.fmt_expr(cond);
        self.op(",");
        self.name();
        if let Some(c) = confirm {
            self.sp("for");
            self.fmt_duration_expr(c);
        }
    }

    /// `log_stmt`
    fn fmt_log_stmt(&mut self) {
        self.sp("log");
        self.name();
    }

    /// `send_stmt`
    fn fmt_send_stmt(&mut self, value: &Expr) {
        self.sp("send");
        self.name();
        self.op(",");
        self.fmt_expr(value);
    }

    /// `pulse_stmt`
    fn fmt_pulse_stmt(&mut self, value: &Expr, duration: &Expr) {
        self.sp("pulse");
        self.name();
        self.sp("=");
        self.fmt_expr(value);
        self.sp("for");
        self.fmt_duration_expr(duration);
    }

    /// `cancel_stmt`
    fn fmt_cancel_stmt(&mut self) {
        self.sp("cancel");
        self.name();
    }

    /// `measure_stmt`
    fn fmt_measure_stmt(&mut self, value: &Expr) {
        self.sp("measure");
        self.name();
        self.sp("=");
        self.fmt_expr(value);
    }

    /// `job_stmt`
    fn fmt_job_stmt(&mut self, args: &[Arg]) {
        self.sp("job");
        self.name();
        self.sp("=");
        self.name();
        self.fmt_arg_list(args);
    }

    /// `arm_stmt`
    fn fmt_arm_stmt(&mut self) {
        self.name();
        self.name();
    }

    /// `verify_stmt`
    fn fmt_verify_stmt(&mut self, cond: &Expr, req: bool) {
        self.sp("verify");
        self.fmt_expr(cond);
        self.op(",");
        self.name();
        if req {
            self.sp("req");
            self.name();
        }
    }

    /// `verdict_stmt`
    fn fmt_verdict_stmt(&mut self, message: bool) {
        self.sp("verdict");
        self.name();
        if message {
            self.name();
        }
    }

    /// `raise_stmt`
    fn fmt_raise_stmt(&mut self) {
        self.sp("raise");
        self.name();
    }

    /// `goto_stmt`
    fn fmt_goto_stmt(&mut self) {
        self.sp("->");
        self.name();
    }

    /// `abort_stmt`
    fn fmt_abort_stmt(&mut self, message: bool) {
        self.sp("abort");
        if message {
            self.name();
        }
    }

    /// `return_stmt`
    fn fmt_return_stmt(&mut self, e: &Expr) {
        self.sp("return");
        self.fmt_expr(e);
    }

    // ------------------------------------------------------------ Ausdruecke

    /// `expr` bis `primary`, ueber den Baum.
    fn fmt_expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Number { unit, .. } => {
                self.fmt_number();
                if let Some(u) = unit {
                    self.fmt_unit_lit(u);
                }
            }
            ExprKind::Duration(_) => self.fmt_duration_lit(),
            ExprKind::Str(_) | ExprKind::Bool(_) | ExprKind::None | ExprKind::Default | ExprKind::Ident(_) => {
                self.name()
            }
            ExprKind::Call { generics, args, .. } => {
                self.name();
                if !generics.is_empty() {
                    self.fmt_generic_args(generics);
                }
                self.fmt_arg_list(args);
            }
            ExprKind::Upper { args, .. } | ExprKind::TypeName { args, .. } => {
                self.name();
                if let Some(args) = args {
                    self.fmt_arg_list(args);
                }
            }
            ExprKind::Paren(inner) => {
                self.sp("(");
                self.glue();
                self.fmt_expr(inner);
                self.op(")");
            }
            ExprKind::Tuple(a, b) => {
                self.sp("(");
                self.glue();
                self.fmt_expr(a);
                self.op(",");
                self.fmt_expr(b);
                self.op(")");
            }
            ExprKind::Array(items) => {
                self.sp("[");
                self.glue();
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        self.op(",");
                    }
                    self.fmt_expr(item);
                }
                self.op("]");
            }
            ExprKind::InstanceArray { count, args, .. } => {
                self.sp("[");
                self.glue();
                self.fmt_const_expr(count);
                self.op("]");
                self.name();
                self.fmt_arg_list(args);
            }
            ExprKind::Member { base, args, .. } => {
                self.fmt_expr(base);
                self.op(".");
                self.glue();
                self.fmt_member();
                if let Some(args) = args {
                    self.fmt_arg_list(args);
                }
            }
            ExprKind::Index { base, index } => {
                self.fmt_expr(base);
                self.op("[");
                self.glue();
                self.fmt_expr(index);
                self.op("]");
            }
            ExprKind::Slice { base, from, to } => {
                self.fmt_expr(base);
                self.op("[");
                self.glue();
                self.fmt_expr(from);
                self.op("..");
                self.glue();
                self.fmt_expr(to);
                self.op("]");
            }
            ExprKind::Index2 { base, row, col } => {
                self.fmt_expr(base);
                self.op("[");
                self.glue();
                self.fmt_expr(row);
                self.op(",");
                self.fmt_expr(col);
                self.op("]");
            }
            ExprKind::Cast { expr, ty } => {
                self.fmt_expr(expr);
                self.sp("as");
                self.fmt_scalar_type(ty);
            }
            ExprKind::Unary { op, expr } => {
                match op {
                    UnaryOp::Neg => {
                        self.sp("-");
                        self.glue();
                    }
                    UnaryOp::BitNot => {
                        self.sp("~");
                        self.glue();
                    }
                    UnaryOp::Not => self.sp("not"),
                }
                self.fmt_expr(expr);
            }
            ExprKind::Binary { op, lhs, rhs } => {
                self.fmt_expr(lhs);
                match op {
                    BinaryOp::Shr => {
                        self.sp(">");
                        self.op(">");
                    }
                    _ => self.sp(binary_op(*op)),
                }
                self.fmt_expr(rhs);
            }
            ExprKind::Match { subject, pattern, binding, .. } => {
                self.fmt_expr(subject);
                self.name();
                self.fmt_pattern(pattern);
                if binding.is_some() {
                    self.sp("as");
                    self.name();
                }
            }
            ExprKind::Conditional { then, cond, otherwise } => {
                self.fmt_expr(then);
                self.sp("if");
                self.fmt_expr(cond);
                self.sp("else");
                self.fmt_expr(otherwise);
            }
            ExprKind::Temporal { window, inner, .. } => {
                self.name();
                if window.is_some() {
                    self.op("[");
                    self.glue();
                    self.fmt_duration_lit();
                    self.op("]");
                }
                self.op("(");
                self.glue();
                self.fmt_expr(inner);
                self.op(")");
            }
            ExprKind::Implies { lhs, rhs } => {
                self.fmt_expr(lhs);
                self.sp("implies");
                self.fmt_expr(rhs);
            }
        }
    }

    /// `member`
    fn fmt_member(&mut self) {
        self.name();
    }

    /// `generic_args`
    fn fmt_generic_args(&mut self, args: &[GenericArg]) {
        self.op("[");
        self.glue();
        for (i, a) in args.iter().enumerate() {
            if i > 0 {
                self.op(",");
            }
            self.fmt_generic_arg(a);
        }
        self.op("]");
    }

    /// `generic_arg`
    fn fmt_generic_arg(&mut self, a: &GenericArg) {
        match a {
            GenericArg::Unit(u) => self.fmt_unit_expr(u),
            GenericArg::Type(t) => self.fmt_type(t),
            GenericArg::Const(c) => self.fmt_const_expr(c),
        }
    }

    /// `"(" [ args ] ")"`
    fn fmt_arg_list(&mut self, args: &[Arg]) {
        self.op("(");
        self.glue();
        self.fmt_args(args);
        self.op(")");
    }

    /// `args`
    fn fmt_args(&mut self, args: &[Arg]) {
        for (i, a) in args.iter().enumerate() {
            if i > 0 {
                self.op(",");
            }
            self.fmt_arg(a);
        }
    }

    /// `arg`
    fn fmt_arg(&mut self, a: &Arg) {
        if a.name.is_some() {
            self.name();
            self.sp("=");
        }
        self.fmt_expr(&a.value);
    }

    /// `number`
    fn fmt_number(&mut self) {
        self.name();
    }

    /// `int_lit`
    fn fmt_int_lit(&mut self) {
        self.name();
    }

    /// `duration_lit`
    fn fmt_duration_lit(&mut self) {
        self.name();
    }

    /// `duration_expr`
    fn fmt_duration_expr(&mut self, e: &Expr) {
        self.fmt_expr(e);
    }

    /// `const_expr`
    fn fmt_const_expr(&mut self, e: &Expr) {
        self.fmt_expr(e);
    }

    /// `pattern`
    fn fmt_pattern(&mut self, p: &Pattern) {
        match p {
            Pattern::Text(_) => self.name(),
            Pattern::Record { fields, .. } => {
                self.name();
                self.op("(");
                self.glue();
                for (i, (_, value)) in fields.iter().enumerate() {
                    if i > 0 {
                        self.op(",");
                    }
                    self.name();
                    self.sp("=");
                    self.fmt_const_expr(value);
                }
                self.op(")");
            }
        }
    }

    /// `unit_lit`: Einheit nach einer Zahl, genau ein Leerzeichen davor.
    fn fmt_unit_lit(&mut self, u: &UnitExpr) {
        self.fmt_unit_expr(u);
    }

    /// `unit_expr`: kompakt, ohne Leerraum zwischen den Termen.
    fn fmt_unit_expr(&mut self, u: &UnitExpr) {
        if u.first.name.is_none() {
            self.name(); // die "1"
        } else {
            self.fmt_unit_term(&u.first);
        }
        for (op, term) in &u.rest {
            self.op(match op {
                UnitOp::Mul => "*",
                UnitOp::Div => "/",
            });
            self.glue();
            self.fmt_unit_term(term);
        }
    }

    /// `unit_term`
    fn fmt_unit_term(&mut self, t: &UnitTerm) {
        self.name();
        if t.exponent.is_some() {
            self.op("^");
            self.glue();
            self.name();
        }
    }

    // ------------------------------------------------------------ Typausdruecke

    /// `type`
    fn fmt_type(&mut self, t: &Type) {
        match &t.kind {
            TypeKind::Scalar { scalar, range, wrap } => {
                self.fmt_scalar_type(scalar);
                if let Some(r) = range {
                    self.sp("in");
                    self.fmt_range(r);
                }
                self.fmt_wrap(wrap);
            }
            TypeKind::Wrapped { inner, wrap } => {
                self.fmt_type(inner);
                self.fmt_wrap(&Some(wrap.clone()));
            }
            TypeKind::Array { len, elem } => {
                self.sp("[");
                self.glue();
                self.fmt_const_expr(len);
                self.op("]");
                self.fmt_type(elem);
            }
            TypeKind::Named { wrap, .. } | TypeKind::TypeVar { wrap, .. } => {
                self.name();
                self.fmt_wrap(wrap);
            }
            TypeKind::Bytes(n) | TypeKind::Line(n) => {
                self.name();
                self.op("<");
                self.glue();
                self.fmt_const_expr(n);
                self.op(">");
            }
            TypeKind::Vec { elem, len } | TypeKind::Samples { elem, len } => {
                self.name();
                self.op("<");
                self.glue();
                self.fmt_type(elem);
                self.op(",");
                self.fmt_const_expr(len);
                self.op(">");
            }
            TypeKind::Stream(elem) => {
                self.name();
                self.op("<");
                self.glue();
                self.fmt_elem_type(elem);
                self.op(">");
            }
            TypeKind::Table { key, value } => {
                self.name();
                self.op("<");
                self.glue();
                self.fmt_type(key);
                self.op(",");
                self.fmt_type(value);
                self.op(">");
            }
            TypeKind::Mat { rows, cols, unit } => {
                self.name();
                self.op("<");
                self.glue();
                self.fmt_const_expr(rows);
                self.op(",");
                self.fmt_const_expr(cols);
                self.op(">");
                if let Some(u) = unit {
                    self.op("[");
                    self.glue();
                    self.fmt_unit_expr(u);
                    self.op("]");
                }
            }
            TypeKind::MatDim { rows, cols } => {
                self.name();
                self.op("[");
                self.glue();
                self.fmt_unit_tuple(rows);
                self.op(",");
                self.fmt_unit_tuple(cols);
                self.op("]");
            }
            TypeKind::VecDim(t) => {
                self.name();
                self.op("[");
                self.glue();
                self.fmt_unit_tuple(t);
                self.op("]");
            }
            TypeKind::Map { key, value, len } => {
                self.name();
                self.op("<");
                self.glue();
                self.fmt_type(key);
                self.op(",");
                self.fmt_type(value);
                self.op(",");
                self.fmt_const_expr(len);
                self.op(">");
            }
        }
    }

    /// `[ "?" | "!" TYPE_IDENT ]`
    fn fmt_wrap(&mut self, w: &Option<Wrap>) {
        match w {
            None => {}
            Some(Wrap::Optional) => self.op("?"),
            Some(Wrap::Result(_)) => {
                self.op("!");
                self.glue();
                self.name();
            }
        }
    }

    /// `scalar_type`
    fn fmt_scalar_type(&mut self, s: &ScalarType) {
        match s {
            ScalarType::Bool | ScalarType::Duration => self.name(),
            ScalarType::Int { unit, .. } => {
                self.fmt_int_type();
                self.fmt_bracket_unit(unit);
            }
            ScalarType::Float { unit, .. } => {
                self.name();
                self.fmt_bracket_unit(unit);
            }
            ScalarType::Str(n) => {
                self.name();
                self.op("<");
                self.glue();
                self.fmt_const_expr(n);
                self.op(">");
            }
        }
    }

    fn fmt_bracket_unit(&mut self, unit: &Option<UnitExpr>) {
        if let Some(u) = unit {
            self.op("[");
            self.glue();
            self.fmt_unit_expr(u);
            self.op("]");
        }
    }

    /// `int_type`
    fn fmt_int_type(&mut self) {
        self.name();
    }

    /// `unit_tuple`
    fn fmt_unit_tuple(&mut self, t: &UnitTuple) {
        match t {
            UnitTuple::Named(_) => self.name(),
            UnitTuple::Inverse(_) => {
                self.name();
                self.op("/");
                self.glue();
                self.name();
            }
            UnitTuple::Literal(units) => {
                self.sp("(");
                self.glue();
                for (i, u) in units.iter().enumerate() {
                    if i > 0 {
                        self.op(",");
                    }
                    self.fmt_unit_expr(u);
                }
                self.op(")");
            }
        }
    }

    /// `elem_type`
    fn fmt_elem_type(&mut self, e: &ElemType) {
        match e {
            ElemType::U8 | ElemType::Edge | ElemType::Named(_) => self.name(),
            ElemType::Bytes(n) | ElemType::Line(n) => {
                self.name();
                self.op("<");
                self.glue();
                self.fmt_const_expr(n);
                self.op(">");
            }
            ElemType::Capture { elem, len } => {
                self.name();
                self.op("<");
                self.glue();
                self.fmt_type(elem);
                self.op(",");
                self.fmt_const_expr(len);
                self.op(">");
            }
        }
    }

    /// `range`
    fn fmt_range(&mut self, r: &Range) {
        self.fmt_const_expr(&r.from);
        self.op("..");
        self.glue();
        self.fmt_const_expr(&r.to);
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
