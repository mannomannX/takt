//! Syntaxbaum: ein Knoten je Produktion aus `grammar/takt.ebnf`, mit Positionen.
//!
//! Der Baum ist ein AST mit Spannen, kein verlustfreier Konkretbaum: Kommentare
//! und Leerzeilen liegen als Beiwerk im Tokenstrom (lexer.md) und werden vom
//! Formatter ueber die Positionen wieder zugeordnet. Sequenzen bleiben Sequenzen;
//! das Desugaring nach 6.2 ist Sache der MIR.
//!
//! Knoten werden einmal gebaut und danach nur gelesen; grosse Varianten neben
//! kleinen (`Stmt`, `SeqItem`) sind gewollt, Boxen wuerde nur Zugriffe kosten.
#![allow(clippy::large_enum_variant)]

/// Byte-Bereich im Quelltext (derselbe Typ wie in den Diagnosen).
pub use takt_diag::Span;

/// Ein Name (IDENT, UPPER_IDENT, TYPE_IDENT oder Membername) mit Position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ident {
    /// Text des Namens.
    pub name: String,
    /// Position.
    pub span: Span,
}

/// Stringliteral mit aufgeloesten Escapes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StrLit {
    /// Inhalt ohne Anfuehrungszeichen.
    pub value: String,
    /// Position einschliesslich Anfuehrungszeichen.
    pub span: Span,
}

/// Ganzzahlliteral in einer der vier Schreibweisen; der Text bleibt erhalten.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntLit {
    /// Quelltext, z. B. `0xFFFF_FFFF`.
    pub text: String,
    /// Position.
    pub span: Span,
}

/// Fliesskommaliteral; der Dezimaltext bleibt erhalten (4.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FloatLit {
    /// Quelltext.
    pub text: String,
    /// Position.
    pub span: Span,
}

/// `number := int_lit | FLOAT`
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Number {
    /// Ganze Zahl.
    Int(IntLit),
    /// Fliesskommazahl.
    Float(FloatLit),
}

/// Dauer, exakt in Nanosekunden (3.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurationLit {
    /// Wert in Nanosekunden.
    pub ns: i64,
    /// Position.
    pub span: Span,
}

// ---------------------------------------------------------------- Datei

/// Eine Takt-Datei.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct File {
    /// Deklarationen in Quelltextreihenfolge.
    pub items: Vec<Item>,
}

/// Deklaration auf Dateiebene (`file`).
#[derive(Clone, Debug, PartialEq)]
#[allow(missing_docs)]
pub enum Item {
    Import(Import),
    System(SystemDecl),
    Type(TypeDecl),
    Unitvec(UnitvecDecl),
    Enum(EnumDecl),
    Record(RecordDecl),
    Unit(UnitDecl),
    Stream(StreamDecl),
    Port(PortDecl),
    Node(NodeDecl),
    Property(PropertyDecl),
    Const(ConstDecl),
    Param(ParamDecl),
    Profile(ProfileDecl),
    Channel(ChannelDecl),
    Command(CommandDecl),
    Fn(FnDecl),
    Native(NativeDecl),
    Block(BlockDecl),
    Machine(MachineDecl),
    Instance(InstanceDecl),
    Scenario(ScenarioDecl),
    Campaign(CampaignDecl),
    Trigger(TriggerDecl),
}

/// `import`
#[derive(Clone, Debug, PartialEq)]
pub enum Import {
    /// `import a.b as c`
    Module {
        /// Pfad.
        path: Vec<Ident>,
        /// Alias.
        alias: Option<Ident>,
        /// Position.
        span: Span,
    },
    /// `import channels from "site.hw"`
    Channels {
        /// Datei der Hardware-Konfiguration.
        file: StrLit,
        /// Position.
        span: Span,
    },
}

/// `system:`
#[derive(Clone, Debug, PartialEq)]
pub struct SystemDecl {
    /// Eintraege.
    pub items: Vec<SystemItem>,
    /// Position.
    pub span: Span,
}

/// `system_item`
#[derive(Clone, Debug, PartialEq)]
#[allow(missing_docs)]
pub enum SystemItem {
    Tick(DurationLit),
    OutputTiming(OutputTiming),
    FaultIsFail(bool),
    TickSource(StrLit),
    TickTolerance { value: Expr, ticks: Option<IntLit> },
    Target(Ident),
    Float(FloatWidth),
    Language(IntLit),
}

/// `asap | boundary`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum OutputTiming {
    Asap,
    Boundary,
}

/// `f32 | f64` in `system: float = …`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum FloatWidth {
    F32,
    F64,
}

// ---------------------------------------------------------------- Typen und Einheiten

/// `type_decl`
#[derive(Clone, Debug, PartialEq)]
pub struct TypeDecl {
    /// Aliasname.
    pub name: Ident,
    /// Typ.
    pub ty: Type,
    /// Position.
    pub span: Span,
}

