//! Deklarationen auf Dateiebene (`file` und die *_decl-Produktionen ausser Maschinen).

use super::{PResult, Parser};
use crate::ast::*;
use crate::token::TokenKind;

const ITEM_KEYWORDS: &[&str] = &[
    "import",
    "system",
    "type",
    "unitvec",
    "enum",
    "record",
    "unit",
    "stream",
    "port",
    "node",
    "property",
    "assumption",
    "const",
    "param",
    "tunable",
    "profile",
    "input",
    "output",
    "command",
    "fn",
    "native",
    "block",
    "machine",
    "driver",
    "instance",
    "scenario",
    "campaign",
    "trigger",
];

impl<'t, 's> Parser<'t, 's> {
    /// `file := { NEWLINE | import | … | trigger_decl }`
    pub(super) fn parse_file(&mut self) -> File {
        let mut file = File::default();
        while !self.at(TokenKind::Eof) {
            if self.eat(TokenKind::Newline) {
                continue;
            }
            let start = self.pos;
            match self.parse_item() {
                Ok(item) => file.items.push(item),
                Err(e) => {
                    self.report(e);
                    self.recover(start);
                }
            }
        }
        file
    }

    pub(super) fn at_item_start(&self) -> bool {
        self.kind() == TokenKind::Keyword && ITEM_KEYWORDS.contains(&self.text())
    }

    pub(super) fn parse_item(&mut self) -> PResult<Item> {
        if self.kind() != TokenKind::Keyword {
            return Err(self.error_at(
                self.tok(),
                format!("erwartet eine Deklaration, gefunden {}", self.describe(self.tok())),
                Some("auf Dateiebene stehen system, Typen, Channels, Parameter, fn, block, machine und Instanzen"),
            ));
        }
        Ok(match self.text() {
            "import" => Item::Import(self.parse_import()?),
            "system" => Item::System(self.parse_system_decl()?),
            "type" => Item::Type(self.parse_type_decl()?),
            "unitvec" => Item::Unitvec(self.parse_unitvec_decl()?),
            "enum" => Item::Enum(self.parse_enum_decl()?),
            "record" => Item::Record(self.parse_record_decl()?),
            "unit" => Item::Unit(self.parse_unit_decl()?),
            "stream" => Item::Stream(self.parse_stream_decl()?),
            "port" => Item::Port(self.parse_port_decl()?),
            "node" => Item::Node(self.parse_node_decl()?),
            "property" => Item::Property(self.parse_property_decl()?),
            "assumption" => Item::Property(self.parse_assumption_decl()?),
            "const" => Item::Const(self.parse_const_decl()?),
            "param" | "tunable" => Item::Param(self.parse_param_decl()?),
            "profile" => Item::Profile(self.parse_profile_decl()?),
            "input" | "output" => Item::Channel(self.parse_channel_decl()?),
            "command" => Item::Command(self.parse_command_decl()?),
            "fn" => Item::Fn(self.parse_fn_decl()?),
            "native" => Item::Native(self.parse_native_decl()?),
            "block" => Item::Block(self.parse_block_decl()?),
            "machine" | "driver" => Item::Machine(self.parse_machine_decl()?),
            "instance" => Item::Instance(self.parse_instance_decl()?),
            "scenario" => Item::Scenario(self.parse_scenario_decl()?),
            "campaign" => Item::Campaign(self.parse_campaign_decl()?),
            "trigger" => Item::Trigger(self.parse_trigger_decl()?),
            other => {
                // Wer `pub var` oder `signal` auf Dateiebene schreibt, sucht
                // die Maschinenebene (5.8) — der Hinweis nennt sie, statt nur
                // aufzuzaehlen, was hier erlaubt waere.
                let hint = match other {
                    "pub" | "var" | "persist" => "`var`, `pub var` und `persist var` stehen in einer Maschine (5.1)",
                    "signal" => "`signal` steht in einer Maschine (5.8)",
                    "loop" | "state" | "initial" | "enter" | "exit" | "on" | "sequence" => {
                        "das gehoert in eine Maschine (2.3: machine_body)"
                    }
                    _ => "auf Dateiebene stehen system, Typen, Channels, Parameter, fn, block, machine und Instanzen",
                };
                return Err(self.error_at(
                    self.tok(),
                    format!("`{other}` kann keine Deklaration einleiten"),
                    Some(hint),
                ));
            }
        })
    }

