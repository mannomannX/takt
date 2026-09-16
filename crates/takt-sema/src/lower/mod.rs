//! Elaborator (plan/m1.md 3.1): loest Namen auf, bestimmt Typen bidirektional
//! und erzeugt die MIR in einem Durchlauf. Ein Fehler je Anweisung; die MIR
//! entsteht nur ohne Fehler.

pub mod decl;
pub mod expr;
pub mod format;
pub mod generics;
pub mod layout;
pub mod machine;
pub mod pattern;
pub mod stmt;
pub mod stream;
pub mod types;

use std::collections::HashMap;

use takt_diag::{Diagnostic, Span, Stage};
use takt_interp::{Trap, Value, eval_const};
use takt_mir::expr::{Expr, ExprKind};
use takt_mir::machine::{Machine, VarDef};
use takt_mir::program::{Config, Program};
use takt_mir::types::{FloatWidth, IntWidth, Type};
use takt_mir::*;
use takt_syntax::Edition;
use takt_syntax::ast;

use crate::Options;
use crate::symbols::{Entity, Scopes, Symbol};
use crate::units::{Unit, Units};

/// Generische Variable einer Vorlage (3.12): Einheit (v1) oder Konstante
/// (v1.1); Typvariablen sind v1.2.
#[derive(Clone, Debug, PartialEq)]
pub enum GenericVar {
    /// Einheitenvariable `U`.
    Unit(String),
    /// `const N in lo..hi`; ohne Range die ganze Breite von `int`.
    Const {
        /// Name.
        name: String,
        /// Untergrenze.
        lo: i64,
        /// Obergrenze.
        hi: i64,
    },
}

impl GenericVar {
    /// Name der Variablen.
    pub fn name(&self) -> &str {
        match self {
            GenericVar::Unit(n) | GenericVar::Const { name: n, .. } => n,
        }
    }
}

/// Bindung einer generischen Variablen bei der Instanziierung.
#[derive(Clone, Debug, PartialEq)]
pub enum Binding {
    /// Einheit.
    Unit(Unit),
    /// Konstante.
    Const(i64),
}

/// Generische Vorlage einer Funktion (3.12).
#[derive(Clone, Debug)]
pub struct FnTemplate {
    /// Deklaration.
    pub decl: ast::FnDecl,
    /// Generische Variablen.
    pub generics: Vec<GenericVar>,
    /// Aus dem Prelude.
    pub prelude: bool,
}

/// Generische Vorlage eines Blocks.
#[derive(Clone, Debug)]
pub struct BlockTemplate {
    /// Deklaration.
    pub decl: ast::BlockDecl,
    /// Generische Variablen.
    pub generics: Vec<GenericVar>,
    /// Aus dem Prelude.
    pub prelude: bool,
}

/// Maschinenvorlage (5.8).
#[derive(Clone, Debug)]
pub struct MachineTemplate {
    /// Deklaration.
    pub decl: ast::MachineDecl,
    /// Eintrag `MachineKind::Template` im Programm.
    pub id: MachineId,
    /// Zustandstyp der Vorlage und ihrer Instanzen.
    pub state_enum: EnumId,
    /// Aus dem Prelude.
    pub prelude: bool,
}

/// Alle Vorlagen.
#[derive(Debug, Default)]
pub struct Templates {
    /// Funktionen.
    pub fns: Vec<FnTemplate>,
    /// Bloecke.
    pub blocks: Vec<BlockTemplate>,
    /// Maschinen.
    pub machines: Vec<MachineTemplate>,
}

/// Umgebung der generischen Variablen einer Instanziierung.
#[derive(Clone, Debug, Default)]
pub struct Env {
    /// Die Variablen (Index = `Atom::Var` fuer Einheiten).
    pub vars: Vec<GenericVar>,
    /// Einheitenbindungen; `None` waehrend der generischen Pruefung und
    /// bei Konstantenvariablen.
    pub units: Vec<Option<Unit>>,
    /// Konstantenbindungen; `None` waehrend der generischen Pruefung und
    /// bei Einheitenvariablen.
    pub consts: Vec<Option<i64>>,
}

impl Env {
    /// Offene Umgebung (generische Pruefung).
    pub fn open(vars: &[GenericVar]) -> Env {
        Env { vars: vars.to_vec(), units: vec![None; vars.len()], consts: vec![None; vars.len()] }
    }

