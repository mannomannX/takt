//! Sema: Symbole, Einheiten, Typen, Lowering in die MIR und die Pruefungen
//! aus Referenz 10 (Entwurf `plan/m1.md`).
//!
//! `check` ist die Syntaxstufe (Edition, Tokenizer, Parser, 49 bis 51);
//! `compile` fuegt das Lowering hinzu und liefert das Programm, wenn keine
//! Fehler auftraten. Jede Diagnose traegt den Code ihrer Pruefung (`SC-n`),
//! damit Inventur, Tests und Meldungen dieselbe Nummer haben.

pub mod checks;
pub mod collect;
pub mod edition;
pub mod enums;
pub mod lower;
pub mod names;
pub mod symbols;
pub mod units;
pub mod visit;

use takt_diag::{Diagnostic, Policy, Sink};
use takt_mir::analysis::Report;
use takt_mir::program::Program;
use takt_syntax::Edition;
use takt_syntax::ast::File;
use takt_syntax::{parse_file, tokenize_in};

use crate::lower::Lowerer;

/// Das Prelude: vordefinierte Einheiten, Enums, Records und Bibliothek in
/// Takt selbst (plan/m1.md 1.9).
pub const PRELUDE: &str = include_str!("prelude.takt");

/// Art des Builds (8.3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Build {
    /// Simulation: `sim`-Outputs speisen `hw`-Inputs.
    #[default]
    Sim,
    /// Hardware.
    Hw,
}

/// Optionen des Uebersetzens.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Politik der Diagnosen.
    pub policy: Policy,
    /// Sim- oder HW-Build.
    pub build: Build,
    /// Gewaehltes Profil (8.4).
    pub profile: Option<String>,
}

impl Options {
    /// Optionen mit einer Politik.
    pub fn new(policy: Policy) -> Self {
        Options { policy, ..Default::default() }
    }
}

/// Ergebnis von `check`.
#[derive(Debug)]
pub struct Checked {
    /// Der Baum, wenn Tokenizer und Parser ohne Fehler waren.
    pub file: Option<File>,
    /// Die verwendete Edition.
    pub edition: Edition,
    /// Alle Diagnosen, nach Position sortiert und mit angewandter Politik.
    pub diagnostics: Vec<Diagnostic>,
}

impl Checked {
    /// Gibt es Fehler?
    pub fn has_errors(&self) -> bool {
        self.diagnostics.iter().any(Diagnostic::is_error)
    }
}

/// Ergebnis von `compile`.
#[derive(Debug)]
pub struct Compiled {
    /// Das Programm in Kernform, wenn keine Fehler auftraten.
    pub program: Option<Program>,
    /// Die verwendete Edition.
    pub edition: Edition,
    /// Alle Diagnosen.
    pub diagnostics: Vec<Diagnostic>,
    /// Kennzahlen des statischen Gates (3.4; M3). Ohne Programm leer.
    pub report: Report,
}

impl Compiled {
    /// Gibt es Fehler?
    pub fn has_errors(&self) -> bool {
        self.diagnostics.iter().any(Diagnostic::is_error)
    }
}

/// Prueft eine Datei: Edition, Tokenizer, Parser, dann die semantischen
/// Pruefungen (nur bei fehlerfreier Syntax, damit ein kaputter Baum keine
/// Folgemeldungen erzeugt).
pub fn check(src: &str, policy: Policy) -> Checked {
    let mut sink = Sink::new(policy);
    let (edition, file) = match syntax(src, &mut sink) {
        Some(pair) => pair,
        None => return Checked { file: None, edition: Edition::LATEST, diagnostics: sink.sorted() },
    };
    sink.extend(names::check(&file, edition));
    sink.extend(enums::check(&file, edition));
    Checked { file: Some(file), edition, diagnostics: sink.sorted() }
}

/// Uebersetzt eine Datei in die MIR (Kernform nach dem Desugaring 6.2).
pub fn compile(src: &str, options: &Options) -> Compiled {
    let mut sink = Sink::new(options.policy);
    let (edition, file) = match syntax(src, &mut sink) {
        Some(pair) => pair,
        None => {
            return Compiled {
                program: None,
                edition: Edition::LATEST,
                diagnostics: sink.sorted(),
                report: Report::default(),
            };
        }
    };
    sink.extend(names::check(&file, edition));
    let (program, diags) = lower::run(&file, edition, options);
    sink.extend(diags);
    let mut program = program.filter(|_| !sink.has_errors());
    // Das statische Gate laeuft nach dem Lowering auf der fertigen MIR
    // (3.4, 9.4.3, 11.5; plan/m3.md 1.1): Es braucht Kontrollfluss, und
    // Codegen und `takt size` benutzen dieselben Intervalle.
    let report = match &mut program {
        Some(p) => {
            let (d, r) = takt_mir::analysis::analyze(p);
            sink.extend(d);
            takt_mir::analysis::cost::budgets(p);
            r
        }
        None => Report::default(),
    };
    let program = program.filter(|_| !sink.has_errors());
    Compiled { program, edition, diagnostics: sink.sorted(), report }
}

/// Edition, Tokenizer, Parser; `None`, wenn die Syntax Fehler hat.
fn syntax(src: &str, sink: &mut Sink) -> Option<(Edition, File)> {
    let (edition, edition_diags) = edition::resolve(src);
    sink.extend(edition_diags);
    let toks = tokenize_in(src, edition);
    let lex_ok = toks.errors.is_empty();
    sink.extend(toks.errors.iter().cloned().map(relabel_reserved));
    let (file, parse_errors) = parse_file(&toks);
    let ok = lex_ok && parse_errors.is_empty();
    sink.extend(parse_errors);
    ok.then_some((edition, file))
}

/// Reservierte Woerter als Bezeichner meldet der Tokenizer; Referenz 10 fuehrt
/// sie unter Pruefung 50.
fn relabel_reserved(mut d: Diagnostic) -> Diagnostic {
    if d.code == "E_RESERVED" {
        d.code = names::CODE;
    }
    d
}

/// Uebersetzt das Prelude allein; nur fuer Tests und Werkzeuge.
pub fn compile_prelude() -> Compiled {
    compile("", &Options::default())
}

/// Zugriff auf den Elaborator fuer Werkzeuge (Dump, Tests).
pub fn lower_file(file: &File, edition: Edition, options: &Options) -> (Option<Program>, Vec<Diagnostic>) {
    let _ = Lowerer::new(takt_mir::program::Config::new(edition.number(), 1_000_000), edition, options);
    lower::run(file, edition, options)
}
