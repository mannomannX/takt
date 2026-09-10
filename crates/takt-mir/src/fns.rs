//! Funktionen, native Funktionen und Bloecke (Referenz 3.9, 3.12, 4.5, 5.7).

use takt_diag::Span;

use crate::expr::Expr;
use crate::ids::*;
use crate::machine::VarDef;
use crate::stmt::Block;

/// Kostenvektor ueber den sieben Operationsklassen (9.4.3).
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
}

impl std::ops::Add for CostVec {
    type Output = CostVec;

    /// Komponentenweise Summe (`N(s1; s2)`, 9.4.3).
    fn add(self, o: CostVec) -> CostVec {
        CostVec {
            i32: self.i32 + o.i32,
            i64: self.i64 + o.i64,
            f32: self.f32 + o.f32,
            f64: self.f64 + o.f64,
            mem: self.mem + o.mem,
            call: self.call + o.call,
            native: self.native + o.native,
        }
    }
}

impl CostVec {
    /// Komponentenweises Maximum (`max` ueber Zweige, 9.4.3).
    pub fn max(self, o: CostVec) -> CostVec {
        CostVec {
            i32: self.i32.max(o.i32),
            i64: self.i64.max(o.i64),
            f32: self.f32.max(o.f32),
            f64: self.f64.max(o.f64),
            mem: self.mem.max(o.mem),
            call: self.call.max(o.call),
            native: self.native.max(o.native),
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
    /// Generische Vorlage und Argumente.
    pub origin: Option<GenericOrigin>,
    /// Position.
    pub span: Span,
}
