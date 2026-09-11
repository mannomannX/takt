//! Reine Funktionen (4.4) als LLVM-Funktionen.
//!
//! Eine `fn` in Takt ist rein: Sie liest ihre Parameter und liefert einen
//! Wert, ohne Zustand und ohne Seiteneffekt (4.4). Das macht die Senkung
//! einfach — sie wird eine gewoehnliche LLVM-Funktion, und LLVM darf sie
//! einbetten, gemeinsame Teilausdruecke zusammenfassen und ueber Aufrufe
//! hinweg optimieren.
//!
//! **Lokale Variablen liegen auf dem Stack, nicht im Zustands-Struct.**
//! Eine Funktion hat keinen Zustand ueber ihren Aufruf hinaus (4.4); ihre
//! Locals sind `alloca`, und `mem2reg` hebt heraus, was nicht im Speicher
//! bleiben muss. Das ist der Unterschied zur Maschine, deren Variablen
//! einen Tick ueberdauern muessen.
//!
//! 12.3 verlangt einen statisch bekannten Stack. Das ist erfuellt: Keine
//! Rekursion (13.4, Power of Ten), keine dynamische Allokation, und jede
//! `alloca` steht im Eintrittsblock — ihre Zahl ist die der Locals.

use takt_mir::fns::Fn as FnDef;
use takt_mir::program::Program;

use crate::emit::{Module, Reg};
use crate::expr::{Lowered, NotYet, Vars};
use crate::ty::{self, LlvmType};

/// Der Name einer Funktion im erzeugten Code.
///
/// Das Praefix trennt sie von den Schrittfunktionen der Maschinen und von
/// allem, was der Linker sonst sieht.
pub fn symbol(f: &FnDef) -> String {
    format!("takt_fn_{}", f.name)
}

/// Die Locals einer Funktion: `alloca` im Eintrittsblock.
///
/// Die Parameter stehen zuerst (die MIR legt sie so ab) und werden aus
/// ihren Registern in ihre Slots geschrieben; danach sind Parameter und
/// Locals ununterscheidbar, was den Rumpf einfacher macht.
pub struct Locals {
    /// Zeiger je lokaler Variable, in der Reihenfolge der MIR.
    slots: Vec<(Reg, LlvmType)>,
}

impl Vars for Locals {
    fn var(&self, id: takt_mir::VarId, m: &mut Module) -> Option<Lowered> {
        let (ptr, ty) = self.slots.get(id.index())?.clone();
        let v = m.inst(&format!("load {ty}, ptr {ptr}"));
        Some(Lowered { value: v.to_string(), ty })
    }
}

impl Locals {
    /// Der Speicherort einer lokalen Variablen.
    pub fn slot(&self, id: takt_mir::VarId) -> Option<(Reg, LlvmType)> {
        self.slots.get(id.index()).cloned()
    }
}

/// Legt die Slots einer Funktion an und fuellt die Parameter.
pub fn prologue(f: &FnDef, p: &Program, args: &[Reg], m: &mut Module) -> Result<Locals, NotYet> {
    let mut slots = Vec::with_capacity(f.locals.len());
    for (i, v) in f.locals.iter().enumerate() {
        let ty = ty::lower(v.ty, p).ok_or(NotYet { what: "Typ einer lokalen Variablen" })?;
        let ptr = m.inst(&format!("alloca {ty}"));
        if let Some(arg) = args.get(i) {
            m.void_inst(&format!("store {ty} {arg}, ptr {ptr}"));
        }
        slots.push((ptr, ty));
    }
    Ok(Locals { slots })
}

/// Die Signatur einer Funktion: Parametertypen und Rueckgabetyp.
pub fn signature(f: &FnDef, p: &Program) -> Option<(Vec<LlvmType>, LlvmType)> {
    let mut params = Vec::with_capacity(f.params.len());
    for i in 0..f.params.len() {
        params.push(ty::lower(f.locals.get(i)?.ty, p)?);
    }
    let ret = match f.ret {
        Some(t) => ty::lower(t, p)?,
        None => LlvmType::Void,
    };
    Some((params, ret))
}

/// Schreibt eine Funktion vollstaendig (4.4).
///
/// `Err` verwirft die angefangene Funktion: Eine halbe waere gueltige IR
/// mit falschem Inhalt, und der Aufrufer saehe ihr das nicht an.
pub fn function(f: &FnDef, p: &Program, m: &mut Module) -> Result<(), NotYet> {
    let (params, ret) = signature(f, p).ok_or(NotYet { what: "Signatur" })?;
    let mark = m.mark();
    let args = m.begin(&symbol(f), &ret, &params);
    match body(f, p, &args, &ret, m) {
        Ok(()) => Ok(()),
        Err(e) => {
            m.abort(mark);
            // Die Funktion wird deklariert statt weggelassen: Ein Aufruf
            // auf ein undefiniertes Symbol ist gueltige IR, und der
            // Linker nennt es beim Namen. Ein fehlendes Symbol laesst
            // clang die ganze Datei zurueckweisen — der Fehler traefe
            // dann alle Funktionen statt der einen.
            let ps: Vec<String> = params.iter().map(|t| t.to_string()).collect();
            m.declare(&format!("declare {ret} @{}({})", symbol(f), ps.join(", ")));
            Err(e)
        }
    }
}

/// Der Rumpf; `function` raeumt bei `Err` auf.
fn body(f: &FnDef, p: &Program, args: &[Reg], ret: &LlvmType, m: &mut Module) -> Result<(), NotYet> {
    let locals = prologue(f, p, args, m)?;
    let mut ctx = crate::stmt::FnCtx { program: p, locals, labels: 0, breaks: Vec::new() };
    crate::stmt::fn_block(&f.body, &mut ctx, m)?;
    // Ein Rumpf ohne `return` am Ende kann nicht vorkommen (Pruefung 11),
    // aber LLVM verlangt einen Terminator. Der Wert ist unerreichbar.
    if !m.terminated() {
        match ret {
            LlvmType::Void => m.void_inst("ret void"),
            _ => m.void_inst("unreachable"),
        }
    }
    m.end(None);
    Ok(())
}