/// `unitvec_decl`
#[derive(Clone, Debug, PartialEq)]
pub struct UnitvecDecl {
    /// Name.
    pub name: Ident,
    /// Einheiten je Komponente.
    pub units: Vec<UnitExpr>,
    /// Position.
    pub span: Span,
}

/// `enum_decl`
#[derive(Clone, Debug, PartialEq)]
pub struct EnumDecl {
    /// Name.
    pub name: Ident,
    /// `layout u8` usw.
    pub layout: Option<IntType>,
    /// `open`
    pub open: bool,
    /// Varianten.
    pub variants: Vec<Variant>,
    /// Position.
    pub span: Span,
}

/// `variant`
#[derive(Clone, Debug, PartialEq)]
pub struct Variant {
    /// Name.
    pub name: Ident,
    /// Explizite Diskriminante.
    pub discriminant: Option<IntLit>,
    /// Felder.
    pub fields: Vec<Field>,
    /// Position.
    pub span: Span,
}

/// `record_decl`
#[derive(Clone, Debug, PartialEq)]
pub struct RecordDecl {
    /// Name.
    pub name: Ident,
    /// Drahtformat.
    pub layout: Option<Layout>,
    /// Felder.
    pub fields: Vec<RecordField>,
    /// Position.
    pub span: Span,
}

/// `layout little | big [, align = N]`
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    /// Byte-Reihenfolge.
    pub endian: Endian,
    /// Ausrichtung.
    pub align: Option<IntLit>,
}

/// `little | big`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum Endian {
    Little,
    Big,
}

/// `record_field`
#[derive(Clone, Debug, PartialEq)]
pub enum RecordField {
    /// Gewoehnliches Feld.
    Plain(Field),
    /// Traegerfeld mit Bitfeldern.
    Bits {
        /// Name.
        name: Ident,
        /// Traegertyp.
        ty: IntType,
        /// Bitfelder.
        bits: Vec<Bitfield>,
        /// Position.
        span: Span,
    },
}

/// `field`; ein Padding-Feld heisst `_`.
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    /// Name oder `_`.
    pub name: Ident,
    /// Typ.
    pub ty: Type,
    /// Konstantenfeld.
    pub value: Option<Expr>,
    /// `offset = N`
    pub offset: Option<IntLit>,
    /// `with len = feld`
    pub len_field: Option<Ident>,
    /// Position.
    pub span: Span,
}

/// `bitfield`
#[derive(Clone, Debug, PartialEq)]
pub struct Bitfield {
    /// Name.
    pub name: Ident,
    /// `bool` oder Integer-Typ.
    pub ty: BitType,
    /// Erste Bitposition.
    pub from: IntLit,
    /// Letzte Bitposition bei `a..b`.
    pub to: Option<IntLit>,
    /// Position.
    pub span: Span,
}

/// Typ eines Bitfelds.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum BitType {
    Bool,
    Int(IntType),
}

/// `unit_decl`
#[derive(Clone, Debug, PartialEq)]
pub enum UnitDecl {
    /// `unit psi = 6894.76 Pa`; ohne Einheit dimensionslos.
    Scaled {
        /// Name.
        name: Ident,
        /// Faktor.
        factor: Number,
        /// Bezugseinheit.
        unit: Option<UnitExpr>,
        /// Position.
        span: Span,
    },
    /// `unit degC = affine(K, 273.15)`
    Affine {
        /// Name.
        name: Ident,
        /// Basiseinheit.
        base: UnitExpr,
        /// Verschiebung.
        offset: Number,
        /// Position.
        span: Span,
    },
}

/// `unit_expr := unit_term { (* | /) unit_term }`
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnitExpr {
    /// Erster Term.
    pub first: UnitTerm,
    /// Weitere Terme mit Operator.
    pub rest: Vec<(UnitOp, UnitTerm)>,
    /// Position.
    pub span: Span,
}

/// `*` oder `/` zwischen Einheitentermen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum UnitOp {
    Mul,
    Div,
}

/// `unit_term`
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnitTerm {
    /// Einheitenname oder Variable; `None` ist die dimensionslose `1`.
    pub name: Option<Ident>,
    /// Exponent nach `^`.
    pub exponent: Option<IntLit>,
    /// Position.
    pub span: Span,
}

/// `unit_tuple`
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnitTuple {
    /// Name eines `unitvec`.
    Named(Ident),
    /// `1/X`
    Inverse(Ident),
    /// `(m, m/s)`
    Literal(Vec<UnitExpr>),
}

// ---------------------------------------------------------------- Konstanten, Parameter, Channels

/// `const_decl`
#[derive(Clone, Debug, PartialEq)]
pub struct ConstDecl {
    /// Name.
    pub name: Ident,
    /// Typ.
    pub ty: Option<Type>,
    /// Wert.
    pub value: Expr,
    /// Position.
    pub span: Span,
}

