//! Funktionen, native Funktionen und Bloecke (Referenz 3.9, 3.12, 4.5, 5.7).

use takt_diag::Span;

use crate::expr::Expr;
use crate::ids::*;
use crate::machine::VarDef;
use crate::stmt::Block;

/// Kostenvektor ueber den sieben Operationsklassen (9.4.3), mit den
/// Divisionen je Zahlklasse.
///
/// **Division hat eigene Gewichte** (7.2): Je nach Kern ist sie ein
/// Hardwarebefehl oder ein Bibliotheksaufruf, und sie kostet ein
/// Vielfaches einer Addition — auf dem RV32IMAC rund das Dreissigfache.
/// Eine Division zaehlt darum weiter in ihrer Klasse, wie 9.4.3 es sagt,
/// und die Felder `*_div` halten fest, wie viele der Operationen einer
/// Klasse Divisionen (und Reste) sind. Die Zeitschranke gewichtet sie mit
/// dem eigenen Gewicht aus der Kalibrierung (13.8).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[allow(missing_docs)]
pub struct CostVec {
    pub i32: u64,
    pub i64: u64,
    pub f32: u64,
    pub f64: u64,
    pub mem: u64,
    pub call: u64,
    pub native: u64,
    /// Davon Divisionen in `i32`.
    pub i32_div: u64,
    /// Davon Divisionen in `i64`.
    pub i64_div: u64,
    /// Davon Divisionen in `f32`.
    pub f32_div: u64,
    /// Davon Divisionen in `f64`.
    pub f64_div: u64,
}

impl std::ops::Add for CostVec {
    type Output = CostVec;

    /// Komponentenweise Summe (`N(s1; s2)`, 9.4.3), saettigend: Eine
    /// Schranke, die ueberlaeuft, waere keine mehr.
    fn add(self, o: CostVec) -> CostVec {
        CostVec {
            i32: self.i32.saturating_add(o.i32),
            i64: self.i64.saturating_add(o.i64),
            f32: self.f32.saturating_add(o.f32),
            f64: self.f64.saturating_add(o.f64),
            mem: self.mem.saturating_add(o.mem),
            call: self.call.saturating_add(o.call),
            native: self.native.saturating_add(o.native),
            i32_div: self.i32_div.saturating_add(o.i32_div),
            i64_div: self.i64_div.saturating_add(o.i64_div),
            f32_div: self.f32_div.saturating_add(o.f32_div),
            f64_div: self.f64_div.saturating_add(o.f64_div),
        }
    }
}

impl CostVec {
    /// Keine Operation.
    pub const ZERO: CostVec = CostVec {
        i32: 0,
        i64: 0,
        f32: 0,
        f64: 0,
        mem: 0,
        call: 0,
        native: 0,
        i32_div: 0,
        i64_div: 0,
        f32_div: 0,
        f64_div: 0,
    };

    /// Das `n`-fache (`n · N(s)`, 9.4.3), saettigend wie die Summe.
    pub fn times(self, n: u64) -> CostVec {
        CostVec {
            i32: self.i32.saturating_mul(n),
            i64: self.i64.saturating_mul(n),
            f32: self.f32.saturating_mul(n),
            f64: self.f64.saturating_mul(n),
            mem: self.mem.saturating_mul(n),
            call: self.call.saturating_mul(n),
            native: self.native.saturating_mul(n),
            i32_div: self.i32_div.saturating_mul(n),
            i64_div: self.i64_div.saturating_mul(n),
            f32_div: self.f32_div.saturating_mul(n),
            f64_div: self.f64_div.saturating_mul(n),
        }
    }

    /// Komponentenweises Maximum (`max` ueber Zweige, 9.4.3).
    ///
    /// Auch die Divisionen gehen komponentenweise: Die Schranke bleibt eine,
    /// solange eine Division nicht billiger gewichtet ist als ihre Klasse —
    /// das haelt [`crate::hardware::CTarget`] fest.
    pub fn max(self, o: CostVec) -> CostVec {
        CostVec {
            i32: self.i32.max(o.i32),
            i64: self.i64.max(o.i64),
            f32: self.f32.max(o.f32),
            f64: self.f64.max(o.f64),
            mem: self.mem.max(o.mem),
            call: self.call.max(o.call),
            native: self.native.max(o.native),
            i32_div: self.i32_div.max(o.i32_div),
            i64_div: self.i64_div.max(o.i64_div),
            f32_div: self.f32_div.max(o.f32_div),
            f64_div: self.f64_div.max(o.f64_div),
        }
    }