    /// Gebundene Umgebung einer Instanz.
    pub fn bound(vars: &[GenericVar], bindings: &[Binding]) -> Env {
        let units = bindings.iter().map(|b| if let Binding::Unit(u) = b { Some(u.clone()) } else { None }).collect();
        let consts = bindings.iter().map(|b| if let Binding::Const(n) = b { Some(*n) } else { None }).collect();
        Env { vars: vars.to_vec(), units, consts }
    }

    /// Index einer Variablen.
    pub fn index(&self, name: &str) -> Option<u32> {
        self.vars.iter().position(|v| v.name() == name).map(|i| i as u32)
    }

    /// Name einer Variablen.
    pub fn name(&self, i: u32) -> Option<&str> {
        self.vars.get(i as usize).map(GenericVar::name)
    }

    /// Wert einer Konstantenvariablen: die Bindung, waehrend der generischen
    /// Pruefung die Untergrenze ihrer Range (3.12).
    pub fn const_value(&self, name: &str) -> Option<i64> {
        let i = self.index(name)? as usize;
        match &self.vars[i] {
            GenericVar::Const { lo, .. } => Some(self.consts[i].unwrap_or(if *lo == i64::MIN { 1 } else { *lo })),
            GenericVar::Unit(_) => None,
        }
    }
}

/// Memoisierte Instanz einer Vorlage.
#[derive(Clone, Copy, Debug)]
pub enum Memo {
    /// Funktion.
    Fn(FnId),
    /// Block.
    Block(BlockId),
}

/// Kontext eines Funktions- oder Methodenrumpfs.
#[derive(Debug)]
pub struct FnCtx {
    /// Lokale (Parameter zuerst) mit `VarId = base + Index`.
    pub locals: Vec<VarDef>,
    /// Erste `VarId` der Lokalen (Blockmethoden: nach Parametern und Zustandsvariablen).
    pub base: u32,
    /// Rueckgabetyp.
    pub ret: Option<TypeId>,
    /// Methodenrumpf eines Blocks.
    pub block: Option<BlockId>,
}

/// Art des umgebenden Blocks (Aktionsblock-Regeln 5.5, Ziel von `->`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockKind {
    /// `loop:` eines Zustands oder der Maschine.
    Loop,
    /// `enter:`, `exit:`.
    EnterExit,
    /// Aktionen eines Uebergangs.
    Transition,
    /// `at`-Block.
    At,
    /// Funktions- oder Methodenrumpf.
    Fn,
    /// Sequenz.
    Sequence,
    /// `until … else:`.
    Else,
    /// Handler (M2).
    Handler,
}

impl BlockKind {
    /// Aktionsblock nach 5.5?
    pub fn is_action(self) -> bool {
        matches!(self, BlockKind::EnterExit | BlockKind::Transition | BlockKind::At | BlockKind::Else)
    }
}

/// Kontext einer Maschine waehrend des Lowerings.
#[derive(Debug)]
pub struct MachineCtx {
    /// Die entstehende Maschine.
    pub machine: Machine,
    /// Ihr reservierter Eintrag.
    pub id: MachineId,
    /// Zustandstyp.
    pub state_enum: EnumId,
    /// Zustandsnamen → Id (alle Zustaende der Maschine).
    pub states: HashMap<String, StateId>,
    /// Aktueller Zustand (fuer zustandslokale Variablen, Zaehler).
    pub current: Option<StateId>,
    /// Zustand, in den eine Sequenz ihre Variablen hebt.
    pub lift_to: Option<StateId>,
    /// Vorlage, deren Rumpf gerade generisch geprueft oder instanziiert wird.
    pub template: Option<usize>,
}

/// Haeufig gebrauchte Typen.
#[derive(Clone, Copy, Debug)]
pub struct Builtins {
    /// `bool`
    pub bool: TypeId,
    /// `int`
    pub int: TypeId,
    /// `float` in Programmbreite (4.2).
    pub float: TypeId,
    /// `Duration`
    pub duration: TypeId,
    /// `FaultKind` aus dem Prelude (5.3).
    pub fault_kind: EnumId,
    /// Record `LastFault` (5.3).
    pub last_fault: RecordId,
    /// Typ von `last_fault`.
    pub last_fault_ty: TypeId,
}