    /// `import`
    fn parse_import(&mut self) -> PResult<Import> {
        let start = self.pos;
        self.expect_kw("import")?;
        if self.at_word("channels") && self.at_word_at(1, "from") {
            self.bump();
            self.bump();
            let file = self.string()?;
            self.expect_newline()?;
            return Ok(Import::Channels { file, span: self.span_from(start) });
        }
        let mut path = vec![self.ident()?];
        while self.eat_op(".") {
            path.push(self.ident()?);
        }
        let alias = if self.eat_kw("as") { Some(self.ident()?) } else { None };
        self.expect_newline()?;
        Ok(Import::Module { path, alias, span: self.span_from(start) })
    }

    /// `system_decl`
    fn parse_system_decl(&mut self) -> PResult<SystemDecl> {
        let start = self.pos;
        self.expect_kw("system")?;
        self.expect_op(":")?;
        self.expect_newline()?;
        self.expect_indent()?;
        let mut items = Vec::new();
        while !self.at(TokenKind::Dedent) && !self.at(TokenKind::Eof) {
            let item_start = self.pos;
            match self.parse_system_item() {
                Ok(item) => items.push(item),
                Err(e) => {
                    self.report(e);
                    self.recover(item_start);
                }
            }
        }
        self.expect_dedent()?;
        Ok(SystemDecl { items, span: self.span_from(start) })
    }

    /// `system_item`
    fn parse_system_item(&mut self) -> PResult<SystemItem> {
        if !Self::is_word(self.kind()) {
            return Err(self.error_here("einen Systemeintrag wie `tick = 1 ms`"));
        }
        let name_tok = self.bump();
        let name = self.text_of(name_tok);
        self.expect_op("=")?;
        let item = match name {
            "tick" => SystemItem::Tick(self.parse_duration_lit()?),
            "output_timing" => {
                let v = if self.eat_word("asap") {
                    OutputTiming::Asap
                } else if self.eat_word("boundary") {
                    OutputTiming::Boundary
                } else {
                    return Err(self.error_here("`asap` oder `boundary`"));
                };
                SystemItem::OutputTiming(v)
            }
            "fault_is_fail" => SystemItem::FaultIsFail(self.parse_bool_word()?),
            "tick_source" => {
                self.expect_word("hw")?;
                self.expect_op("(")?;
                let s = self.string()?;
                self.expect_op(")")?;
                SystemItem::TickSource(s)
            }
            "tick_tolerance" => {
                let value = self.parse_const_expr()?;
                let ticks = if self.eat_kw("for") {
                    let n = self.parse_int_lit()?;
                    self.expect_word("ticks")?;
                    Some(n)
                } else {
                    None
                };
                SystemItem::TickTolerance { value, ticks }
            }
            "target" => SystemItem::Target(self.ident()?),
            "float" => {
                let w = if self.eat_word("f32") {
                    FloatWidth::F32
                } else if self.eat_word("f64") {
                    FloatWidth::F64
                } else {
                    return Err(self.error_here("`f32` oder `f64`"));
                };
                SystemItem::Float(w)
            }
            "language" => SystemItem::Language(self.int_token()?),
            other => {
                return Err(self.error_at(
                    name_tok,
                    format!("unbekannter Systemeintrag `{other}`"),
                    Some(
                        "Eintraege: tick output_timing fault_is_fail tick_source tick_tolerance target float language",
                    ),
                ));
            }
        };
        self.expect_newline()?;
        Ok(item)
    }

    /// `type_decl`
    fn parse_type_decl(&mut self) -> PResult<TypeDecl> {
        let start = self.pos;
        self.expect_kw("type")?;
        let name = self.type_ident()?;
        self.expect_op("=")?;
        let ty = self.parse_type()?;
        self.expect_newline()?;
        Ok(TypeDecl { name, ty, span: self.span_from(start) })
    }