    /// Wie viele Operationen einer Klasse Divisionen sind; null ausserhalb
    /// der vier Zahlklassen.
    pub fn divisions(self, c: CostClass) -> u64 {
        match c {
            CostClass::I32 => self.i32_div,
            CostClass::I64 => self.i64_div,
            CostClass::F32 => self.f32_div,
            CostClass::F64 => self.f64_div,
            CostClass::Mem | CostClass::Call | CostClass::Native => 0,
        }
    }

    /// Eine Operation der Klasse `c`.
    pub fn op(c: CostClass) -> CostVec {
        let one = CostVec::default();
        match c {
            CostClass::I32 => CostVec { i32: 1, ..one },
            CostClass::I64 => CostVec { i64: 1, ..one },
            CostClass::F32 => CostVec { f32: 1, ..one },
            CostClass::F64 => CostVec { f64: 1, ..one },
            CostClass::Mem => CostVec { mem: 1, ..one },
            CostClass::Call => CostVec { call: 1, ..one },
            CostClass::Native => CostVec { native: 1, ..one },
        }
    }

    /// Eine Operation der Klasse `c`, die eine Division ist.
    pub fn division(c: CostClass) -> CostVec {
        let one = CostVec::default();
        match c {
            CostClass::I32 => CostVec { i32: 1, i32_div: 1, ..one },
            CostClass::I64 => CostVec { i64: 1, i64_div: 1, ..one },
            CostClass::F32 => CostVec { f32: 1, f32_div: 1, ..one },
            CostClass::F64 => CostVec { f64: 1, f64_div: 1, ..one },
            CostClass::Mem | CostClass::Call | CostClass::Native => one,
        }
    }

    /// Der Wert einer Klasse.
    pub fn of(self, c: CostClass) -> u64 {
        match c {
            CostClass::I32 => self.i32,
            CostClass::I64 => self.i64,
            CostClass::F32 => self.f32,
            CostClass::F64 => self.f64,
            CostClass::Mem => self.mem,
            CostClass::Call => self.call,
            CostClass::Native => self.native,
        }
    }

    /// Summe ueber alle Klassen; eine Division zaehlt einmal, in ihrer Klasse.
    ///
    /// Nur fuer Vergleiche und Anteile im Bericht: Operationen
    /// verschiedener Klassen kosten verschieden viel, und was sie in Zeit
    /// bedeuten, sagt erst `c_target` (13.8). Eine Summe ueber Klassen ist
    /// darum eine Ordnungsgroesse, keine Zeitaussage.
    pub fn sum(self) -> u64 {
        CostClass::ALL.iter().map(|c| self.of(*c)).sum()
    }

    /// Ist der Vektor ueberall null?
    pub fn is_zero(self) -> bool {
        self.sum() == 0
    }
}

/// Die sieben Operationsklassen (9.4.3, Grammatik `cost_class`).
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

impl CostClass {
    /// Alle Klassen in der Reihenfolge der Referenz (9.4.3).
    pub const ALL: [CostClass; 7] = [
        CostClass::I32,
        CostClass::I64,
        CostClass::F32,
        CostClass::F64,
        CostClass::Mem,
        CostClass::Call,
        CostClass::Native,
    ];

    /// Der Name, wie ihn die Grammatik schreibt (`cost_class`).
    pub fn name(self) -> &'static str {
        match self {
            CostClass::I32 => "i32",
            CostClass::I64 => "i64",
            CostClass::F32 => "f32",
            CostClass::F64 => "f64",
            CostClass::Mem => "mem",
            CostClass::Call => "call",
            CostClass::Native => "native",
        }
    }
}