/// Der Elaborator.
pub struct Lowerer<'a> {
    /// Das entstehende Programm.
    pub program: Program,
    /// Einheiten.
    pub units: Units,
    /// Sichtbereiche.
    pub scopes: Scopes,
    /// Namen, die im gerade gesenkten `else`-Zweig keinen Wert tragen
    /// (Pruefung 6, 6.2): die Bindung eines `until`-Guards, dessen Muster
    /// nicht getroffen hat.
    pub unbound: Vec<String>,
    /// Diagnosen.
    pub diags: Vec<Diagnostic>,
    /// Optionen.
    pub options: &'a Options,
    /// Edition.
    pub edition: Edition,
    /// Vorlagen.
    pub templates: Templates,
    /// Instanzen der Vorlagen.
    pub memo: HashMap<String, Memo>,
    /// Generische Umgebung.
    pub env: Env,
    /// Rahmen der Funktionsruempfe (innerster zuletzt).
    pub fn_ctx: Vec<FnCtx>,
    /// Maschine.
    pub mctx: Option<MachineCtx>,
    /// Dominanzfakten je Block (Schluessel einer Stelle).
    pub facts: Vec<Vec<String>>,
    /// Tiefe der generischen Pruefung (> 0: Programm ist eine Kopie).
    pub checking: u32,
    /// Prelude wird gelowert.
    pub prelude: bool,
    /// Untergrenze einer Range: ein einheitenloses Literal erbt die Einheit
    /// der Obergrenze (3.4, die einzige Ausnahme von 3.6).
    pub in_range_bound: bool,
    /// Eingebaute Typen.
    pub tys: Builtins,
    /// Zustandstypen je Maschine.
    pub state_enums: HashMap<MachineId, EnumId>,
    /// Zustandsrecord je Block (Typ der Instanzen).
    pub block_records: HashMap<BlockId, RecordId>,
    /// Ausgehobene `step`-Aufrufe anonymer Instanzen (5.7); `stmts` stellt
    /// sie vor die Anweisung, in der sie stehen.
    pub pending: Vec<takt_mir::stmt::Stmt>,
    /// Gerade wird eine Anweisung gesenkt — nur dort darf ein Aufruf
    /// ausgehoben werden, in Guards nicht.
    pub in_stmt: bool,
    /// Tiefe verschachtelter `for`-Schleifen (5.7: kein `step` darin).
    pub for_depth: u32,
    /// Laufende Nummer anonymer Instanzen.
    pub anon: u32,
    /// Zaehler fuer eindeutige Namen (Instanzen, Segmente).
    pub counter: u32,
}

/// Code der Namensaufloesung.
pub const SC2: &str = "SC-2";
/// Code der Typpruefung.
pub const SC3: &str = "SC-3";
/// Code der Maschinenregeln.
pub const SC8: &str = "SC-8";
/// Code der Einheiten auf Ganzzahlen (3.2).
pub const SC38: &str = "SC-38";

impl<'a> Lowerer<'a> {
    /// Neuer Elaborator; die Konfiguration kommt aus dem `system:`-Block.
    pub fn new(config: Config, edition: Edition, options: &'a Options) -> Self {
        let mut program = Program::new(config);
        let units = Units::new(&mut program);
        let bool = program.types.intern(Type::Bool);
        let int = program.types.intern(Type::Int { width: IntWidth::I64, unit: None, range: None });
        let float = program.types.intern(Type::Float { width: program.config.float_width, unit: None, range: None });
        let duration = program.types.intern(Type::Duration { range: None });
        let mut scopes = Scopes::default();
        scopes.push();
        Lowerer {
            program,
            units,
            scopes,
            unbound: Vec::new(),
            diags: Vec::new(),
            options,
            edition,
            templates: Templates::default(),
            memo: HashMap::new(),
            env: Env::default(),
            fn_ctx: Vec::new(),
            mctx: None,
            facts: Vec::new(),
            checking: 0,
            prelude: false,
            in_range_bound: false,
            tys: Builtins {
                bool,
                int,
                float,
                duration,
                fault_kind: EnumId(0),
                last_fault: RecordId(0),
                last_fault_ty: bool,
            },
            state_enums: HashMap::new(),
            block_records: HashMap::new(),
            counter: 0,
            pending: Vec::new(),
            in_stmt: false,
            for_depth: 0,
            anon: 0,
        }
    }