    /// `unitvec_decl`
    fn parse_unitvec_decl(&mut self) -> PResult<UnitvecDecl> {
        let start = self.pos;
        self.expect_kw("unitvec")?;
        let name = self.upper()?;
        self.expect_op("=")?;
        self.expect_op("(")?;
        let mut units = vec![self.parse_unit_expr(false)?];
        while self.eat_op(",") {
            units.push(self.parse_unit_expr(false)?);
        }
        self.expect_op(")")?;
        self.expect_newline()?;
        Ok(UnitvecDecl { name, units, span: self.span_from(start) })
    }

    /// `enum_decl` (einzeilig oder als Block)
    fn parse_enum_decl(&mut self) -> PResult<EnumDecl> {
        let start = self.pos;
        self.expect_kw("enum")?;
        let name = self.type_ident()?;
        let layout = if self.eat_word("layout") { Some(self.parse_int_type()?) } else { None };
        let open = self.eat_word("open");
        self.expect_op(":")?;
        let mut variants = Vec::new();
        if self.eat(TokenKind::Newline) {
            self.expect_indent()?;
            while !self.at(TokenKind::Dedent) && !self.at(TokenKind::Eof) {
                variants.push(self.parse_variant()?);
                self.expect_newline()?;
            }
            self.expect_dedent()?;
        } else {
            loop {
                variants.push(self.parse_variant()?);
                if !self.eat_op(",") {
                    break;
                }
            }
            self.expect_newline()?;
        }
        Ok(EnumDecl { name, layout, open, variants, span: self.span_from(start) })
    }

    /// `variant`
    fn parse_variant(&mut self) -> PResult<Variant> {
        let start = self.pos;
        let name = self.upper()?;
        let discriminant = if self.eat_op("=") { Some(self.parse_int_lit()?) } else { None };
        let mut fields = Vec::new();
        if self.eat_op("(") {
            loop {
                fields.push(self.parse_field()?);
                if !self.eat_op(",") {
                    break;
                }
            }
            self.expect_op(")")?;
        }
        Ok(Variant { name, discriminant, fields, span: self.span_from(start) })
    }

    /// `record_decl`
    fn parse_record_decl(&mut self) -> PResult<RecordDecl> {
        let start = self.pos;
        self.expect_kw("record")?;
        let name = self.type_ident()?;
        let layout = if self.eat_word("layout") {
            let endian = if self.eat_word("little") {
                Endian::Little
            } else if self.eat_word("big") {
                Endian::Big
            } else {
                return Err(self.error_here("`little` oder `big`"));
            };
            let align = if self.at_op(",") && self.at_word_at(1, "align") {
                self.bump();
                self.bump();
                self.expect_op("=")?;
                Some(self.parse_int_lit()?)
            } else {
                None
            };
            Some(Layout { endian, align })
        } else {
            None
        };
        self.expect_op(":")?;
        self.expect_newline()?;
        self.expect_indent()?;
        let mut fields = Vec::new();
        while !self.at(TokenKind::Dedent) && !self.at(TokenKind::Eof) {
            let field_start = self.pos;
            match self.parse_record_field() {
                Ok(f) => fields.push(f),
                Err(e) => {
                    self.report(e);
                    self.recover(field_start);
                }
            }
        }
        self.expect_dedent()?;
        Ok(RecordDecl { name, layout, fields, span: self.span_from(start) })
    }

    /// `record_field := field NEWLINE | IDENT ":" int_type "with" "bits" ":" NEWLINE INDENT { bitfield NEWLINE } DEDENT`
    fn parse_record_field(&mut self) -> PResult<RecordField> {
        let start = self.pos;
        let is_bits = self.at(TokenKind::Ident)
            && self.at_op_at(1, ":")
            && self.tok_at(3).kind == TokenKind::Keyword
            && self.text_of(self.tok_at(3)) == "with"
            && self.at_word_at(4, "bits");
        if is_bits {
            let name = self.ident()?;
            self.expect_op(":")?;
            let ty = self.parse_int_type()?;
            self.expect_kw("with")?;
            self.expect_word("bits")?;
            self.expect_op(":")?;
            self.expect_newline()?;
            self.expect_indent()?;
            let mut bits = Vec::new();
            while !self.at(TokenKind::Dedent) && !self.at(TokenKind::Eof) {
                bits.push(self.parse_bitfield()?);
                self.expect_newline()?;
            }
            self.expect_dedent()?;
            return Ok(RecordField::Bits { name, ty, bits, span: self.span_from(start) });
        }
        let field = self.parse_field()?;
        self.expect_newline()?;
        Ok(RecordField::Plain(field))
    }