/// `param_decl`
#[derive(Clone, Debug, PartialEq)]
pub struct ParamDecl {
    /// `tunable`
    pub tunable: bool,
    /// Name.
    pub name: Ident,
    /// Typ.
    pub ty: Type,
    /// Default.
    pub value: Expr,
    /// Metadaten.
    pub attrs: Vec<Attr>,
    /// Position.
    pub span: Span,
}

/// `profile_decl`
#[derive(Clone, Debug, PartialEq)]
pub struct ProfileDecl {
    /// Name.
    pub name: Ident,
    /// Belegungen.
    pub entries: Vec<(Ident, Expr)>,
    /// Position.
    pub span: Span,
}

/// `channel_decl`
#[derive(Clone, Debug, PartialEq)]
pub struct ChannelDecl {
    /// Richtung.
    pub dir: Direction,
    /// Name.
    pub name: Ident,
    /// Typ.
    pub ty: Type,
    /// Bindung.
    pub binding: Binding,
    /// Attribute.
    pub attrs: Vec<Attr>,
    /// Position.
    pub span: Span,
}

/// `input | output`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum Direction {
    Input,
    Output,
}

/// `binding`
#[derive(Clone, Debug, PartialEq)]
#[allow(missing_docs)]
pub enum Binding {
    Hw(StrLit),
    Sim(StrLit),
    None,
}

/// Ein Attribut nach `with`.
#[derive(Clone, Debug, PartialEq)]
pub struct Attr {
    /// Inhalt.
    pub kind: AttrKind,
    /// Position.
    pub span: Span,
}

/// `attr`
#[derive(Clone, Debug, PartialEq)]
#[allow(missing_docs)]
pub enum AttrKind {
    Safe(Expr),
    MaxAge(DurationLit),
    Rate(Expr),
    MaxRate(Expr),
    Capacity(IntLit),
    Framing(Framing),
    Overflow(Overflow),
    Wake(bool),
    Jitter(DurationLit),
    MaxSlew(Expr),
    Debounce(IntLit),
    CapacityBytes(IntLit),
    ExpectLen(IntLit),
    Irreversible,
    Label(StrLit),
    Display(UnitExpr),
    Group(StrLit),
    Doc(StrLit),
    /// `budget = {ram = …, wcet = …}` je Maschine (7.2).
    Budget(Vec<BudgetItem>),
}

/// Ein Posten in `budget = {…}` (7.2).
#[derive(Clone, Debug, PartialEq)]
pub struct BudgetItem {
    /// Welche Groesse.
    pub kind: BudgetKind,
    /// Der Wert.
    pub value: Expr,
    /// Position.
    pub span: Span,
}

/// Groesse eines Budgetpostens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum BudgetKind {
    /// Speicher der Maschine in Byte.
    Ram,
    /// Rechenzeit je Aktivierung; braucht `c_target` (13.8).
    Wcet,
}

/// `framing`
#[derive(Clone, Debug, PartialEq)]
#[allow(missing_docs)]
pub enum Framing {
    Raw,
    Lines,
    Cobs,
    LengthPrefixed(Ident),
    Fixed(IntLit),
}

/// `overflow = …`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum Overflow {
    Fault,
    DropOldest,
    Drop,
}

/// `stream_decl` (interner Stream)
#[derive(Clone, Debug, PartialEq)]
pub struct StreamDecl {
    /// Elementtyp.
    pub elem: ElemType,
    /// Name.
    pub name: Ident,
    /// Attribute.
    pub attrs: Vec<Attr>,
    /// Position.
    pub span: Span,
}

/// `port_decl`
#[derive(Clone, Debug, PartialEq)]
pub struct PortDecl {
    /// Name.
    pub name: Ident,
    /// Registerrecord.
    pub regs: Ident,
    /// Adresse.
    pub address: IntLit,
    /// Position.
    pub span: Span,
}

/// `command_decl`
#[derive(Clone, Debug, PartialEq)]
pub struct CommandDecl {
    /// Name.
    pub name: Ident,
    /// Attribute.
    pub attrs: Vec<Attr>,
    /// Position.
    pub span: Span,
}

// ---------------------------------------------------------------- Funktionen, Natives, Bloecke

/// `fn_decl`
#[derive(Clone, Debug, PartialEq)]
pub struct FnDecl {
    /// Name.
    pub name: Ident,
    /// Generische Variablen.
    pub generics: Vec<GenericVar>,
    /// Parameter.
    pub params: Vec<Param>,
    /// Rueckgabetyp; fehlt nur bei `inout` (3.9).
    pub ret: Option<Type>,
    /// Rumpf.
    pub body: Block,
    /// Position.
    pub span: Span,
}

/// `native_decl`
#[derive(Clone, Debug, PartialEq)]
pub struct NativeDecl {
    /// `fn` oder `job`.
    pub kind: NativeKind,
    /// Name.
    pub name: Ident,
    /// Generische Variablen.
    pub generics: Vec<GenericVar>,
    /// Parameter.
    pub params: Vec<Param>,
    /// Rueckgabetyp.
    pub ret: Type,
    /// Projekt-Native aus Datei.
    pub from: Option<StrLit>,
    /// Kostenvertrag.
    pub cost: CostSpec,
    /// Stack-Vertrag.
    pub stack: IntLit,
    /// Worst-Case-Dauer eines Jobs.
    pub duration: Option<DurationLit>,
    /// Position.
    pub span: Span,
}