    // ------------------------------------------------------------ Diagnosen

    /// Fehler.
    pub fn error(&mut self, code: &'static str, span: Span, msg: impl Into<String>) -> Diagnostic {
        let d = Diagnostic::error(code, span, msg);
        self.diags.push(d.clone());
        d
    }

    /// Fehler mit Vorschlag.
    pub fn error_hint(&mut self, code: &'static str, span: Span, msg: impl Into<String>, hint: impl Into<String>) {
        let d = Diagnostic::error(code, span, msg).with_suggestion(hint);
        self.diags.push(d);
    }

    /// Warnung.
    pub fn warn(&mut self, code: &'static str, span: Span, msg: impl Into<String>) {
        self.diags.push(Diagnostic::warning(code, span, msg));
    }

    /// Warnung mit Vorschlag.
    pub fn warn_hint(&mut self, code: &'static str, span: Span, msg: impl Into<String>, hint: impl Into<String>) {
        self.diags.push(Diagnostic::warning(code, span, msg).with_suggestion(hint));
    }

    /// Konstrukt einer spaeteren Stufe.
    pub fn stage(&mut self, span: Span, what: &str, stage: Stage) {
        self.diags.push(
            Diagnostic::error(SC3, span, format!("{what} wird erst ab {} unterstuetzt", stage.as_str()))
                .with_stage(stage),
        );
    }

    /// Gibt es Fehler?
    pub fn has_errors(&self) -> bool {
        self.diags.iter().any(Diagnostic::is_error)
    }

    // ------------------------------------------------------------ Symbole

    /// Deklariert einen Namen im innersten Bereich (Festlegung 5: keine
    /// Verdeckung ausser von Prelude-Namen).
    pub fn declare(&mut self, name: &ast::Ident, entity: Entity) -> bool {
        let symbol = Symbol { entity, span: name.span, prelude: self.prelude };
        match self.scopes.declare(&name.name, symbol) {
            Ok(None) => true,
            Ok(Some(shadowed)) => {
                if !self.prelude {
                    // Der verdeckte Name steht im Prelude, einer anderen
                    // Datei; sein Span zeigt nicht in die Quelle des Nutzers.
                    let mut d = Diagnostic::warning(
                        SC2,
                        name.span,
                        format!("`{}` verdeckt einen Namen der Standardbibliothek", name.name),
                    );
                    if !shadowed.prelude {
                        d = d.with_note(shadowed.span, "hier definiert");
                    }
                    self.diags.push(d);
                }
                true
            }
            Err(shadowed) => {
                let mut d = Diagnostic::error(SC2, name.span, format!("`{}` ist schon definiert", name.name))
                    .with_suggestion("anderen Namen waehlen; innere Bereiche verdecken keine sichtbaren Namen");
                if !shadowed.prelude {
                    d = d.with_note(shadowed.span, "hier definiert");
                }
                self.diags.push(d);
                false
            }
        }
    }

    /// Sucht einen Namen; unbekannte Namen bekommen einen Vorschlag.
    pub fn lookup(&mut self, name: &ast::Ident) -> Option<Entity> {
        // Pruefung 6 (6.2): Im `else`-Zweig eines `until` traegt die Bindung
        // des Guards keinen Wert — der Zweig laeuft ja, *weil* das Muster
        // nicht getroffen hat.
        if self.unbound.iter().any(|n| n == &name.name) {
            self.error_hint(
                crate::checks::SC6,
                name.span,
                format!("`{}` ist hier nicht gebunden", name.name),
                "der `else`-Zweig laeuft, weil das Muster nicht getroffen hat (6.2)",
            );
            return None;
        }
        match self.scopes.lookup(&name.name) {
            Some(s) => Some(s.entity.clone()),
            None => {
                let hint = crate::symbols::suggestion(&name.name, self.scopes.visible()).map(str::to_string);
                let mut d = Diagnostic::error(SC2, name.span, format!("`{}` ist nicht definiert", name.name));
                if let Some(h) = hint {
                    d = d.with_suggestion(format!("meinst du `{h}`?"));
                }
                self.diags.push(d);
                None
            }
        }
    }