    /// `field`
    fn parse_field(&mut self) -> PResult<Field> {
        let start = self.pos;
        let name = if self.at(TokenKind::Wild) {
            let t = self.bump();
            self.ident_of(t)
        } else {
            self.ident()?
        };
        self.expect_op(":")?;
        let ty = self.parse_type()?;
        let value = if self.eat_op("=") { Some(self.parse_const_expr()?) } else { None };
        let offset = if self.eat_word("offset") {
            self.expect_op("=")?;
            Some(self.parse_int_lit()?)
        } else {
            None
        };
        let len_field = if self.at_kw("with") && self.at_word_at(1, "len") {
            self.bump();
            self.bump();
            self.expect_op("=")?;
            Some(self.ident()?)
        } else {
            None
        };
        Ok(Field { name, ty, value, offset, len_field, span: self.span_from(start) })
    }

    /// `bitfield`
    fn parse_bitfield(&mut self) -> PResult<Bitfield> {
        let start = self.pos;
        let name = self.ident()?;
        self.expect_op(":")?;
        let ty = if self.eat_word("bool") { BitType::Bool } else { BitType::Int(self.parse_int_type()?) };
        self.expect_kw("at")?;
        let from = self.parse_int_lit()?;
        let to = if self.eat_op("..") { Some(self.parse_int_lit()?) } else { None };
        Ok(Bitfield { name, ty, from, to, span: self.span_from(start) })
    }

    /// `unit_decl`
    fn parse_unit_decl(&mut self) -> PResult<UnitDecl> {
        let start = self.pos;
        self.expect_kw("unit")?;
        let name = self.parse_unit_name()?;
        self.expect_op("=")?;
        if self.eat_word("affine") {
            self.expect_op("(")?;
            let base = self.parse_unit_expr(false)?;
            self.expect_op(",")?;
            let offset = self.parse_number()?;
            self.expect_op(")")?;
            self.expect_newline()?;
            return Ok(UnitDecl::Affine { name, base, offset, span: self.span_from(start) });
        }
        let factor = self.parse_number()?;
        let unit = if self.at_unit_start() { Some(self.parse_unit_lit()?) } else { None };
        self.expect_newline()?;
        Ok(UnitDecl::Scaled { name, factor, unit, span: self.span_from(start) })
    }

    /// `const_decl`
    fn parse_const_decl(&mut self) -> PResult<ConstDecl> {
        let start = self.pos;
        self.expect_kw("const")?;
        let name = self.upper()?;
        let ty = if self.eat_op(":") { Some(self.parse_type()?) } else { None };
        self.expect_op("=")?;
        let value = self.parse_const_expr()?;
        self.expect_newline()?;
        Ok(ConstDecl { name, ty, value, span: self.span_from(start) })
    }

    /// `param_decl`
    fn parse_param_decl(&mut self) -> PResult<ParamDecl> {
        let start = self.pos;
        let tunable = self.eat_kw("tunable");
        self.expect_kw("param")?;
        let name = self.upper()?;
        self.expect_op(":")?;
        let ty = self.parse_type()?;
        self.expect_op("=")?;
        let value = self.parse_const_expr()?;
        let attrs = self.parse_with_attrs()?;
        self.expect_newline()?;
        Ok(ParamDecl { tunable, name, ty, value, attrs, span: self.span_from(start) })
    }