/// `native fn | native job`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum NativeKind {
    Fn,
    Job,
}

/// `cost_spec`
#[derive(Clone, Debug, PartialEq)]
pub enum CostSpec {
    /// Ein Wert, Klasse `i32`.
    Single(IntLit),
    /// Vektor je Klasse.
    Classes(Vec<(CostClass, IntLit)>),
}

/// `cost_class`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum CostClass {
    I32,
    I64,
    F32,
    F64,
    Mem,
    Call,
    Native,
}

/// `block_decl`
#[derive(Clone, Debug, PartialEq)]
pub struct BlockDecl {
    /// Name.
    pub name: Ident,
    /// Generische Variablen.
    pub generics: Vec<GenericVar>,
    /// Konstruktionsparameter.
    pub params: Vec<Param>,
    /// Zustandsvariablen.
    pub vars: Vec<VarDecl>,
    /// `step`
    pub step: Option<StepDecl>,
    /// Weitere Methoden.
    pub methods: Vec<MethodDecl>,
    /// Position.
    pub span: Span,
}

/// `step_decl`
#[derive(Clone, Debug, PartialEq)]
pub struct StepDecl {
    /// Parameter.
    pub params: Vec<Param>,
    /// Rueckgabetyp.
    pub ret: Type,
    /// Rumpf.
    pub body: Block,
    /// Position.
    pub span: Span,
}

/// `method_decl`
#[derive(Clone, Debug, PartialEq)]
pub struct MethodDecl {
    /// Name.
    pub name: Ident,
    /// Parameter.
    pub params: Vec<Param>,
    /// Rueckgabetyp.
    pub ret: Option<Type>,
    /// Rumpf.
    pub body: Block,
    /// Position.
    pub span: Span,
}

/// `gvar`
#[derive(Clone, Debug, PartialEq)]
pub enum GenericVar {
    /// Einheitenvariable `U`.
    Unit(Ident),
    /// `type T: capability`
    Type {
        /// Name.
        name: Ident,
        /// Faehigkeit.
        capability: Option<Capability>,
    },
    /// `const N in a..b`
    Const {
        /// Name.
        name: Ident,
        /// Range.
        range: Option<Range>,
    },
}

/// `capability`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum Capability {
    Pod,
    Eq,
    Ord,
    Numeric,
    Integer,
    Float,
}

/// `param`
#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    /// `inout`
    pub inout: bool,
    /// Name.
    pub name: Ident,
    /// Channel-Richtung bei Maschinenparametern.
    pub dir: Option<Direction>,
    /// Typ.
    pub ty: Type,
    /// Default.
    pub default: Option<Expr>,
    /// Position.
    pub span: Span,
}

// ---------------------------------------------------------------- Maschinen

/// `machine_decl`
#[derive(Clone, Debug, PartialEq)]
pub struct MachineDecl {
    /// `driver machine`
    pub driver: bool,
    /// Name.
    pub name: Ident,
    /// Parameter eines Templates.
    pub params: Vec<Param>,
    /// `follows`
    pub follows: Vec<Ident>,
    /// Knoten.
    pub node: Option<Ident>,
    /// Periode.
    pub every: Option<DurationLit>,
    /// Phase.
    pub phase: Option<DurationLit>,
    /// Metadaten.
    pub attrs: Vec<Attr>,
    /// Rumpf.
    pub body: MachineBody,
    /// Position.
    pub span: Span,
}

/// `machine_body`
#[derive(Clone, Debug, PartialEq)]
pub struct MachineBody {
    /// Variablen, Persistenz, Signale, Fault-Ziel in Quelltextreihenfolge.
    pub prelude: Vec<MachinePrelude>,
    /// `initial`
    pub initial: Ident,
    /// Maschinenweiter `loop:`.
    pub loop_block: Option<Block>,
    /// Handler.
    pub handlers: Vec<OnHandler>,
    /// Zustaende.
    pub states: Vec<StateDecl>,
    /// Position.
    pub span: Span,
}

/// Eintrag vor `initial` in einer Maschine.
#[derive(Clone, Debug, PartialEq)]
#[allow(missing_docs)]
pub enum MachinePrelude {
    Var(VarDecl),
    Persist(PersistDecl),
    Signal(Ident),
    Fault(Ident),
}

/// `persist_decl`
#[derive(Clone, Debug, PartialEq)]
pub struct PersistDecl {
    /// Name.
    pub name: Ident,
    /// Typ.
    pub ty: Type,
    /// Default.
    pub value: Expr,
    /// `min_interval`
    pub min_interval: Option<DurationLit>,
    /// Position.
    pub span: Span,
}