    /// Sucht ohne Meldung.
    pub fn peek(&self, name: &str) -> Option<&Entity> {
        self.scopes.lookup(name).map(|s| &s.entity)
    }

    // ------------------------------------------------------------ Variablen

    /// Legt eine Variable im aktuellen Kontext an: Funktionsrahmen oder Maschine.
    pub fn new_var(&mut self, def: VarDef) -> VarId {
        if let Some(ctx) = self.fn_ctx.last_mut() {
            let id = VarId(ctx.base + ctx.locals.len() as u32);
            ctx.locals.push(def);
            id
        } else if let Some(m) = &mut self.mctx {
            m.machine.add_var(def)
        } else {
            VarId(u32::MAX)
        }
    }

    /// Typ einer Variablen.
    pub fn var_type(&self, v: VarId) -> Option<TypeId> {
        if let Some(ctx) = self.fn_ctx.last() {
            if v.0 >= ctx.base {
                return ctx.locals.get((v.0 - ctx.base) as usize).map(|d| d.ty);
            }
            if let Some(b) = ctx.block {
                let def = &self.program.blocks[b.index()];
                let i = v.index();
                if i < def.params.len() {
                    return Some(def.params[i].ty);
                }
                return def.state_vars.get(i - def.params.len()).map(|d| d.ty);
            }
            return None;
        }
        self.mctx.as_ref().and_then(|m| m.machine.vars.get(v.index()).map(|d| d.ty))
    }

    // ------------------------------------------------------------ Konstanten

    /// Faltet einen konstanten Ausdruck zu einem Literal (11.3).
    pub fn fold(&mut self, e: Expr) -> Option<Expr> {
        if is_literal(&e) {
            return Some(e);
        }
        let span = e.span;
        match eval_const(&self.program, &e) {
            Ok(v) => match value_to_expr(&v, e.ty, span, &self.program) {
                Some(lit) => Some(lit),
                None => Some(e),
            },
            Err(Trap::Fault(f)) => {
                self.error(SC3, span, format!("konstanter Ausdruck faultet: {}", f.message));
                None
            }
            Err(Trap::Bug(msg)) => {
                self.error_hint(SC3, span, "Ausdruck ist nicht konstant", msg);
                None
            }
        }
    }

    /// Konstante Ganzzahl (Array-Laengen, Kapazitaeten, `range(N)`).
    pub fn const_int(&mut self, e: &ast::Expr) -> Option<i64> {
        // 3.12: eine Konstantenvariable steht in Typen und `range(N)`.
        if let ast::ExprKind::Upper { name, args: None } = &e.kind {
            if let Some(n) = self.env.const_value(&name.name) {
                return Some(n);
            }
        }
        let ty = self.tys.int;
        let lowered = self.expr(e, Some(ty))?;
        // 8.4: `param` ist zur Compile-Zeit unbekannt, ein Tunable ein Input
        // (Pruefung 35) — `fold` wuerde sonst den Default einsetzen.
        if let Some(p) = first_param(&lowered) {
            let param = &self.program.params[p.index()];
            let (code, what) = if param.tunable {
                (crate::checks::SC35, format!("`tunable param {}` ist keine Compile-Zeit-Konstante (8.4)", param.name))
            } else {
                (SC3, format!("`param {}` ist zur Compile-Zeit unbekannt (8.4)", param.name))
            };
            self.error_hint(
                code,
                e.span(),
                what,
                "Array-Groessen, Kapazitaeten und `repeat` brauchen `const` oder ein Literal",
            );
            return None;
        }
        let folded = self.fold(lowered)?;
        match folded.kind {
            ExprKind::Int(i) => Some(i),
            _ => {
                self.error(SC3, e.span(), "konstante Ganzzahl erwartet");
                None
            }
        }
    }

    // ------------------------------------------------------------ Dominanz