    /// `profile_decl`
    fn parse_profile_decl(&mut self) -> PResult<ProfileDecl> {
        let start = self.pos;
        self.expect_kw("profile")?;
        let name = self.upper()?;
        self.expect_op(":")?;
        self.expect_newline()?;
        self.expect_indent()?;
        let mut entries = Vec::new();
        while !self.at(TokenKind::Dedent) && !self.at(TokenKind::Eof) {
            let param = self.upper()?;
            self.expect_op("=")?;
            let value = self.parse_const_expr()?;
            self.expect_newline()?;
            entries.push((param, value));
        }
        self.expect_dedent()?;
        Ok(ProfileDecl { name, entries, span: self.span_from(start) })
    }

    /// `channel_decl`
    fn parse_channel_decl(&mut self) -> PResult<ChannelDecl> {
        let start = self.pos;
        let dir = if self.eat_kw("input") {
            Direction::Input
        } else {
            self.expect_kw("output")?;
            Direction::Output
        };
        let name = self.ident()?;
        self.expect_op(":")?;
        let ty = self.parse_type()?;
        self.expect_op("@")?;
        let binding = self.parse_binding()?;
        let attrs = self.parse_with_attrs()?;
        self.expect_newline()?;
        Ok(ChannelDecl { dir, name, ty, binding, attrs, span: self.span_from(start) })
    }

    /// `stream_decl`
    fn parse_stream_decl(&mut self) -> PResult<StreamDecl> {
        let start = self.pos;
        self.expect_kw("stream")?;
        self.expect_op("<")?;
        let elem = self.parse_elem_type()?;
        self.expect_op(">")?;
        let name = self.ident()?;
        if !self.at_kw("with") {
            return Err(self.error_here("`with capacity = N` (ein interner Stream nennt seine Kapazitaet)"));
        }
        let attrs = self.parse_with_attrs()?;
        self.expect_newline()?;
        Ok(StreamDecl { elem, name, attrs, span: self.span_from(start) })
    }

    /// `port_decl`
    fn parse_port_decl(&mut self) -> PResult<PortDecl> {
        let start = self.pos;
        self.expect_kw("port")?;
        let name = self.ident()?;
        self.expect_op(":")?;
        let regs = self.type_ident()?;
        self.expect_op("@")?;
        self.expect_word("mmio")?;
        self.expect_op("(")?;
        let t = self.expect(TokenKind::Hex, "eine Hex-Adresse wie `0x40000000`")?;
        let address = IntLit { text: self.text_of(t).to_string(), span: Span::new(t.start, t.end) };
        self.expect_op(")")?;
        self.expect_newline()?;
        Ok(PortDecl { name, regs, address, span: self.span_from(start) })
    }

    /// `command_decl`
    fn parse_command_decl(&mut self) -> PResult<CommandDecl> {
        let start = self.pos;
        self.expect_kw("command")?;
        let name = self.ident()?;
        let attrs = self.parse_with_attrs()?;
        self.expect_newline()?;
        Ok(CommandDecl { name, attrs, span: self.span_from(start) })
    }

    /// `fn_decl`
    fn parse_fn_decl(&mut self) -> PResult<FnDecl> {
        let start = self.pos;
        self.expect_kw("fn")?;
        let name = self.ident()?;
        let generics = self.parse_generic_vars()?;
        let params = self.parse_param_list()?;
        let ret = if self.eat_op("->") { Some(self.parse_type()?) } else { None };
        if ret.is_none() && !params.iter().any(|p| p.inout) {
            let hint = if self.arrow_on_next_line() {
                "`->` setzt die Zeile nicht fort; innerhalb der Parameterliste umbrechen, dort traegt die Klammer (2.1)"
            } else {
                "ohne Rueckgabetyp nur mit einem inout-Parameter (3.9)"
            };
            return Err(self.error_at(self.tok(), "erwartet `->` mit Rueckgabetyp", Some(hint)));
        }
        self.expect_op(":")?;
        let body = self.parse_block()?;
        Ok(FnDecl { name, generics, params, ret, body, span: self.span_from(start) })
    }