/// `state_decl`
#[derive(Clone, Debug, PartialEq)]
pub struct StateDecl {
    /// Name.
    pub name: Ident,
    /// `idle`
    pub idle: bool,
    /// `resume`
    pub resume: bool,
    /// Metadaten.
    pub attrs: Vec<Attr>,
    /// Rumpf.
    pub body: StateBody,
    /// Position.
    pub span: Span,
}

/// `state_body`
#[derive(Clone, Debug, PartialEq, Default)]
pub struct StateBody {
    /// Fault-Ziel, Variablen, gescopte Instanzen in Quelltextreihenfolge.
    pub prelude: Vec<StatePrelude>,
    /// `initial` bei Kindzustaenden.
    pub initial: Option<Ident>,
    /// `enter:`
    pub enter: Option<Block>,
    /// `loop:`
    pub loop_block: Option<Block>,
    /// Handler.
    pub handlers: Vec<OnHandler>,
    /// `sequence:`
    pub sequence: Option<Vec<SeqItem>>,
    /// Uebergaenge.
    pub transitions: Vec<Transition>,
    /// `exit:`
    pub exit: Option<Block>,
    /// Kindzustaende.
    pub states: Vec<StateDecl>,
}

/// Eintrag vor `initial` in einem Zustand.
#[derive(Clone, Debug, PartialEq)]
#[allow(missing_docs)]
pub enum StatePrelude {
    Fault(Ident),
    Var(VarDecl),
    Instance(InstanceDecl),
}

/// `on_handler`
#[derive(Clone, Debug, PartialEq)]
pub struct OnHandler {
    /// Stream.
    pub stream: Ident,
    /// Muster.
    pub pattern: Option<(MatchKind, Pattern)>,
    /// Bindung.
    pub binding: Option<Ident>,
    /// Rumpf.
    pub body: Block,
    /// Position.
    pub span: Span,
}

/// `matches | has`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum MatchKind {
    Matches,
    Has,
}

/// `transition`
#[derive(Clone, Debug, PartialEq)]
pub struct Transition {
    /// `when guard` oder `after d`.
    pub trigger: Trigger,
    /// Aktionen vor dem Uebergang.
    pub actions: Vec<Stmt>,
    /// Ziel.
    pub target: Ident,
    /// Position.
    pub span: Span,
}

/// Ausloeser eines Uebergangs.
#[derive(Clone, Debug, PartialEq)]
#[allow(missing_docs)]
pub enum Trigger {
    When(Guard),
    After(Expr),
}

/// `guard`; Musterguards stecken als `ExprKind::Match` im Ausdruck.
#[derive(Clone, Debug, PartialEq)]
pub enum Guard {
    /// Bedingung (auch `x matches P as m`).
    Expr(Expr),
    /// `s as e`: naechstes Element eines Streams.
    Next {
        /// Stream oder Handle.
        subject: Expr,
        /// Bindung.
        binding: Ident,
    },
}

/// `seq_item`
#[derive(Clone, Debug, PartialEq)]
#[allow(missing_docs)]
pub enum SeqItem {
    Stmt(Stmt),
    Wait(Expr),
    Until { guard: Guard, timeout: Option<Timeout>, span: Span },
    Expect { cond: Expr, message: Option<StrLit>, span: Span },
    Repeat { count: Expr, body: Vec<SeqItem>, span: Span },
    Step { name: StrLit, body: Vec<SeqItem>, span: Span },
}

/// `timeout d [-> X | else: …]`
#[derive(Clone, Debug, PartialEq)]
pub struct Timeout {
    /// Frist.
    pub duration: Expr,
    /// Reaktion.
    pub action: TimeoutAction,
}

/// Reaktion auf einen Timeout.
#[derive(Clone, Debug, PartialEq)]
pub enum TimeoutAction {
    /// Fault-Ziel des Zustands (6.2).
    Fault,
    /// `-> ZIEL`
    Goto(Ident),
    /// `else:` Aktionsblock (weicher Timeout).
    Else(Block),
}

/// `instance_decl`
#[derive(Clone, Debug, PartialEq)]
pub struct InstanceDecl {
    /// Name.
    pub name: Ident,
    /// `[i in a..b]`
    pub index: Option<(Ident, Range)>,
    /// `resume`
    pub resume: bool,
    /// Template.
    pub template: Ident,
    /// Argumente.
    pub args: Vec<Arg>,
    /// Position.
    pub span: Span,
}

/// `node_decl`
#[derive(Clone, Debug, PartialEq)]
pub struct NodeDecl {
    /// Name.
    pub name: Ident,
    /// Adresse.
    pub address: StrLit,
    /// Knotentick.
    pub tick: Option<DurationLit>,
    /// Position.
    pub span: Span,
}