    /// Schluessel einer Stelle fuer Dominanzfakten (3.5, 3.8).
    pub fn dom_key(e: &Expr) -> Option<String> {
        match &e.kind {
            ExprKind::Input { channel, .. } => Some(format!("in{}", channel.0)),
            ExprKind::Var(v) => Some(format!("var{}", v.0)),
            ExprKind::Checked { expr, .. } => Self::dom_key(expr),
            ExprKind::Index { base, index } => {
                let b = Self::dom_key(base)?;
                match &index.kind {
                    ExprKind::Int(i) => Some(format!("{b}[{i}]")),
                    ExprKind::Var(v) => Some(format!("{b}[var{}]", v.0)),
                    _ => None,
                }
            }
            ExprKind::Field { base, field } => Some(format!("{}.{field}", Self::dom_key(base)?)),
            _ => None,
        }
    }

    /// Gilt die Stelle als dominiert?
    pub fn dominated(&self, key: &str) -> bool {
        self.facts.iter().any(|f| f.iter().any(|k| k == key))
    }

    /// Fakten aus einer Bedingung (`x.valid`, `r.ok`, `a and b`).
    pub fn facts_of(e: &Expr) -> Vec<String> {
        use takt_mir::expr::{Accessor, BinaryOp};
        match &e.kind {
            ExprKind::Accessor { base, accessor: Accessor::Valid | Accessor::Ok, .. } => {
                Self::dom_key(base).into_iter().collect()
            }
            ExprKind::Binary { op: BinaryOp::And, lhs, rhs } => {
                let mut v = Self::facts_of(lhs);
                v.extend(Self::facts_of(rhs));
                v
            }
            _ => Vec::new(),
        }
    }

    // ------------------------------------------------------------ Kontexte

    /// Fuehrt `f` in einem neuen Sichtbereich und Faktenrahmen aus.
    pub fn scoped<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        self.scopes.push();
        self.facts.push(Vec::new());
        let r = f(self);
        self.facts.pop();
        self.scopes.pop();
        r
    }

    /// Eindeutiger Zaehler.
    pub fn next_counter(&mut self) -> u32 {
        self.counter += 1;
        self.counter
    }

    /// Breite von `float` im Programm.
    pub fn float_width(&self) -> FloatWidth {
        self.program.config.float_width
    }

    /// Typ zu einem Index.
    pub fn ty(&self, id: TypeId) -> &Type {
        self.program.types.get(id)
    }

    /// Interniert einen Typ.
    pub fn intern(&mut self, t: Type) -> TypeId {
        self.program.types.intern(t)
    }
}

/// Uebersetzt Prelude und Datei in die MIR (plan/m1.md 3.2).
pub fn run(file: &ast::File, edition: Edition, options: &Options) -> (Option<Program>, Vec<Diagnostic>) {
    let prelude_ast = prelude_file(edition);
    let mut diags = Vec::new();
    let config = decl::config_from(file, edition.number(), &mut diags);
    let mut lo = Lowerer::new(config, edition, options);
    lo.diags = diags;
    lo.declare_builtins();
    // Fehler aus dem `system:`-Block der Nutzerdatei stehen schon in `diags`
    // und zaehlen nicht zum Prelude.
    let before = lo.diags.len();
    lo.prelude = true;
    lo.collect(&prelude_ast);
    lo.lower_bodies(&prelude_ast);
    let broken: Vec<&Diagnostic> = lo.diags[before..].iter().filter(|d| d.is_error()).collect();
    if !broken.is_empty() {
        // Ein fehlerhaftes Prelude ist ein Fehler des Compilers, kein Fehler
        // des Nutzers; er wird gemeldet, statt den Prozess zu beenden.
        let first = broken[0].message.clone();
        let mut diags = std::mem::take(&mut lo.diags);
        diags.push(Diagnostic::error(
            SC3,
            Span::default(),
            format!("interner Fehler: das Prelude uebersetzt nicht (`{first}`)"),
        ));
        return (None, diags);
    }
    lo.prelude = false;
    // Die Datei bekommt ihren Bereich ueber dem Prelude; Vorlagen der
    // Bibliothek sehen bei spaeter Instanziierung nur das Prelude.
    lo.scopes.push();
    lo.collect(file);
    lo.lower_bodies(file);
    if !lo.has_errors() {
        lo.run_mir_checks();
    }
    let mut program = lo.program;
    let mut diags = lo.diags;
    if diags.iter().any(Diagnostic::is_error) {
        return (None, diags);
    }
    if let Err(d) = takt_mir::desugar(&mut program) {
        diags.push(d);
        return (None, diags);
    }
    (Some(program), diags)
}