    /// Steht hinter dem Zeilenende ein `->`?
    ///
    /// `->` setzt eine Zeile nicht fort (2.1), weil `-> ZIEL` eine eigene
    /// Zeile bildet. Eine Signatur, die davor umbricht, laeuft darum in
    /// eine Meldung, deren Vorschlag nicht passt — diese Frage trennt die
    /// beiden Faelle.
    fn arrow_on_next_line(&self) -> bool {
        let mut n = 0;
        while matches!(self.tok_at(n).kind, TokenKind::Newline | TokenKind::Indent | TokenKind::Dedent) {
            n += 1;
        }
        n > 0 && self.at_op_at(n, "->")
    }

    /// `native_decl`
    fn parse_native_decl(&mut self) -> PResult<NativeDecl> {
        let start = self.pos;
        self.expect_kw("native")?;
        let kind = if self.eat_kw("fn") {
            NativeKind::Fn
        } else {
            self.expect_kw("job")?;
            NativeKind::Job
        };
        let name = self.ident()?;
        let generics = self.parse_generic_vars()?;
        let params = self.parse_param_list()?;
        self.expect_op("->")?;
        let ret = self.parse_type()?;
        let from = if self.eat_word("from") { Some(self.string()?) } else { None };
        self.expect_kw("with")?;
        self.expect_word("cost")?;
        self.expect_op("=")?;
        let cost = self.parse_cost_spec()?;
        self.expect_op(",")?;
        self.expect_word("stack")?;
        self.expect_op("=")?;
        let stack = self.int_token()?;
        let duration = if self.at_op(",") && self.at_word_at(1, "duration") {
            self.bump();
            self.bump();
            self.expect_op("=")?;
            Some(self.parse_duration_lit()?)
        } else {
            None
        };
        self.expect_op(",")?;
        self.expect_word("total")?;
        self.expect_newline()?;
        Ok(NativeDecl { kind, name, generics, params, ret, from, cost, stack, duration, span: self.span_from(start) })
    }

    /// `cost_spec`
    fn parse_cost_spec(&mut self) -> PResult<CostSpec> {
        if self.eat_op("{") {
            let mut classes = Vec::new();
            loop {
                let class = self.parse_cost_class()?;
                self.expect_op(":")?;
                let value = self.parse_int_lit()?;
                classes.push((class, value));
                if !self.eat_op(",") {
                    break;
                }
            }
            self.expect_op("}")?;
            return Ok(CostSpec::Classes(classes));
        }
        Ok(CostSpec::Single(self.parse_int_lit()?))
    }

    /// `cost_class`
    fn parse_cost_class(&mut self) -> PResult<CostClass> {
        let class = if Self::is_word(self.kind()) {
            match self.text() {
                "i32" => CostClass::I32,
                "i64" => CostClass::I64,
                "f32" => CostClass::F32,
                "f64" => CostClass::F64,
                "mem" => CostClass::Mem,
                "call" => CostClass::Call,
                "native" => CostClass::Native,
                _ => return Err(self.error_here("eine Kostenklasse: i32 i64 f32 f64 mem call native")),
            }
        } else {
            return Err(self.error_here("eine Kostenklasse: i32 i64 f32 f64 mem call native"));
        };
        self.bump();
        Ok(class)
    }

    /// `block_decl`
    fn parse_block_decl(&mut self) -> PResult<BlockDecl> {
        let start = self.pos;
        self.expect_kw("block")?;
        let name = self.ident()?;
        let generics = self.parse_generic_vars()?;
        let params = self.parse_param_list()?;
        self.expect_op(":")?;
        self.expect_newline()?;
        self.expect_indent()?;
        let mut vars = Vec::new();
        let mut step = None;
        let mut methods = Vec::new();
        while !self.at(TokenKind::Dedent) && !self.at(TokenKind::Eof) {
            let item_start = self.pos;
            let result: PResult<()> = (|| {
                if self.at_kw("var") || self.at_kw("pub") {
                    if step.is_some() || !methods.is_empty() {
                        return Err(self.error_at(
                            self.tok(),
                            "Variablen stehen vor `step` und den Methoden",
                            Some("Reihenfolge im Block: var, step, weitere Methoden (2.3: block_decl)"),
                        ));
                    }
                    let v = self.parse_var_decl()?;
                    self.expect_newline()?;
                    vars.push(v);
                } else if self.at_kw("step") && self.at_op_at(1, "(") {
                    if step.is_some() {
                        return Err(self.error_at(self.tok(), "`step` ist doppelt", None));
                    }
                    if !methods.is_empty() {
                        return Err(self.error_at(
                            self.tok(),
                            "`step` steht vor den Methoden",
                            Some("2.3: block_decl"),
                        ));
                    }
                    step = Some(self.parse_step_decl()?);
                } else if self.at(TokenKind::Ident) && self.at_op_at(1, "(") {
                    methods.push(self.parse_method_decl()?);
                } else {
                    return Err(self.error_here("`var`, `step(…)` oder eine Methode `name(…)`"));
                }
                Ok(())
            })();
            if let Err(e) = result {
                self.report(e);
                self.recover(item_start);
            }
        }
        if step.is_none() && methods.is_empty() {
            return Err(self.error_at(self.tok(), "ein Block braucht `step(…)` oder eine Methode", None));
        }
        self.expect_dedent()?;
        Ok(BlockDecl { name, generics, params, vars, step, methods, span: self.span_from(start) })
    }

