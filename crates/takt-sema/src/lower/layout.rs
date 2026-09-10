//! Drahtformat der Records (Referenz 3.7, Pruefung 46).
//!
//! Ein Record mit `layout little | big` hat eine Byte-Repraesentation:
//! `decode(b) -> T?` und `f.encode() -> bytes<SIZE>`. Der Plan entsteht
//! einmal beim Registrieren des Records — Offset und Breite je Feld,
//! Bitfelder im Traegerfeld, Konstantenfelder, Gesamtlaenge nach `align` —
//! und ist danach die einzige Quelle fuer Interpreter und Codegen
//! (plan/m2.md 1.8). Die Pruefung 46 faellt bei der Berechnung an: die
//! Ueberlappungspruefung *ist* die Planberechnung.

use takt_diag::Span;
use takt_mir::types::{FieldDef, Type};
use takt_mir::{RecordId, TypeId};

use super::Lowerer;
use crate::checks::SC46;

impl Lowerer<'_> {
    /// Rechnet den Byteplan eines Records mit `layout` und prueft ihn (46).
    /// Ohne `layout` gibt es keine Byte-Repraesentation und nichts zu tun.
    pub fn wire_layout(&mut self, id: RecordId) {
        let record = &self.program.records[id.index()];
        if record.layout.is_none() {
            return;
        }
        let (fields, align, span) = (record.fields.clone(), record.layout.as_ref().and_then(|l| l.align), record.span);
        let mut cursor = 0u32;
        let mut placed: Vec<(u32, u32, String)> = Vec::new();
        let mut out = fields.clone();
        for (i, f) in fields.iter().enumerate() {
            let Some(size) = self.wire_size(f.ty, f.span) else { continue };
            // `offset = N` setzt die Position absolut, sonst laeuft sie fort.
            let at = f.offset.unwrap_or(cursor);
            self.check_bits(f, span);
            if f.const_value.is_some() {
                self.check_const_fits(f, size, span);
            }
            for (start, len, name) in &placed {
                if at < start + len && *start < at + size {
                    self.error_hint(
                        SC46,
                        f.span,
                        format!("Feld `{}` ueberlappt `{name}` (Bytes {at}..{})", f.name, at + size),
                        "`offset = N` anpassen oder das Feld verschieben (3.7)",
                    );
                    break;
                }
            }
            placed.push((at, size, f.name.clone()));
            out[i].offset = Some(at);
            cursor = cursor.max(at + size);
        }
        // `align` rundet die Gesamtlaenge auf.
        if let Some(a) = align {
            if a == 0 || !a.is_power_of_two() {
                self.error_hint(SC46, span, format!("`align = {a}` ist keine Zweierpotenz"), "1, 2, 4, 8, … waehlen");
            } else {
                cursor = cursor.div_ceil(a) * a;
            }
        }
        let record = &mut self.program.records[id.index()];
        record.fields = out;
        record.wire_size = Some(cursor);
    }

    /// Bitfelder liegen innerhalb der Breite ihres Traegerfelds und
    /// ueberlappen einander nicht (3.7).
    fn check_bits(&mut self, f: &FieldDef, _span: Span) {
        let mut seen: Vec<(u8, u8, &str)> = Vec::new();
        for b in &f.bits {
            for (lo, hi, name) in &seen {
                if b.lo <= *hi && *lo <= b.hi {
                    self.error_hint(
                        SC46,
                        b.span,
                        format!("Bitfeld `{}` ueberlappt `{name}`", b.name),
                        "Bitpositionen ohne Ueberschneidung waehlen (3.7)",
                    );
                    break;
                }
            }
            seen.push((b.lo, b.hi, &b.name));
        }
    }

    /// Ein Konstantenfeld muss in die Breite seines Typs passen (3.7).
    fn check_const_fits(&mut self, f: &FieldDef, size: u32, span: Span) {
        let Some(takt_mir::types::Const::Int(v)) = f.const_value else { return };
        let bits = size * 8;
        if bits >= 64 {
            return;
        }
        let fits = match self.ty(f.ty) {
            Type::Int { width, .. } if width.signed() => {
                let half = 1i64 << (bits - 1);
                (-half..half).contains(&v)
            }
            _ => (0..(1i64 << bits)).contains(&v),
        };
        if !fits {
            self.error(SC46, span, format!("Konstantenfeld `{}` passt nicht in {size} Byte", f.name));
        }
    }

    /// Groesse eines Typs im Drahtformat, in Bytes (3.7). `None` und eine
    /// Diagnose, wenn der Typ keine Byte-Repraesentation hat.
    pub fn wire_size(&mut self, ty: TypeId, span: Span) -> Option<u32> {
        match self.ty(ty).clone() {
            Type::Bool => Some(1),
            Type::Int { width, .. } => Some(width.bits() / 8),
            Type::Float { width, .. } => Some(match width {
                takt_mir::types::FloatWidth::F32 => 4,
                takt_mir::types::FloatWidth::F64 => 8,
            }),
            Type::Bytes { cap } => Some(cap),
            Type::Array { elem, len } => {
                let inner = self.wire_size(elem, span)?;
                Some(inner * len)
            }
            Type::Enum(e) => {
                // `enum … layout u8` gibt die Breite vor; sonst ein Byte.
                Some(self.program.enums[e.index()].layout.map_or(1, |w| w.bits() / 8))
            }
            Type::Record(r) => {
                let size = self.program.records[r.index()].wire_size;
                match size {
                    Some(n) => Some(n),
                    None => {
                        let name = self.program.records[r.index()].name.clone();
                        self.error_hint(
                            SC46,
                            span,
                            format!("verschachtelter Record `{name}` hat kein `layout`"),
                            "`layout little` oder `layout big` am Record ergaenzen (3.7)",
                        );
                        None
                    }
                }
            }
            other => {
                let _ = other;
                let n = self.type_name(ty);
                self.error_hint(
                    SC46,
                    span,
                    format!("`{n}` hat keine Byte-Repraesentation"),
                    "Felder eines `layout`-Records sind Zahlen, Bools, Enums, Bytes, Arrays und Records (3.7)",
                );
                None
            }
        }
    }
}
