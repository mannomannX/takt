//! Funktionen, native Funktionen und Bloecke (Referenz 3.9, 3.12, 4.5, 5.7).

use takt_diag::Span;

use crate::expr::Expr;
use crate::ids::*;
use crate::machine::VarDef;
use crate::stmt::Block;

/// Kostenvektor ueber den sieben Operationsklassen (9.4.3), mit den
/// Operationen eigenen Gewichts je Zahlklasse.
///
/// **Division, `fma` und `sqrt` haben eigene Gewichte** (7.2): Je nach Kern
/// sind sie ein Hardwarebefehl oder ein Bibliotheksaufruf, und ohne FPU
/// oder Dividierer kosten sie ein Vielfaches einer Addition. Eine solche
/// Operation zaehlt weiter in ihrer Klasse, wie 9.4.3 es sagt, und die
/// Felder `*_div`, `*_fma` und `*_sqrt` halten fest, wie viele der
/// Operationen einer Klasse von welcher Art ([`Heavy`]) sind. Die
/// Zeitschranke gewichtet sie mit dem eigenen Gewicht aus der Kalibrierung
/// (13.8).
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
    /// Davon `fma` in `f32`.
    pub f32_fma: u64,
    /// Davon `fma` in `f64`.
    pub f64_fma: u64,
    /// Davon Wurzeln in `f32`.
    pub f32_sqrt: u64,
    /// Davon Wurzeln in `f64`.
    pub f64_sqrt: u64,
}

impl std::ops::Add for CostVec {
    type Output = CostVec;

    /// Komponentenweise Summe (`N(s1; s2)`, 9.4.3), saettigend: Eine
    /// Schranke, die ueberlaeuft, waere keine mehr.
    fn add(self, o: CostVec) -> CostVec {
        self.zip(o, u64::saturating_add)
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
        f32_fma: 0,
        f64_fma: 0,
        f32_sqrt: 0,
        f64_sqrt: 0,
    };

    /// `f` komponentenweise ueber beide Vektoren.
    pub fn zip(self, o: CostVec, f: impl std::ops::Fn(u64, u64) -> u64) -> CostVec {
        CostVec {
            i32: f(self.i32, o.i32),
            i64: f(self.i64, o.i64),
            f32: f(self.f32, o.f32),
            f64: f(self.f64, o.f64),
            mem: f(self.mem, o.mem),
            call: f(self.call, o.call),
            native: f(self.native, o.native),
            i32_div: f(self.i32_div, o.i32_div),
            i64_div: f(self.i64_div, o.i64_div),
            f32_div: f(self.f32_div, o.f32_div),
            f64_div: f(self.f64_div, o.f64_div),
            f32_fma: f(self.f32_fma, o.f32_fma),
            f64_fma: f(self.f64_fma, o.f64_fma),
            f32_sqrt: f(self.f32_sqrt, o.f32_sqrt),
            f64_sqrt: f(self.f64_sqrt, o.f64_sqrt),
        }
    }

    /// Das `n`-fache (`n · N(s)`, 9.4.3), saettigend wie die Summe.
    pub fn times(self, n: u64) -> CostVec {
        self.zip(CostVec::ZERO, |a, _| a.saturating_mul(n))
    }

    /// Komponentenweises Maximum (`max` ueber Zweige, 9.4.3).
    ///
    /// Auch die Operationen eigenen Gewichts gehen komponentenweise: Die
    /// Schranke bleibt eine, solange keine von ihnen leichter gewichtet ist
    /// als ihre Klasse — das haelt [`crate::hardware::CTarget`] fest.
    pub fn max(self, o: CostVec) -> CostVec {
        self.zip(o, u64::max)
    }

    /// Das Feld der Art `h` in der Klasse `c`, wo es die Art dort gibt.
    fn heavy_mut(&mut self, h: Heavy, c: CostClass) -> Option<&mut u64> {
        match (h, c) {
            (Heavy::Div, CostClass::I32) => Some(&mut self.i32_div),
            (Heavy::Div, CostClass::I64) => Some(&mut self.i64_div),
            (Heavy::Div, CostClass::F32) => Some(&mut self.f32_div),
            (Heavy::Div, CostClass::F64) => Some(&mut self.f64_div),
            (Heavy::Fma, CostClass::F32) => Some(&mut self.f32_fma),
            (Heavy::Fma, CostClass::F64) => Some(&mut self.f64_fma),
            (Heavy::Sqrt, CostClass::F32) => Some(&mut self.f32_sqrt),
            (Heavy::Sqrt, CostClass::F64) => Some(&mut self.f64_sqrt),
            _ => None,
        }
    }

    /// Wie viele Operationen der Klasse `c` von der Art `h` sind; null, wo
    /// es die Art in der Klasse nicht gibt.
    pub fn heavy(mut self, h: Heavy, c: CostClass) -> u64 {
        self.heavy_mut(h, c).map_or(0, |n| *n)
    }

    /// Die gewoehnlichen Operationen einer Klasse: alle ausser denen
    /// eigenen Gewichts.
    pub fn ordinary(self, c: CostClass) -> u64 {
        Heavy::ALL.iter().fold(self.of(c), |n, h| n.saturating_sub(self.heavy(*h, c)))
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

    /// Eine Operation der Klasse `c` von der Art `h`; wo es die Art in der
    /// Klasse nicht gibt, eine gewoehnliche.
    pub fn heavy_op(h: Heavy, c: CostClass) -> CostVec {
        let mut v = CostVec::op(c);
        if let Some(n) = v.heavy_mut(h, c) {
            *n = 1;
        }
        v
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

    /// Summe ueber alle Klassen; eine Operation eigenen Gewichts zaehlt
    /// einmal, in ihrer Klasse.
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

/// Operationen mit eigenem Gewicht (7.2): Je nach Kern sind sie ein
/// Hardwarebefehl oder ein Bibliotheksaufruf. Sie zaehlen in ihrer Klasse
/// und daneben fuer sich ([`CostVec::heavy`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Heavy {
    /// Division und Rest.
    Div,
    /// `fma(a, b, c)`, korrekt gerundet (4.2), auch in den Skalarprodukten
    /// der Matrizen.
    Fma,
    /// Die Quadratwurzel, korrekt gerundet (4.2).
    Sqrt,
}

impl Heavy {
    /// Alle Arten.
    pub const ALL: [Heavy; 3] = [Heavy::Div, Heavy::Fma, Heavy::Sqrt];

    /// Gibt es die Art in der Klasse? Division in allen vier Zahlklassen,
    /// `fma` und `sqrt` nur im Fliesskomma.
    pub fn exists_in(self, c: CostClass) -> bool {
        match self {
            Heavy::Div => matches!(c, CostClass::I32 | CostClass::I64 | CostClass::F32 | CostClass::F64),
            Heavy::Fma | Heavy::Sqrt => matches!(c, CostClass::F32 | CostClass::F64),
        }
    }

    /// Die Endung ihres Schluessels in der Hardware-Konfiguration
    /// (`i32_div`, `f32_fma`, `f64_sqrt`).
    pub fn suffix(self) -> &'static str {
        match self {
            Heavy::Div => "div",
            Heavy::Fma => "fma",
            Heavy::Sqrt => "sqrt",
        }
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
