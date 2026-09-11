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

impl crate::stmt::Slots for Locals {
    fn slot(&self, id: takt_mir::VarId, _m: &mut Module) -> Option<(Reg, LlvmType)> {
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
    let mut ctx = crate::stmt::FnCtx { program: p, vars: locals, labels: 0, breaks: Vec::new() };
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

/// Die Variablen einer Blockmethode (5.7).
///
/// Die MIR nummeriert sie in einem Raum, und die Reihenfolge ist die des
/// Interpreters (`call_block_method`): **erst die Variablen der Instanz**
/// — Konstruktionsparameter, dann Zustandsvariablen —, **dann die
/// Parameter der Methode**. Wer sie vertauscht, liest den Zustand als
/// Argument und umgekehrt; beides uebersetzt, und nur die Zahlen sind
/// falsch.
pub struct BlockVars {
    /// Typen der Instanzvariablen (Parameter, dann Zustand).
    instance_fields: Vec<LlvmType>,
    /// Zeiger auf die Instanz.
    instance: Reg,
    /// Der Struct der Instanz.
    instance_ty: LlvmType,
    /// Die Parameter der Methode als Slots, hinter den Instanzvariablen.
    params: Vec<(Reg, LlvmType)>,
}

impl BlockVars {
    /// Woher eine Variable kommt.
    fn locate(&self, id: takt_mir::VarId) -> Option<Ort> {
        let n = self.instance_fields.len();
        if id.index() < n {
            return Some(Ort::Instanz(id.index() as u32, self.instance_fields[id.index()].clone()));
        }
        let (ptr, ty) = self.params.get(id.index() - n)?.clone();
        Some(Ort::Parameter(ptr, ty))
    }
}

/// Wo eine Variable einer Blockmethode liegt.
enum Ort {
    /// Feld der Instanz, mit seinem Index.
    Instanz(u32, LlvmType),
    /// Slot auf dem Stack.
    Parameter(Reg, LlvmType),
}

impl Vars for BlockVars {
    fn var(&self, id: takt_mir::VarId, m: &mut Module) -> Option<Lowered> {
        let (ptr, ty) = match self.locate(id)? {
            Ort::Parameter(ptr, ty) => (ptr, ty),
            Ort::Instanz(i, ty) => {
                let ptr = m.inst(&format!(
                    "getelementptr inbounds {}, ptr {}, i32 0, i32 {i}",
                    self.instance_ty, self.instance
                ));
                (ptr, ty)
            }
        };
        let v = m.inst(&format!("load {ty}, ptr {ptr}"));
        Some(Lowered { value: v.to_string(), ty })
    }
}

impl crate::stmt::Slots for BlockVars {
    fn slot(&self, id: takt_mir::VarId, m: &mut Module) -> Option<(Reg, LlvmType)> {
        match self.locate(id)? {
            Ort::Parameter(ptr, ty) => Some((ptr, ty)),
            Ort::Instanz(i, ty) => {
                let ptr = m.inst(&format!(
                    "getelementptr inbounds {}, ptr {}, i32 0, i32 {i}",
                    self.instance_ty, self.instance
                ));
                Some((ptr, ty))
            }
        }
    }
}

/// Schreibt eine Methode eines Blocks (5.7).
///
/// Die Signatur beginnt mit dem Zeiger auf die Instanz: Die Methode
/// aendert ihren Zustand, also bekommt sie ihn als Speicherort — dieselbe
/// Ueberlegung wie bei den Sammlungen (3.9).
pub fn block_method(b: &takt_mir::fns::BlockDef, f: &FnDef, p: &Program, m: &mut Module) -> Result<(), NotYet> {
    let mut params = vec![LlvmType::Ptr];
    for i in 0..f.params.len() {
        let local = f.locals.get(i).ok_or(NotYet { what: "Parameter" })?;
        params.push(ty::lower(local.ty, p).ok_or(NotYet { what: "Parametertyp" })?);
    }
    let ret = match f.ret {
        Some(t) => ty::lower(t, p).ok_or(NotYet { what: "Rueckgabetyp" })?,
        None => LlvmType::Void,
    };
    let mark = m.mark();
    let args = m.begin(&crate::block::method_symbol(b, &f.name), &ret, &params);
    match block_body(b, f, p, &args, &ret, m) {
        Ok(()) => Ok(()),
        Err(e) => {
            m.abort(mark);
            Err(e)
        }
    }
}

/// Der Rumpf einer Blockmethode.
fn block_body(
    b: &takt_mir::fns::BlockDef,
    f: &FnDef,
    p: &Program,
    args: &[Reg],
    ret: &LlvmType,
    m: &mut Module,
) -> Result<(), NotYet> {
    let Some(&instance) = args.first() else { return Err(NotYet { what: "Instanzzeiger" }) };
    let inst = crate::block::instance_of(b, p).ok_or(NotYet { what: "Blockinstanz" })?;
    // Die Parameter der Methode bekommen Slots; die Instanzvariablen
    // liegen im Struct und brauchen keine.
    let mut params = Vec::with_capacity(f.params.len());
    for (i, v) in f.locals.iter().enumerate() {
        let ty = ty::lower(v.ty, p).ok_or(NotYet { what: "Typ einer lokalen Variablen" })?;
        let ptr = m.inst(&format!("alloca {ty}"));
        if let Some(arg) = args.get(i + 1) {
            m.void_inst(&format!("store {ty} {arg}, ptr {ptr}"));
        }
        params.push((ptr, ty));
    }
    let vars = BlockVars {
        instance_fields: inst.fields[..inst.fields.len() - 1].to_vec(),
        instance,
        instance_ty: inst.llvm(),
        params,
    };
    let mut ctx = crate::stmt::FnCtx { program: p, vars, labels: 0, breaks: Vec::new() };
    crate::stmt::fn_block(&f.body, &mut ctx, m)?;
    if !m.terminated() {
        match ret {
            LlvmType::Void => m.void_inst("ret void"),
            _ => m.void_inst("unreachable"),
        }
    }
    m.end(None);
    Ok(())
}