/// Art einer Eigenschaftsdeklaration (13.3).
///
/// `property` und `assumption` tragen dieselbe Temporallogik und denselben
/// Monitor; sie unterscheiden sich nur in der Beweisrichtung — eine
/// Eigenschaft ist zu *zeigen*, eine Annahme darf *vorausgesetzt* werden.
/// Ein gemeinsamer Knoten haelt beide Seiten automatisch im Gleichschritt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropertyKind {
    /// `property p: …` — Beweisziel und Monitor.
    Property,
    /// `assumption a: …` — Umgebungsannahme, beschraenkt die
    /// Beweisverpflichtung und wird beobachtet.
    Assumption,
}

impl PropertyKind {
    /// Das Schluesselwort.
    pub fn word(self) -> &'static str {
        match self {
            PropertyKind::Property => "property",
            PropertyKind::Assumption => "assumption",
        }
    }
}

/// `property_decl` und `assumption_decl`
#[derive(Clone, Debug, PartialEq)]
pub struct PropertyDecl {
    /// Art.
    pub kind: PropertyKind,
    /// Name.
    pub name: Ident,
    /// Eigenschaft: ein Ausdruck mit Temporaloperatoren und `implies`.
    pub prop: Expr,
    /// `with monitor = true`
    pub monitor: bool,
    /// Position.
    pub span: Span,
}

/// Temporaloperator in einer Eigenschaft (13.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum TemporalOp {
    Always,
    Never,
    Eventually,
    Stable,
    Once,
}

/// `scenario_decl`
#[derive(Clone, Debug, PartialEq)]
pub struct ScenarioDecl {
    /// Name.
    pub name: StrLit,
    /// Periode.
    pub every: Option<DurationLit>,
    /// Rumpf.
    pub body: MachineBody,
    /// Position.
    pub span: Span,
}

/// `campaign_decl`
#[derive(Clone, Debug, PartialEq)]
pub struct CampaignDecl {
    /// Name.
    pub name: Ident,
    /// Eintraege.
    pub items: Vec<CampaignItem>,
    /// Position.
    pub span: Span,
}

/// `campaign_item`
#[derive(Clone, Debug, PartialEq)]
#[allow(missing_docs)]
pub enum CampaignItem {
    Program(StrLit),
    Profile(Ident),
    SweepRange { param: Ident, from: Expr, to: Expr, step: Expr },
    SweepList { param: Ident, values: Vec<Expr> },
    Repeat(IntLit),
    StopOn(StopOn),
}

/// `stop_on fail | never`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum StopOn {
    Fail,
    Never,
}

/// `trigger_decl`
#[derive(Clone, Debug, PartialEq)]
pub struct TriggerDecl {
    /// Name.
    pub name: Ident,
    /// Knoten.
    pub node: Option<Ident>,
    /// Guard.
    pub when: Guard,
    /// Geplante Ausgabe.
    pub then: AtStmt,
    /// Garantierte Reaktionszeit.
    pub bound: DurationLit,
    /// Position.
    pub span: Span,
}

/// `at T: …`
#[derive(Clone, Debug, PartialEq)]
pub struct AtStmt {
    /// Zeitpunkt.
    pub time: Expr,
    /// Aktionsblock.
    pub body: Block,
    /// Position.
    pub span: Span,
}

// ---------------------------------------------------------------- Statements

/// `block`; die Inline-Form ist ein Block mit einer Anweisung.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Block {
    /// Anweisungen.
    pub stmts: Vec<Stmt>,
    /// Position.
    pub span: Span,
}

/// `var_decl`
#[derive(Clone, Debug, PartialEq)]
pub struct VarDecl {
    /// `pub var`
    pub public: bool,
    /// Name.
    pub name: Ident,
    /// Typ.
    pub ty: Option<Type>,
    /// Initialwert.
    pub value: Expr,
    /// Position.
    pub span: Span,
}

/// Eine Anweisung.
#[derive(Clone, Debug, PartialEq)]
pub struct Stmt {
    /// Inhalt.
    pub kind: StmtKind,
    /// Position.
    pub span: Span,
}

/// `stmt` und `simple_stmt`
#[derive(Clone, Debug, PartialEq)]
#[allow(missing_docs)]
pub enum StmtKind {
    Assign {
        target: Expr,
        op: AssignOp,
        value: Expr,
    },
    Var(VarDecl),
    Job {
        handle: Ident,
        callee: Ident,
        args: Vec<Arg>,
    },
    Arm {
        arm: bool,
        trigger: Ident,
    },
    Check {
        cond: Expr,
        message: Option<StrLit>,
        confirm: Option<Expr>,
        /// `within d` (9.4.5): Anforderung an die Safe-State-Latenz.
        within: Option<Expr>,
        target: Option<Ident>,
        req: Option<StrLit>,
    },
    Alert {
        cond: Expr,
        message: StrLit,
        confirm: Option<Expr>,
    },
    Log(StrLit),
    Goto(Ident),
    Abort(Option<StrLit>),
    Return(Expr),
    Send {
        stream: Ident,
        value: Expr,
    },
    Pulse {
        output: Ident,
        value: Expr,
        duration: Expr,
    },
    Cancel(Ident),
    Measure {
        name: Ident,
        value: Expr,
    },
    Verify {
        cond: Expr,
        message: StrLit,
        req: Option<StrLit>,
    },
    Verdict {
        pass: bool,
        message: Option<StrLit>,
    },
    Raise(Ident),
    Break,
    Pass,
    Expr(Expr),
    If {
        branches: Vec<(Expr, Block)>,
        otherwise: Option<Block>,
    },
    For {
        target: ForTarget,
        iter: ForIter,
        body: Block,
    },
    Match {
        subject: Expr,
        cases: Vec<Case>,
    },
    At(AtStmt),
    Every {
        period: Expr,
        body: Block,
    },
}