/// Parst das Prelude; ein Fehler dort ist ein Fehler des Compilers.
fn prelude_file(edition: Edition) -> ast::File {
    let toks = takt_syntax::tokenize_in(crate::PRELUDE, edition);
    assert!(toks.errors.is_empty(), "Prelude: Tokenizer {:?}", toks.errors);
    let (file, errors) = takt_syntax::parse_file(&toks);
    assert!(errors.is_empty(), "Prelude: Parser {errors:?}");
    file
}

/// Ist der Ausdruck ein Literal (keine Faltung noetig)?
pub fn is_literal(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Bool(_)
        | ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Duration(_)
        | ExprKind::Str(_)
        | ExprKind::None => true,
        ExprKind::Variant { fields, .. } | ExprKind::Record { fields, .. } | ExprKind::Array(fields) => {
            fields.iter().all(is_literal)
        }
        ExprKind::Tuple(a, b) => is_literal(a) && is_literal(b),
        ExprKind::Lift(x) | ExprKind::Ok(x) | ExprKind::Err(x) => is_literal(x),
        _ => false,
    }
}

/// Wert als Literal-Ausdruck.
pub fn value_to_expr(v: &Value, ty: TypeId, span: Span, p: &Program) -> Option<Expr> {
    let kind = match v {
        Value::Bool(b) => ExprKind::Bool(*b),
        Value::Int(i) => ExprKind::Int(*i),
        Value::UInt(u) => ExprKind::Int(i64::try_from(*u).ok()?),
        Value::F32(f) => ExprKind::Float(f64::from(*f)),
        Value::F64(f) => ExprKind::Float(*f),
        Value::Duration(d) => ExprKind::Duration(*d),
        Value::Str(s) => ExprKind::Str(s.clone()),
        Value::Optional(None) => ExprKind::None,
        Value::Optional(Some(inner)) => {
            let inner_ty = match p.types.get(ty) {
                Type::Optional(t) => *t,
                _ => return None,
            };
            ExprKind::Lift(Box::new(value_to_expr(inner, inner_ty, span, p)?))
        }
        Value::Enum { variant, fields } => {
            let Type::Enum(id) = p.types.get(ty) else { return None };
            let def = &p.enums[id.index()].variants[*variant as usize];
            let fields = fields
                .iter()
                .zip(&def.fields)
                .map(|(f, d)| value_to_expr(f, d.ty, span, p))
                .collect::<Option<Vec<_>>>()?;
            ExprKind::Variant { enum_id: *id, variant: *variant, fields }
        }
        Value::Record(fields) => {
            let Type::Record(id) = p.types.get(ty) else { return None };
            let def = &p.records[id.index()];
            let fields = fields
                .iter()
                .zip(&def.fields)
                .map(|(f, d)| value_to_expr(f, d.ty, span, p))
                .collect::<Option<Vec<_>>>()?;
            ExprKind::Record { record: *id, fields }
        }
        Value::Array(items) => {
            let Type::Array { elem, .. } = p.types.get(ty) else { return None };
            ExprKind::Array(items.iter().map(|x| value_to_expr(x, *elem, span, p)).collect::<Option<Vec<_>>>()?)
        }
        Value::Table(points) => {
            let Type::Table { key, value } = p.types.get(ty) else { return None };
            let (key, value) = (*key, *value);
            ExprKind::Array(
                points
                    .iter()
                    .map(|(a, b)| {
                        Some(Expr::new(
                            ExprKind::Tuple(
                                Box::new(value_to_expr(a, key, span, p)?),
                                Box::new(value_to_expr(b, value, span, p)?),
                            ),
                            ty,
                            span,
                        ))
                    })
                    .collect::<Option<Vec<_>>>()?,
            )
        }
        _ => return None,
    };
    Some(Expr::new(kind, ty, span))
}

/// Spanne eines AST-Ausdrucks.
pub trait Spanned {
    /// Position.
    fn span(&self) -> Span;
}

impl Spanned for ast::Expr {
    fn span(&self) -> Span {
        self.span
    }
}

/// Der erste Parameter in einem Ausdruck, wenn es einen gibt.
fn first_param(e: &Expr) -> Option<ParamId> {
    if let ExprKind::Param(p) = &e.kind {
        return Some(*p);
    }
    e.children().into_iter().find_map(first_param)
}
