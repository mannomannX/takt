//! Methoden der Sammlungen (3.9): `push`, `append`, `clear`.
//!
//! Eine Sammlung ist ein Struct `{ len: i32, data: [N x T] }` (siehe
//! `ty`). Die Kapazitaet steht im Typ, die Laenge im Wert — daraus folgt
//! alles Weitere.
//!
//! **Beschraenkt und alles-oder-nichts.** 3.9 verlangt beides: `push`
//! schreibt nur, wenn Platz ist, und `append` haengt entweder die ganze
//! Folge an oder gar nichts. Ein Teilanhang liesse einen halben Rahmen im
//! Puffer zurueck, den niemand als Fehler erkennt. Der Rueckgabewert sagt,
//! was geschah — er ist kein Fault, weil ein voller Puffer ein
//! vorhersehbarer Betriebszustand ist und das Programm darauf reagieren
//! koennen soll.
//!
//! **Keine Schleife bei `append`.** Der Anhang ist ein `memcpy` mit
//! berechneter Laenge; LLVM kennt es als Intrinsic, und eine Schleife
//! haette eine Schranke gebraucht, die 4.1 ohnehin verlangt.

use takt_mir::stmt::{Method, Place};

use crate::emit::{Module, Reg};
use crate::expr::{Lowered, NotYet};
use crate::ty::LlvmType;

/// Die Felder einer Sammlung (3.9).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum Slot {
    Len = 0,
    Data = 1,
}

/// Kapazitaet und Elementtyp einer Sammlung.
pub struct Layout {
    /// Hoechstzahl der Elemente; steht im Typ (3.9).
    pub cap: u32,
    /// Typ eines Elements.
    pub elem: LlvmType,
    /// Der Struct selbst.
    pub ty: LlvmType,
}

/// Liest den Aufbau einer Sammlung aus ihrem LLVM-Typ.
pub fn layout_of(ty: &LlvmType) -> Option<Layout> {
    let LlvmType::Struct(fields) = ty else { return None };
    let [LlvmType::Int(32), LlvmType::Array(elem, cap)] = fields.as_slice() else { return None };
    Some(Layout { cap: *cap, elem: (**elem).clone(), ty: ty.clone() })
}

/// `c.push(x)` (3.9): haengt ein Element an, wenn Platz ist.
///
/// Liefert `true`, wenn geschrieben wurde. Ohne Verzweigung geht es nicht
/// — geschrieben wird nur im einen Fall —, aber der Vergleich steht vor
/// dem Sprung, damit der haeufige Weg (Platz vorhanden) der gerade ist.
pub fn push(recv: Reg, l: &Layout, value: &Lowered, label: u32, m: &mut Module) -> Reg {
    let ty = &l.ty;
    let before = m.block().to_string();
    let len_ptr = m.inst(&format!("getelementptr inbounds {ty}, ptr {recv}, i32 0, i32 0"));
    let len = m.inst(&format!("load i32, ptr {len_ptr}"));
    let fits = m.inst(&format!("icmp ult i32 {len}, {}", l.cap));
    let (write, done) = (format!("push{label}"), format!("push{label}_ende"));
    m.void_inst(&format!("br i1 {fits}, label %{write}, label %{done}"));
    m.label(&write);
    let data = m.inst(&format!("getelementptr inbounds {ty}, ptr {recv}, i32 0, i32 1"));
    let at = m.inst(&format!("getelementptr inbounds [{} x {}], ptr {data}, i32 0, i32 {len}", l.cap, l.elem));
    m.write(&value.ty, &value.value, &at.to_string());
    let next = m.inst(&format!("add i32 {len}, 1"));
    m.void_inst(&format!("store i32 {next}, ptr {len_ptr}"));
    m.void_inst(&format!("br label %{done}"));
    m.label(&done);
    // `phi` statt eines Zwischenspeichers: Der Wert steht in den beiden
    // Vorgaengerbloecken fest, und LLVM erwartet ihn in dieser Form.
    m.inst(&format!("phi i1 [ true, %{write} ], [ false, %{before} ]"))
}