/// `= += -= *= /=`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum AssignOp {
    Set,
    Add,
    Sub,
    Mul,
    Div,
}

/// Laufvariable(n) einer `for`-Schleife.
#[derive(Clone, Debug, PartialEq)]
pub enum ForTarget {
    /// `for x in …`
    One(Ident),
    /// `for (k, v) in …`
    Pair(Ident, Ident),
}

/// Bereich einer `for`-Schleife.
#[derive(Clone, Debug, PartialEq)]
pub enum ForIter {
    /// `range(N)`
    Range(Expr),
    /// Array, Stream, Samples, Map.
    Expr(Expr),
}

/// `case`
#[derive(Clone, Debug, PartialEq)]
pub struct Case {
    /// Muster.
    pub pattern: CasePattern,
    /// Rumpf.
    pub body: Block,
    /// Position.
    pub span: Span,
}

/// `case_pattern`
#[derive(Clone, Debug, PartialEq)]
pub enum CasePattern {
    /// Variante mit Feldbindungen.
    Variant {
        /// Name.
        name: Ident,
        /// Gebundene Felder.
        fields: Vec<Ident>,
    },
    /// `_`
    Wild,
    /// Werte und Bereiche.
    Values(Vec<CaseValue>),
}

/// Ein Wert oder Bereich in einem `case`.
#[derive(Clone, Debug, PartialEq)]
pub struct CaseValue {
    /// Wert oder Untergrenze.
    pub from: Expr,
    /// Obergrenze bei `a..b`.
    pub to: Option<Expr>,
}

// ---------------------------------------------------------------- Ausdruecke

/// Ein Ausdruck.
#[derive(Clone, Debug, PartialEq)]
pub struct Expr {
    /// Inhalt.
    pub kind: ExprKind,
    /// Position.
    pub span: Span,
}