    /// `step_decl`
    pub(super) fn parse_step_decl(&mut self) -> PResult<StepDecl> {
        let start = self.pos;
        self.expect_kw("step")?;
        let params = self.parse_param_list()?;
        self.expect_op("->")?;
        let ret = self.parse_type()?;
        self.expect_op(":")?;
        let body = self.parse_block()?;
        Ok(StepDecl { params, ret, body, span: self.span_from(start) })
    }

    /// `method_decl`
    fn parse_method_decl(&mut self) -> PResult<MethodDecl> {
        let start = self.pos;
        let name = self.ident()?;
        let params = self.parse_param_list()?;
        let ret = if self.eat_op("->") { Some(self.parse_type()?) } else { None };
        self.expect_op(":")?;
        let body = self.parse_block()?;
        Ok(MethodDecl { name, params, ret, body, span: self.span_from(start) })
    }

    /// `instance_decl`
    pub(super) fn parse_instance_decl(&mut self) -> PResult<InstanceDecl> {
        let start = self.pos;
        self.expect_kw("instance")?;
        let name = self.ident()?;
        let index = if self.eat_op("[") {
            let var = self.ident()?;
            self.expect_kw("in")?;
            let range = self.parse_range()?;
            self.expect_op("]")?;
            Some((var, range))
        } else {
            None
        };
        let resume = self.eat_word("resume");
        self.expect_op("=")?;
        let template = self.ident()?;
        let args = self.parse_arg_list()?;
        self.expect_newline()?;
        Ok(InstanceDecl { name, index, resume, template, args, span: self.span_from(start) })
    }

    /// `node_decl`
    fn parse_node_decl(&mut self) -> PResult<NodeDecl> {
        let start = self.pos;
        self.expect_kw("node")?;
        let name = self.ident()?;
        self.expect_op("@")?;
        self.expect_word("hw")?;
        self.expect_op("(")?;
        let address = self.string()?;
        self.expect_op(")")?;
        let tick = if self.eat_kw("with") {
            self.expect_word("tick")?;
            self.expect_op("=")?;
            Some(self.parse_duration_lit()?)
        } else {
            None
        };
        self.expect_newline()?;
        Ok(NodeDecl { name, address, tick, span: self.span_from(start) })
    }

    /// `property_decl`
    fn parse_property_decl(&mut self) -> PResult<PropertyDecl> {
        self.parse_property_like(PropertyKind::Property)
    }

    /// `assumption_decl` — dieselbe Form wie `property_decl` (13.3).
    fn parse_assumption_decl(&mut self) -> PResult<PropertyDecl> {
        self.parse_property_like(PropertyKind::Assumption)
    }