/// `c.append(src)` (3.9): haengt eine ganze Folge an, oder nichts.
pub fn append(recv: Reg, src: Reg, l: &Layout, label: u32, m: &mut Module) -> Reg {
    let ty = &l.ty;
    let before = m.block().to_string();
    let len_ptr = m.inst(&format!("getelementptr inbounds {ty}, ptr {recv}, i32 0, i32 0"));
    let len = m.inst(&format!("load i32, ptr {len_ptr}"));
    let src_len_ptr = m.inst(&format!("getelementptr inbounds {ty}, ptr {src}, i32 0, i32 0"));
    let src_len = m.inst(&format!("load i32, ptr {src_len_ptr}"));
    let sum = m.inst(&format!("add i32 {len}, {src_len}"));
    let fits = m.inst(&format!("icmp ule i32 {sum}, {}", l.cap));
    let (write, done) = (format!("append{label}"), format!("append{label}_ende"));
    m.void_inst(&format!("br i1 {fits}, label %{write}, label %{done}"));
    m.label(&write);
    let data = m.inst(&format!("getelementptr inbounds {ty}, ptr {recv}, i32 0, i32 1"));
    let at = m.inst(&format!("getelementptr inbounds [{} x {}], ptr {data}, i32 0, i32 {len}", l.cap, l.elem));
    let src_data = m.inst(&format!("getelementptr inbounds {ty}, ptr {src}, i32 0, i32 1"));
    let bytes = m.inst(&format!("mul i32 {src_len}, {}", l.elem.size().max(1)));
    let bytes64 = m.inst(&format!("zext i32 {bytes} to i64"));
    m.void_inst(&format!("call void @llvm.memcpy.p0.p0.i64(ptr {at}, ptr {src_data}, i64 {bytes64}, i1 false)"));
    m.void_inst(&format!("store i32 {sum}, ptr {len_ptr}"));
    m.void_inst(&format!("br label %{done}"));
    m.label(&done);
    m.inst(&format!("phi i1 [ true, %{write} ], [ false, %{before} ]"))
}

/// `c.clear()` (3.9): setzt die Laenge auf null.
///
/// Der Speicher wird nicht ueberschrieben — die Elemente jenseits der
/// Laenge sind nicht lesbar (3.9), und sie zu loeschen kostete O(N) ohne
/// Gewinn.
pub fn clear(recv: Reg, l: &Layout, m: &mut Module) -> Reg {
    let len_ptr = m.inst(&format!("getelementptr inbounds {}, ptr {recv}, i32 0, i32 0", l.ty));
    m.void_inst(&format!("store i32 0, ptr {len_ptr}"));
    // `clear` gelingt immer; der Rueckgabewert ist der Gleichfoermigkeit
    // halber da (3.9: alle Sammlungsmethoden liefern `bool`).
    m.inst("add i1 true, false")
}

/// Braucht die Methode einen Empfaenger, der eine Sammlung ist?
pub fn is_collection_method(method: Method) -> bool {
    matches!(method, Method::Push | Method::Append | Method::Clear)
}

/// Der Empfaenger als Speicherort; Sammlungsmethoden aendern ihn.
pub fn receiver_is_place(p: &Place) -> bool {
    matches!(p, Place::Var(_) | Place::Output(_) | Place::Field(..) | Place::Index(..))
}

/// Meldet, was an einer Methode noch fehlt.
pub fn unsupported(method: Method) -> NotYet {
    NotYet {
        what: match method {
            Method::Step => "`step()` einer Blockinstanz",
            Method::Reset => "`reset()` einer Blockinstanz",
            Method::Block(_) => "Blockmethode",
            Method::Insert => "`insert`",
            Method::Remove => "`remove`",
            _ => "Methode",
        },
    }
}