/// `expr` bis `primary`
#[derive(Clone, Debug, PartialEq)]
#[allow(missing_docs)]
pub enum ExprKind {
    /// Zahl mit optionaler Einheit.
    Number {
        value: Number,
        unit: Option<UnitExpr>,
    },
    Duration(DurationLit),
    Str(StrLit),
    Bool(bool),
    None,
    Default,
    /// Variable oder eingebaute Groesse.
    Ident(Ident),
    /// Aufruf `f(args)`, mit expliziter Instanziierung `f[U](args)`.
    Call {
        callee: Ident,
        generics: Vec<GenericArg>,
        args: Vec<Arg>,
    },
    /// `UPPER_IDENT [(args)]`: Konstante, Parameter, Zustand oder Variante.
    Upper {
        name: Ident,
        args: Option<Vec<Arg>>,
    },
    /// `TYPE_IDENT [(args)]`: Konstruktor oder Typ vor einer Methode.
    TypeName {
        name: Ident,
        args: Option<Vec<Arg>>,
    },
    Paren(Box<Expr>),
    /// `(a, b)`: Stuetzstelle einer Tabelle.
    Tuple(Box<Expr>, Box<Expr>),
    Array(Vec<Expr>),
    /// `[N] block(args)`: Array von Blockinstanzen.
    InstanceArray {
        count: Box<Expr>,
        template: Ident,
        args: Vec<Arg>,
    },
    /// `x.name` oder `x.name(args)`.
    Member {
        base: Box<Expr>,
        name: Ident,
        args: Option<Vec<Arg>>,
    },
    Index {
        base: Box<Expr>,
        index: Box<Expr>,
    },
    Slice {
        base: Box<Expr>,
        from: Box<Expr>,
        to: Box<Expr>,
    },
    Index2 {
        base: Box<Expr>,
        row: Box<Expr>,
        col: Box<Expr>,
    },
    Cast {
        expr: Box<Expr>,
        ty: ScalarType,
    },
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    Binary {
        op: BinaryOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    /// `x matches P as m` / `x has P`.
    Match {
        subject: Box<Expr>,
        kind: MatchKind,
        pattern: Pattern,
        binding: Option<Ident>,
    },
    /// `a if c else b`
    Conditional {
        then: Box<Expr>,
        cond: Box<Expr>,
        otherwise: Box<Expr>,
    },
    /// Temporaloperator in einer Eigenschaft (13.3).
    Temporal {
        op: TemporalOp,
        window: Option<DurationLit>,
        inner: Box<Expr>,
    },
    /// `a implies b` in einer Eigenschaft.
    Implies {
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
}

/// Einstellige Operatoren.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum UnaryOp {
    Neg,
    Not,
    BitNot,
}

/// Zweistellige Operatoren.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum BinaryOp {
    Or,
    And,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    BitOr,
    BitXor,
    BitAnd,
    Shl,
    Shr,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

/// `arg`
#[derive(Clone, Debug, PartialEq)]
pub struct Arg {
    /// Benannt (`x = …`).
    pub name: Option<Ident>,
    /// Wert.
    pub value: Expr,
    /// Position.
    pub span: Span,
}

/// `generic_arg`; die Klasse folgt der Namensform (3.12).
#[derive(Clone, Debug, PartialEq)]
pub enum GenericArg {
    /// Einheit oder Einheitenvariable.
    Unit(UnitExpr),
    /// Typ.
    Type(Type),
    /// Konstante.
    Const(Expr),
}

/// `pattern`
#[derive(Clone, Debug, PartialEq)]
pub enum Pattern {
    /// Musterliteral (8.7).
    Text(StrLit),
    /// Record-Muster `CanFrame(id = 0x7E8)`.
    Record {
        /// Recordtyp.
        ty: Ident,
        /// Feldbedingungen.
        fields: Vec<(Ident, Expr)>,
    },
}

// ---------------------------------------------------------------- Typausdruecke

/// Ein Typ.
#[derive(Clone, Debug, PartialEq)]
pub struct Type {
    /// Inhalt.
    pub kind: TypeKind,
    /// Position.
    pub span: Span,
}

/// `type`
#[derive(Clone, Debug, PartialEq)]
#[allow(missing_docs)]
pub enum TypeKind {
    Scalar {
        scalar: ScalarType,
        range: Option<Range>,
        wrap: Option<Wrap>,
    },
    Array {
        len: Box<Expr>,
        elem: Box<Type>,
    },
    Named {
        name: Ident,
        wrap: Option<Wrap>,
    },
    Bytes(Box<Expr>),
    Vec {
        elem: Box<Type>,
        len: Box<Expr>,
    },
    Line(Box<Expr>),
    Stream(Box<ElemType>),
    Samples {
        elem: Box<Type>,
        len: Box<Expr>,
    },
    Table {
        key: Box<Type>,
        value: Box<Type>,
    },
    Mat {
        rows: Box<Expr>,
        cols: Box<Expr>,
        unit: Option<UnitExpr>,
    },
    MatDim {
        rows: UnitTuple,
        cols: UnitTuple,
    },
    VecDim(UnitTuple),
    Map {
        key: Box<Type>,
        value: Box<Type>,
        len: Box<Expr>,
    },
    /// Typvariable `T`.
    TypeVar {
        name: Ident,
        wrap: Option<Wrap>,
    },
    /// `bytes<N>?`, `vec<T, N>!E`: die Huelle um einen Puffer oder eine
    /// Sammlung (3.8; FB-94).
    Wrapped {
        inner: Box<Type>,
        wrap: Wrap,
    },
}

/// `?` oder `!E`
#[derive(Clone, Debug, PartialEq)]
pub enum Wrap {
    /// `T?`
    Optional,
    /// `T!E`
    Result(Ident),
}

/// `scalar_type`
#[derive(Clone, Debug, PartialEq)]
#[allow(missing_docs)]
pub enum ScalarType {
    Bool,
    Int { ty: IntType, unit: Option<UnitExpr> },
    Float { width: Option<FloatWidth>, unit: Option<UnitExpr> },
    Duration,
    Str(Box<Expr>),
}

/// `int_type`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum IntType {
    Int,
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
}

/// `elem_type`
#[derive(Clone, Debug, PartialEq)]
#[allow(missing_docs)]
pub enum ElemType {
    U8,
    Bytes(Expr),
    Line(Expr),
    Edge,
    Named(Ident),
    Capture { elem: Box<Type>, len: Expr },
}

/// `range`
#[derive(Clone, Debug, PartialEq)]
pub struct Range {
    /// Untergrenze.
    pub from: Box<Expr>,
    /// Obergrenze.
    pub to: Box<Expr>,
    /// Position.
    pub span: Span,
}

/// Ein Schnipsel: Deklarationen, Zustandsinhalte und Anweisungen in beliebiger
/// Folge (Testeinstieg fuer die Codebloecke der Referenz).
#[derive(Clone, Debug, PartialEq)]
#[allow(missing_docs)]
pub enum SnippetItem {
    Item(Item),
    MachinePrelude(MachinePrelude),
    Initial(Ident),
    Enter(Block),
    Exit(Block),
    Loop(Block),
    On(OnHandler),
    Sequence(Vec<SeqItem>),
    Transition(Transition),
    State(StateDecl),
    Step(StepDecl),
    Seq(SeqItem),
    Instance(InstanceDecl),
}