/// Argument einer monomorphisierten Instanz (3.12).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GenericArgVal {
    /// Einheitenvariable.
    Unit(UnitId),
    /// Typvariable (v1.2).
    Type(TypeId),
    /// Konstantenvariable (v1.1).
    Const(i64),
}

/// Herkunft einer monomorphisierten Instanz.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenericOrigin {
    /// Name der generischen Vorlage.
    pub template: String,
    /// Argumente in Deklarationsreihenfolge.
    pub args: Vec<GenericArgVal>,
}

/// Parameter einer Funktion, eines Blocks oder einer nativen Funktion.
#[derive(Clone, Debug, PartialEq)]
pub struct FnParam {
    /// Name.
    pub name: String,
    /// Typ.
    pub ty: TypeId,
    /// `inout` (3.9): Zucker fuer eine Rueckgabe.
    pub inout: bool,
    /// Default (Maschinen- und Blockparameter).
    pub default: Option<Expr>,
    /// Position.
    pub span: Span,
}

/// Reine Funktion oder Blockmethode (3.9); eine Instanz je Monomorphisierung.
#[derive(Clone, Debug, PartialEq)]
pub struct Fn {
    /// Name; bei Instanzen mit Argumenten dekoriert.
    pub name: String,
    /// Parameter.
    pub params: Vec<FnParam>,
    /// Rueckgabetyp; fehlt bei reinen `inout`-Funktionen.
    pub ret: Option<TypeId>,
    /// Lokale Variablen (Parameter zuerst, in ihrer Reihenfolge).
    pub locals: Vec<VarDef>,
    /// Rumpf.
    pub body: Block,
    /// Kosten `N(body)` (9.4.3), aus M3.
    pub cost: Option<CostVec>,
    /// Stack-Bedarf in Bytes, aus M3.
    pub stack: Option<u32>,
    /// Generische Vorlage und Argumente.
    pub origin: Option<GenericOrigin>,
    /// Position.
    pub span: Span,
}

/// `native fn` oder `native job` (4.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum NativeKind {
    Fn,
    Job,
}

/// Native Funktion mit Kostenvertrag (4.5).
#[derive(Clone, Debug, PartialEq)]
pub struct Native {
    /// Art.
    pub kind: NativeKind,
    /// Name.
    pub name: String,
    /// Parameter.
    pub params: Vec<FnParam>,
    /// Rueckgabetyp.
    pub ret: TypeId,
    /// Deklarierter Kostenvektor.
    pub cost: CostVec,
    /// Deklarierter Stack-Bedarf in Bytes.
    pub stack: u32,
    /// Worst-Case-Dauer eines Jobs in Nanosekunden.
    pub duration: Option<i64>,
    /// `total` zugesichert.
    pub total: bool,
    /// Projekt-Native aus Datei (v1.1).
    pub from: Option<String>,
    /// Generische Vorlage und Argumente.
    pub origin: Option<GenericOrigin>,
    /// Position.
    pub span: Span,
}

/// Block (5.7): Zustand plus `step` und Methoden; Instanzen leben im
/// Maschinenspeicher (`ExprKind::BlockInit`).
#[derive(Clone, Debug, PartialEq)]
pub struct BlockDef {
    /// Name.
    pub name: String,
    /// Konstruktionsparameter.
    pub params: Vec<FnParam>,
    /// Zustandsvariablen mit Initialwerten.
    pub state_vars: Vec<VarDef>,
    /// `step`.
    pub step: Option<FnId>,
    /// Weitere Methoden.
    pub methods: Vec<FnId>,
    /// `requires` des `step` (5.7): Beweisverpflichtungen ueber den
    /// Parametern des Schritts und dem Zustand, im Rahmen des `step`.
    pub requires: Vec<Expr>,
    /// `ensures` des `step`: ueber `result` (die Lokale hinter den
    /// Parametern), den Parametern und dem Zustand danach.
    pub ensures: Vec<Expr>,
    /// Generische Vorlage und Argumente.
    pub origin: Option<GenericOrigin>,
    /// Position.
    pub span: Span,
}