    fn parse_property_like(&mut self, kind: PropertyKind) -> PResult<PropertyDecl> {
        let start = self.pos;
        self.expect_kw(kind.word())?;
        let name = self.ident()?;
        self.expect_op(":")?;
        let prop = self.parse_tprop()?;
        if self.at_kw("if") {
            return Err(self.error_at(
                self.tok(),
                "in einer Eigenschaft gibt es keine Bedingungsform",
                Some("Bedingung als Vergleich oder in einem Aufruf schreiben (2.3: tprop_atom)"),
            ));
        }
        let monitor = if self.eat_kw("with") {
            self.expect_word("monitor")?;
            self.expect_op("=")?;
            self.expect_kw("true")?;
            true
        } else {
            false
        };
        self.expect_newline()?;
        Ok(PropertyDecl { kind, name, prop, monitor, span: self.span_from(start) })
    }

    /// `scenario_decl`
    fn parse_scenario_decl(&mut self) -> PResult<ScenarioDecl> {
        let start = self.pos;
        self.expect_kw("scenario")?;
        let name = self.string()?;
        let every = if self.eat_kw("every") { Some(self.parse_duration_lit()?) } else { None };
        self.expect_op(":")?;
        self.expect_newline()?;
        self.expect_indent()?;
        let body = self.parse_machine_body()?;
        self.expect_dedent()?;
        Ok(ScenarioDecl { name, every, body, span: self.span_from(start) })
    }

    /// `campaign_decl`
    fn parse_campaign_decl(&mut self) -> PResult<CampaignDecl> {
        let start = self.pos;
        self.expect_kw("campaign")?;
        let name = self.ident()?;
        self.expect_op(":")?;
        self.expect_newline()?;
        self.expect_indent()?;
        let mut items = Vec::new();
        while !self.at(TokenKind::Dedent) && !self.at(TokenKind::Eof) {
            let item_start = self.pos;
            match self.parse_campaign_item() {
                Ok(item) => items.push(item),
                Err(e) => {
                    self.report(e);
                    self.recover(item_start);
                }
            }
        }
        self.expect_dedent()?;
        Ok(CampaignDecl { name, items, span: self.span_from(start) })
    }

    /// `campaign_item`
    fn parse_campaign_item(&mut self) -> PResult<CampaignItem> {
        let item = if self.eat_kw("program") {
            CampaignItem::Program(self.string()?)
        } else if self.eat_kw("profile") {
            CampaignItem::Profile(self.upper()?)
        } else if self.eat_kw("sweep") {
            let param = self.upper()?;
            self.expect_op("=")?;
            if self.eat_op("[") {
                let mut values = vec![self.parse_const_expr()?];
                while self.eat_op(",") {
                    values.push(self.parse_const_expr()?);
                }
                self.expect_op("]")?;
                CampaignItem::SweepList { param, values }
            } else {
                let from = self.parse_const_expr()?;
                self.expect_op("..")?;
                let to = self.parse_const_expr()?;
                self.expect_kw("step")?;
                let step = self.parse_const_expr()?;
                CampaignItem::SweepRange { param, from, to, step }
            }
        } else if self.eat_kw("repeat") {
            CampaignItem::Repeat(self.parse_int_lit()?)
        } else if self.eat_kw("stop_on") {
            let v = if self.eat_word("fail") {
                StopOn::Fail
            } else if self.eat_kw("never") {
                StopOn::Never
            } else {
                return Err(self.error_here("`fail` oder `never`"));
            };
            CampaignItem::StopOn(v)
        } else {
            return Err(self.error_here("`program`, `profile`, `sweep`, `repeat` oder `stop_on`"));
        };
        self.expect_newline()?;
        Ok(item)
    }

    /// `trigger_decl`
    fn parse_trigger_decl(&mut self) -> PResult<TriggerDecl> {
        let start = self.pos;
        self.expect_kw("trigger")?;
        let name = self.ident()?;
        let node = if self.eat_kw("node") { Some(self.ident()?) } else { None };
        self.expect_op(":")?;
        self.expect_newline()?;
        self.expect_indent()?;
        self.expect_kw("when")?;
        let when = self.parse_guard()?;
        self.expect_newline()?;
        self.expect_kw("then")?;
        let then = self.parse_at_stmt()?;
        self.expect_kw("bound")?;
        let bound = self.parse_duration_lit()?;
        self.expect_newline()?;
        self.expect_dedent()?;
        Ok(TriggerDecl { name, node, when, then, bound, span: self.span_from(start) })
    }
}
