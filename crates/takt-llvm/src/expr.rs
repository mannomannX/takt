//! Ausdruecke nach LLVM-IR.
//!
//! **Die Totalitaet aus 4.1 ist hier sichtbar.** Ein `Checked`-Knoten der
//! MIR wird zu einem Vergleich und einem Sprung in den Fault-Trampolin;
//! wo M3 bewiesen hat, dass die Pruefung entfallen kann, steht in der MIR
//! kein `Checked`, und der Codegen erzeugt keinen Zweig (plan/m4.md 4.2).
//! Der Codegen entscheidet das nicht selbst — er erzeugt, was dasteht.
//!
//! **Fliesskomma ohne Flags.** Jede `f*`-Zeile entsteht hier, und keine
//! traegt ein Flag; `emit` bietet dafuer keine Moeglichkeit (4.2).

use takt_mir::TypeId;
use takt_mir::expr::{Accessor, BinaryOp, ConvertKind, Expr, ExprKind, Intrinsic, UnaryOp};
use takt_mir::program::Program;
use takt_mir::types::{IntWidth, Type};

use crate::emit::{Module, float_literal};
use crate::ty::{self, LlvmType};

/// Ein gesenkter Ausdruck: sein Wert als Operand und sein Typ.
///
/// Der Operand ist Text, weil ein Literal in LLVM kein Register belegt —
/// `add i32 %1, 7` ist eine Zeile, nicht zwei.
#[derive(Clone, Debug, PartialEq)]
pub struct Lowered {
    /// Der Operand: `%3`, `7`, `0x3FF0000000000000`.
    pub value: String,
    /// Sein LLVM-Typ.
    pub ty: LlvmType,
}

/// Was der Codegen noch nicht kann.
///
/// Eine ehrliche Meldung statt falschem Code: Schritt 6 deckt Literale,
/// Variablen und die Arithmetik ab. Alles andere nennt sich beim Namen,
/// damit ein Fehlschlag zeigt, was fehlt — und nicht, dass etwas falsch
/// gerechnet wurde.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotYet {
    /// Welcher Knoten.
    pub what: &'static str,
}

/// Wo die Werte der Variablen stehen.
///
/// 11.2 legt sie in den Zustands-Struct der Maschine. Eine Variable zu
/// lesen ist darum ein `getelementptr` und ein `load` — Anweisungen, die
/// in den Modulpuffer gehen. Das Trait bekommt ihn deshalb: Ein Trait,
/// das so taete, als koste ein Variablenzugriff nichts, beschriebe etwas
/// anderes als den erzeugten Code.
pub trait Vars {
    /// Laedt eine Variable und liefert ihren Operanden.
    ///
    /// `None` heisst: Die Variable ist hier nicht erreichbar.
    fn var(&self, id: takt_mir::VarId, m: &mut Module) -> Option<Lowered>;

    /// Laedt den Wert eines Input-Channels aus dem Prozessabbild.
    ///
    /// 11.2 gibt der Schrittfunktion dafuer den Zeiger `i`. Die Qualitaet
    /// steht daneben (3.5); `dominated` sagt, ob das Programm sie schon
    /// geprueft hat — ist es nicht dominiert, steht in der MIR ein
    /// `Checked`-Knoten darum herum (4.1), und der erzeugt den Zweig.
    fn input(&self, _channel: takt_mir::ChannelId, _m: &mut Module) -> Option<Lowered> {
        None
    }

    /// Laedt einen Parameter (8.4).
    ///
    /// Parameter sind zur Laufzeit konstant, `tunable` ausgenommen; beide
    /// stehen im Parametervektor des Laufs, nicht im Zustand der Maschine.
    fn param(&self, _id: takt_mir::ParamId, _m: &mut Module) -> Option<Lowered> {
        None
    }

    /// Laedt den Latch eines eigenen Outputs (`latch(o)`, 9.2).
    fn output(&self, _channel: takt_mir::ChannelId, _m: &mut Module) -> Option<Lowered> {
        None
    }

    /// Liest ein Feld des Abbild-Eintrags eines Channels (3.5).
    ///
    /// `.valid`, `.suspect`, `.stale`, `.age` und `.reason` lesen die
    /// Qualitaet neben dem Wert; `crate::image` beschreibt den Aufbau.
    fn quality(&self, _channel: takt_mir::ChannelId, _slot: crate::image::Slot, _m: &mut Module) -> Option<Lowered> {
        None
    }

    /// Wohin ein gescheiterter Laufzeit-Check springt (4.1, 5.3).
    ///
    /// Nur eine Maschine hat einen Fault-Pfad; eine reine Funktion (4.4)
    /// faultet den Aufrufer, und ihr Zweig entsteht erst, wenn die MIR
    /// den `Checked`-Knoten an die Aufrufstelle traegt. Bis dahin meldet
    /// sie `None`.
    fn fault_label(&self) -> Option<String> {
        None
    }

    /// Liest ein Command (8.5).
    ///
    /// Ein Command ist ein Puls, der genau einen Tick gilt; die Runtime
    /// setzt ihn vor dem Schritt und loescht ihn danach (12.1). Im
    /// erzeugten Code ist er ein `bool` im Prozessabbild.
    fn command(&self, _id: takt_mir::CommandId, _m: &mut Module) -> Option<Lowered> {
        None
    }
}

/// Senkt einen Ausdruck und liefert seinen Operanden.
pub fn lower(e: &Expr, p: &Program, m: &mut Module, vars: &dyn Vars) -> Result<Lowered, NotYet> {
    // `interp` steht vor der Typbestimmung: Sein erstes Argument ist eine
    // Tabelle, und `table<A, B>` hat keine Darstellung als Wert — die
    // Stuetzstellen gehen unmittelbar in die Rechnung (3.9). Den Typ zu
    // verlangen hiesse, eine Darstellung zu fordern, die niemand braucht.
    if let ExprKind::Intrinsic { op: Intrinsic::Interp, args } = &e.kind {
        let want = ty::lower(e.ty, p).ok_or(NotYet { what: "Typ" })?;
        return interp(args, &want, p, m, vars);
    }
    let want = ty::lower(e.ty, p).ok_or(NotYet { what: "Typ" })?;
    match &e.kind {
        ExprKind::Bool(b) => Ok(Lowered { value: i32::from(*b).to_string(), ty: want }),
        ExprKind::Int(n) => Ok(Lowered { value: n.to_string(), ty: want }),
        ExprKind::Duration(d) => Ok(Lowered { value: d.to_string(), ty: want }),
        ExprKind::Float(f) => Ok(Lowered { value: float_literal(*f, &want), ty: want }),
        ExprKind::Var(id) => vars.var(*id, m).ok_or(NotYet { what: "unbekannte Variable" }),
        ExprKind::Default => default_of(&want),
        ExprKind::None => default_of(&want),
        // `ok(v)` und `err(e)` bauen ein `T!E` (3.8); `lift` hebt einen
        // Wert in ein `T?`.
        ExprKind::Ok(v) | ExprKind::Lift(v) => wrap(v, true, &want, p, m, vars),
        ExprKind::Err(e) => wrap(e, false, &want, p, m, vars),
        ExprKind::Input { channel, .. } => vars.input(*channel, m).ok_or(NotYet { what: "Input" }),
        ExprKind::Param(id) => vars.param(*id, m).ok_or(NotYet { what: "Parameter" }),
        ExprKind::Output(channel) => vars.output(*channel, m).ok_or(NotYet { what: "Output-Latch" }),
        ExprKind::Command(id) => vars.command(*id, m).ok_or(NotYet { what: "Command" }),
        ExprKind::Unary { op, expr } => unary(*op, expr, &want, p, m, vars),
        ExprKind::Binary { op, lhs, rhs } => binary(*op, lhs, rhs, &want, p, m, vars),
        ExprKind::Cond { cond, then, otherwise } => cond_expr(cond, then, otherwise, &want, p, m, vars),
        ExprKind::Variant { enum_id, variant, fields } => self_variant(*enum_id, *variant, fields, &want, p),
        ExprKind::Record { fields, .. } => record(fields, &want, p, m, vars),
        // Ein Array-Literal wird wie ein Record gebaut: `insertvalue` je
        // Element aus `undef` heraus (3.9).
        ExprKind::Array(items) => record(items, &want, p, m, vars),
        // Eine Stuetzstelle ist ein Paar (3.9); sie steht nur in einer
        // Tabelle, und `interp` liest sie dort unmittelbar.
        ExprKind::Call { callee, args } => call(*callee, args, &want, p, m, vars),
        ExprKind::Intrinsic { op, args } => intrinsic(*op, args, &want, p, m, vars),
        ExprKind::Decode { record, bytes } => decode(*record, bytes, &want, p, m, vars),
        ExprKind::Slice { base, from, to } => slice(base, from, to, &want, p, m, vars),
        ExprKind::Index { base, index } => index_of(base, index, &want, p, m, vars),
        ExprKind::Field { base, field } => field_of(base, *field, &want, p, m, vars),
        ExprKind::Cast { expr, to } => cast(expr, *to, &want, p, m, vars),
        ExprKind::Convert { expr, kind, unit } => convert(expr, *kind, *unit, &want, p, m, vars),
        ExprKind::Accessor { base, accessor: which, args } => access(base, *which, args, &want, p, m, vars),
        ExprKind::Checked { expr, kind } => {
            // 3.5: Ein Lesen auf einem ungueltigen Channel ist ein
            // `SensorFault`. Die Pruefung steht *vor* dem Lesen — der Wert
            // im Abbild ist bei `Bad` bedeutungslos.
            if *kind == takt_mir::expr::CheckedKind::Valid
                && let ExprKind::Input { channel, .. } = &expr.kind
            {
                valid_or_fault(*channel, m, vars)?;
            }
            let inner = lower(expr, p, m, vars)?;
            // `Missing` ist das Auspacken eines `T?`/`T!E` (3.8): Der
            // Knoten prueft, dass ein Wert da ist, *und* liefert ihn. Der
            // Zweig in den Fault-Trampolin entsteht in der Maschine; hier
            // steht das Auspacken, ohne das jeder folgende Zugriff auf den
            // Wrapper statt auf den Inhalt ginge.
            if *kind == takt_mir::expr::CheckedKind::Missing
                && let LlvmType::Struct(_) = &inner.ty
                && inner.ty != want
            {
                let v = m.inst(&format!("extractvalue {} {}, 0", inner.ty, inner.value));
                return Ok(Lowered { value: v.to_string(), ty: want });
            }
            Ok(inner)
        }
        other => Err(NotYet { what: node_name(other) }),
    }
}

/// `a if c else b` (3.8): in LLVM ein `select`.
///
/// `select` statt Verzweigung, weil beide Seiten total sind (4.1) — es
/// gibt keinen Seiteneffekt, den ein Sprung sparen koennte, und `select`
/// bleibt ein Basisblock. Wo eine Seite einen Fault ausloesen kann, steht
/// in der MIR ein `Checked`-Knoten *darin*, und der erzeugt seinen Zweig
/// selbst.
fn cond_expr(
    cond: &Expr,
    then: &Expr,
    otherwise: &Expr,
    want: &LlvmType,
    p: &Program,
    m: &mut Module,
    vars: &dyn Vars,
) -> Result<Lowered, NotYet> {
    let c = lower(cond, p, m, vars)?;
    let a = lower(then, p, m, vars)?;
    let b = lower(otherwise, p, m, vars)?;
    let r = m.inst(&format!("select i1 {}, {} {}, {} {}", c.value, a.ty, a.value, b.ty, b.value));
    Ok(Lowered { value: r.to_string(), ty: want.clone() })
}

/// Zugriffe, die reine Rechnung sind (3.10).
///
/// `.bit`, `.bits` und `.with_bit` stehen hier, weil sie sich in
/// Schiebeoperationen uebersetzen. Die Qualitaetszugriffe (`.valid`,
/// `.age`, …) brauchen dagegen das Prozessabbild und dessen Aufbau; sie
/// gehoeren zu den Inputs und kommen mit ihnen.
fn access(
    base: &Expr,
    which: Accessor,
    args: &[Expr],
    want: &LlvmType,
    p: &Program,
    m: &mut Module,
    vars: &dyn Vars,
) -> Result<Lowered, NotYet> {
    let x = lower(base, p, m, vars)?;
    let arg = |i: usize, m: &mut Module| -> Result<Lowered, NotYet> {
        let e = args.get(i).ok_or(NotYet { what: "Argument fehlt" })?;
        lower(e, p, m, vars)
    };
    match which {
        // `x.bit(i)` = (x >> i) & 1, als `bool`.
        Accessor::Bit => {
            let i = arg(0, m)?;
            let sh = m.inst(&format!("lshr {} {}, {}", x.ty, x.value, i.value));
            let one = m.inst(&format!("and {} {sh}, 1", x.ty));
            let b = m.inst(&format!("icmp ne {} {one}, 0", x.ty));
            Ok(Lowered { value: b.to_string(), ty: LlvmType::Int(1) })
        }
        // `x.bits(hi, lo)` = (x >> lo) & ((1 << (hi - lo + 1)) - 1), also
        // das Feld von `lo` bis `hi` einschliesslich (3.10). Die Grenzen
        // sind Konstanten (die Pruefung im Sema verlangt es), darum
        // entsteht die Maske beim Uebersetzen und nicht zur Laufzeit.
        Accessor::Bits => {
            let (hi, lo) = (arg(0, m)?, arg(1, m)?);
            let (Ok(hi), Ok(lo)) = (hi.value.parse::<u32>(), lo.value.parse::<u32>()) else {
                return Err(NotYet { what: "`.bits` mit berechneten Grenzen" });
            };
            if hi < lo {
                return Err(NotYet { what: "`.bits` mit vertauschten Grenzen" });
            }
            let width = hi - lo + 1;
            let mask: u64 = if width >= 64 { u64::MAX } else { (1u64 << width) - 1 };
            let sh = m.inst(&format!("lshr {} {}, {lo}", x.ty, x.value));
            let r = m.inst(&format!("and {} {sh}, {mask}", x.ty));
            // Das Ergebnis hat die Breite des Traegers, nicht die des
            // Zieltyps: Ein `.bits(7, 4)` auf einem `u16` liefert einen
            // `i16`, den erst ein `as` verengt. Gaebe der Knoten hier den
            // Zieltyp an, waere die naechste Instruktion falsch getypt.
            Ok(Lowered { value: r.to_string(), ty: x.ty.clone() })
        }
        // `x.with_bit(i, b)`: setzen oder loeschen, ohne Verzweigung.
        Accessor::WithBit => {
            let (i, b) = (arg(0, m)?, arg(1, m)?);
            let mask = m.inst(&format!("shl {} 1, {}", x.ty, i.value));
            let set = m.inst(&format!("or {} {}, {mask}", x.ty, x.value));
            let inv = m.inst(&format!("xor {} {mask}, -1", x.ty));
            let clear = m.inst(&format!("and {} {}, {inv}", x.ty, x.value));
            let r = m.inst(&format!("select i1 {}, {} {set}, {} {clear}", b.value, x.ty, x.ty));
            Ok(Lowered { value: r.to_string(), ty: want.clone() })
        }
        // Die Qualitaetszugriffe lesen den Eintrag des Channels neben
        // seinem Wert (3.5, `crate::image`). Sie brauchen den Channel
        // selbst, nicht seinen Wert — `x.valid` fragt nicht, *was*
        // geliefert wurde, sondern *ob*.
        Accessor::Valid | Accessor::Suspect | Accessor::Stale | Accessor::Age | Accessor::Reason => {
            if let ExprKind::Input { channel, .. } = &base.kind {
                return quality_of(*channel, which, want, m, vars);
            }
            // `.valid` auf einem `T?` (3.8) ist das Flag des Wrappers —
            // dieselbe Frage wie bei einem Channel („ist ein Wert da?"),
            // nur mit einer anderen Quelle. `.age` und `.reason` gibt es
            // dort nicht: Ein Wrapper hat keine Herkunft.
            let LlvmType::Struct(fields) = &x.ty else {
                return Err(NotYet { what: "Qualitaet eines Nicht-Channels" });
            };
            if which != Accessor::Valid {
                return Err(NotYet { what: crate::scope::accessor_name(which) });
            }
            let r = m.inst(&format!("extractvalue {} {}, {}", x.ty, x.value, fields.len() - 1));
            Ok(Lowered { value: r.to_string(), ty: LlvmType::Int(1) })
        }
        // `x.or(d)`: der Wert, wenn gueltig, sonst der Ersatz (3.5).
        Accessor::Or => {
            let valid = match &base.kind {
                ExprKind::Input { channel, .. } => quality_of(*channel, Accessor::Valid, &LlvmType::Int(1), m, vars)?,
                // Auf einem `T?`/`T!E` ist es das Flag des Wrappers (3.8).
                _ => {
                    let LlvmType::Struct(fields) = &x.ty else {
                        return Err(NotYet { what: "`.or` auf einem Nicht-Channel" });
                    };
                    let f = m.inst(&format!("extractvalue {} {}, {}", x.ty, x.value, fields.len() - 1));
                    Lowered { value: f.to_string(), ty: LlvmType::Int(1) }
                }
            };
            let fallback = arg(0, m)?;
            // `select` statt Verzweigung: Beide Seiten sind total (4.1),
            // und der Wert ist ohnehin schon geladen.
            // Bei einem Wrapper ist der Wert das Feld 0, nicht der
            // Wrapper selbst (3.8).
            let wert = match (&base.kind, &x.ty) {
                (ExprKind::Input { .. }, _) => x.clone(),
                (_, LlvmType::Struct(_)) => {
                    let v = m.inst(&format!("extractvalue {} {}, 0", x.ty, x.value));
                    Lowered { value: v.to_string(), ty: want.clone() }
                }
                _ => x.clone(),
            };
            let r = m.inst(&format!(
                "select i1 {}, {} {}, {} {}",
                valid.value, wert.ty, wert.value, fallback.ty, fallback.value
            ));
            Ok(Lowered { value: r.to_string(), ty: want.clone() })
        }
        // `f.encode()` (3.7): der Record als Bytes seiner deklarierten
        // Laenge.
        Accessor::Encode => {
            let Some(Type::Record(r)) = p.types.list.get(base.ty.index()) else {
                return Err(NotYet { what: "`encode` auf einem Nicht-Record" });
            };
            crate::wire::encode(&x, *r, want, p, m)
        }
        // `.ok`/`.err` (3.8): das Flag und die Diskriminante.
        Accessor::Ok => {
            let LlvmType::Struct(fields) = &x.ty else { return Err(NotYet { what: "`.ok` ohne Wrapper" }) };
            let r = m.inst(&format!("extractvalue {} {}, {}", x.ty, x.value, fields.len() - 1));
            Ok(Lowered { value: r.to_string(), ty: LlvmType::Int(1) })
        }
        Accessor::Err => {
            let LlvmType::Struct(fields) = &x.ty else { return Err(NotYet { what: "`.err` ohne Wrapper" }) };
            if fields.len() != 3 {
                return Err(NotYet { what: "`.err` auf einem `T?`" });
            }
            let r = m.inst(&format!("extractvalue {} {}, 1", x.ty, x.value));
            Ok(Lowered { value: r.to_string(), ty: want.clone() })
        }
        // `.len` einer Sammlung (3.9): das Laengenfeld des Structs.
        Accessor::Len => {
            let LlvmType::Struct(_) = &x.ty else { return Err(NotYet { what: "`.len` auf einer Nicht-Sammlung" }) };
            let r = m.inst(&format!("extractvalue {} {}, 0", x.ty, x.value));
            // Die Laenge steht als `i32` im Struct; `int` ist `i64` (3.2).
            let wide = m.inst(&format!("sext i32 {r} to {want}"));
            Ok(Lowered { value: wide.to_string(), ty: want.clone() })
        }
        _ => Err(NotYet { what: crate::scope::accessor_name(which) }),
    }
}

/// Ein Qualitaetszugriff auf einen Channel (3.5).
///
/// `.valid` ist der einzige, der rechnet: 3.5 sagt „`Good` oder `Suspect`
/// mit Wert", also `quality <= SUSPECT`. Die uebrigen vergleichen oder
/// lesen direkt.
fn quality_of(
    channel: takt_mir::ChannelId,
    which: Accessor,
    want: &LlvmType,
    m: &mut Module,
    vars: &dyn Vars,
) -> Result<Lowered, NotYet> {
    use crate::image::{Slot, quality};
    let slot = match which {
        Accessor::Age => Slot::Age,
        Accessor::Reason => Slot::Reason,
        _ => Slot::Quality,
    };
    let q = vars.quality(channel, slot, m).ok_or(NotYet { what: "Qualitaet im Abbild" })?;
    let value = match which {
        // `.valid`: Good oder Suspect. Ein `Bad` ohne Wert und ein `Stale`
        // sind beide ungueltig, und beide stehen ueber `SUSPECT`.
        Accessor::Valid => m.inst(&format!("icmp sle {} {}, {}", q.ty, q.value, quality::SUSPECT)).to_string(),
        Accessor::Suspect => m.inst(&format!("icmp eq {} {}, {}", q.ty, q.value, quality::SUSPECT)).to_string(),
        Accessor::Stale => m.inst(&format!("icmp eq {} {}, {}", q.ty, q.value, quality::STALE)).to_string(),
        // `.age` und `.reason` stehen so, wie sie sind.
        _ => q.value,
    };
    let ty = match which {
        Accessor::Valid | Accessor::Suspect | Accessor::Stale => LlvmType::Int(1),
        _ => want.clone(),
    };
    Ok(Lowered { value, ty })
}

/// Eine Enum-Variante (3.7).
///
/// Eine Variante ohne Felder *ist* ihre Diskriminante — eine Konstante,
/// die kein Register belegt. Die Diskriminante steht in der MIR (explizit
/// oder fortlaufend vergeben); der Codegen rechnet sie nicht nach, sonst
/// gaebe es zwei Stellen, an denen sie entsteht.
fn self_variant(
    enum_id: takt_mir::EnumId,
    variant: u32,
    fields: &[Expr],
    want: &LlvmType,
    p: &Program,
) -> Result<Lowered, NotYet> {
    if !fields.is_empty() {
        // Eine Variante mit Feldern braucht ein Struct aus Diskriminante
        // und Nutzlast; `ty::lower` lehnt solche Enums heute ab, und der
        // Musterabgleich, der sie auspackt, fehlt ebenso.
        return Err(NotYet { what: "Variante mit Feldern" });
    }
    let def = p.enums.get(enum_id.index()).ok_or(NotYet { what: "Enum" })?;
    let v = def.variants.get(variant as usize).ok_or(NotYet { what: "Variante" })?;
    Ok(Lowered { value: v.discriminant.to_string(), ty: want.clone() })
}

/// Baut ein `T?` oder `T!E` (3.8).
///
/// Das Flag steht am Ende des Structs: `ok` setzt es, `err` loescht es
/// und legt die Fehlerdiskriminante daneben. So liegt der Wert an
/// derselben Stelle wie ohne Wrapper, und das Auspacken ist ein
/// `extractvalue 0` — dieselbe Instruktion fuer beide Formen.
fn wrap(
    inner: &Expr,
    ok: bool,
    want: &LlvmType,
    p: &Program,
    m: &mut Module,
    vars: &dyn Vars,
) -> Result<Lowered, NotYet> {
    let LlvmType::Struct(fields) = want else { return Err(NotYet { what: "`ok`/`err` ohne Wrapper-Typ" }) };
    let v = lower(inner, p, m, vars)?;
    let mut cur = "undef".to_string();
    if ok {
        cur = m.inst(&format!("insertvalue {want} {cur}, {} {}, 0", v.ty, v.value)).to_string();
    } else if fields.len() == 3 {
        // Bei `err` traegt das Feld 1 die Diskriminante; der Wert bleibt
        // undefiniert, weil ihn niemand lesen darf (3.8: Dominanz).
        cur = m.inst(&format!("insertvalue {want} {cur}, {} {}, 1", v.ty, v.value)).to_string();
    }
    let flag = fields.len() - 1;
    let bit = if ok { "true" } else { "false" };
    let r = m.inst(&format!("insertvalue {want} {cur}, i1 {bit}, {flag}"));
    Ok(Lowered { value: r.to_string(), ty: want.clone() })
}

/// Eine Primitive (4.1).
///
/// Die Integer-Primitiven sind hier vollstaendig: Sie sind exakt
/// definiert und brauchen keine Bibliothek (plan/m4.md 3.1). Die
/// Fliesskomma-Primitiven ruft `libtaktm`, und was dort nicht kuratiert
/// ist, hat der Compiler schon abgelehnt (13.8) — der Codegen sieht es
/// nicht mehr.
fn intrinsic(
    op: Intrinsic,
    args: &[Expr],
    want: &LlvmType,
    p: &Program,
    m: &mut Module,
    vars: &dyn Vars,
) -> Result<Lowered, NotYet> {
    let mut ops = Vec::with_capacity(args.len());
    for a in args {
        ops.push(lower(a, p, m, vars)?);
    }
    let a = |i: usize| ops.get(i).cloned().ok_or(NotYet { what: "Argument einer Primitive" });
    let value = match op {
        // 4.1: Die `wrapping_*`-Primitiven sind der *explizite* Umlauf;
        // der gewoehnliche Operator faultet statt umzulaufen. In LLVM ist
        // der Umlauf das Standardverhalten ohne `nsw`/`nuw` — dieselbe
        // Instruktion, nur ohne den Ueberlauf-Check darum herum.
        Intrinsic::WrappingAdd => {
            let (x, y) = (a(0)?, a(1)?);
            m.inst(&format!("add {} {}, {}", x.ty, x.value, y.value))
        }
        Intrinsic::WrappingSub => {
            let (x, y) = (a(0)?, a(1)?);
            m.inst(&format!("sub {} {}, {}", x.ty, x.value, y.value))
        }
        Intrinsic::WrappingMul => {
            let (x, y) = (a(0)?, a(1)?);
            m.inst(&format!("mul {} {}, {}", x.ty, x.value, y.value))
        }
        // LLVM hat die Saettigung als Intrinsic; sie von Hand zu bauen
        // waere drei Instruktionen und eine Gelegenheit fuer einen Fehler.
        Intrinsic::SaturatingAdd | Intrinsic::SaturatingSub => {
            let (x, y) = (a(0)?, a(1)?);
            let signed = int_is_signed_ty(args[0].ty, p);
            let name = match (op, signed) {
                (Intrinsic::SaturatingAdd, true) => "sadd.sat",
                (Intrinsic::SaturatingAdd, false) => "uadd.sat",
                (_, true) => "ssub.sat",
                (_, false) => "usub.sat",
            };
            m.needs_intrinsic(&format!("{} @llvm.{name}.{}({}, {})", x.ty, x.ty, x.ty, x.ty));
            m.inst(&format!("call {} @llvm.{name}.{}({} {}, {} {})", x.ty, x.ty, x.ty, x.value, y.ty, y.value))
        }
        Intrinsic::Rotl | Intrinsic::Rotr => {
            let (x, y) = (a(0)?, a(1)?);
            let name = if op == Intrinsic::Rotl { "fshl" } else { "fshr" };
            // `fshl(x, x, n)` ist die Rotation: Der Trichter nimmt
            // dieselbe Zahl als beide Haelften.
            m.needs_intrinsic(&format!("{} @llvm.{name}.{}({}, {}, {})", x.ty, x.ty, x.ty, x.ty, x.ty));
            m.inst(&format!(
                "call {} @llvm.{name}.{}({} {}, {} {}, {} {})",
                x.ty, x.ty, x.ty, x.value, x.ty, x.value, y.ty, y.value
            ))
        }
        Intrinsic::Abs if !want.is_float() => {
            let x = a(0)?;
            // `abs` auf einem Integer faultet beim kleinsten Wert (4.1);
            // der `Checked`-Knoten der MIR steht darum herum, und `false`
            // heisst hier „kein undefiniertes Verhalten".
            m.needs_intrinsic(&format!("{} @llvm.abs.{}({}, i1)", x.ty, x.ty, x.ty));
            m.inst(&format!("call {} @llvm.abs.{}({} {}, i1 false)", x.ty, x.ty, x.ty, x.value))
        }
        Intrinsic::Min | Intrinsic::Max => {
            let (x, y) = (a(0)?, a(1)?);
            let signed = int_is_signed_ty(args[0].ty, p);
            let name = match (op == Intrinsic::Min, x.ty.is_float(), signed) {
                (true, true, _) => "minnum",
                (false, true, _) => "maxnum",
                (true, false, true) => "smin",
                (true, false, false) => "umin",
                (false, false, true) => "smax",
                (false, false, false) => "umax",
            };
            m.needs_intrinsic(&format!("{} @llvm.{name}.{}({}, {})", x.ty, x.ty, x.ty, x.ty));
            m.inst(&format!("call {} @llvm.{name}.{}({} {}, {} {})", x.ty, x.ty, x.ty, x.value, y.ty, y.value))
        }
        // 3.9: stueckweise linear, an den Raendern geklemmt, total. Die
        // Stuetzstellen stehen als Literal am Aufruf (der Compiler faltet
        // die Konstante dorthin), also ist ihre Zahl bekannt und die
        // Suche abgerollt — keine Schleife, keine Schranke zu pruefen.
        Intrinsic::Interp => return interp(args, want, p, m, vars),
        _ => return Err(NotYet { what: "Primitive" }),
    };
    Ok(Lowered { value: value.to_string(), ty: want.clone() })
}

/// `R.decode(b)` (3.7).
///
/// Der Puffer liegt als Wert vor; `decode` liest ihn byteweise und
/// braucht dafuer einen Platz. 11.2 nennt ihn den statischen Scratch —
/// LLVM hebt die `alloca` in den Eintrittsblock und entfernt sie, wo sie
/// unnoetig ist.
fn decode(
    record: takt_mir::RecordId,
    bytes: &Expr,
    want: &LlvmType,
    p: &Program,
    m: &mut Module,
    vars: &dyn Vars,
) -> Result<Lowered, NotYet> {
    let b = lower(bytes, p, m, vars)?;
    let LlvmType::Struct(fields) = &b.ty else { return Err(NotYet { what: "`decode` auf einer Nicht-Sammlung" }) };
    let tmp = m.inst(&format!("alloca {}", b.ty));
    m.void_inst(&format!("store {} {}, ptr {tmp}", b.ty, b.value));
    // Die Laenge steht im Kopf der Sammlung (3.9), die Daten dahinter.
    let len = m.inst(&format!("extractvalue {} {}, 0", b.ty, b.value));
    let data = m.inst(&format!("getelementptr inbounds {}, ptr {tmp}, i32 0, i32 1", b.ty));
    let _ = fields;
    let label = m.next_label();
    crate::wire::decode(data, &len.to_string(), record, want, p, m, label)
}

/// Prueft die Gueltigkeit eines Channels und springt sonst in den
/// Fault-Pfad (3.5, 4.1).
///
/// Der Zweig steht vor dem Lesen: Bei `Bad` traegt das Abbild keinen
/// Wert, und ihn zu lesen waere die stille Korruption, die 12.6
/// ausschliesst.
fn valid_or_fault(channel: takt_mir::ChannelId, m: &mut Module, vars: &dyn Vars) -> Result<(), NotYet> {
    let Some(target) = vars.fault_label() else {
        return Err(NotYet { what: "Gueltigkeitspruefung ohne Fault-Pfad" });
    };
    let q = vars.quality(channel, crate::image::Slot::Quality, m).ok_or(NotYet { what: "Qualitaet im Abbild" })?;
    // `.valid` ist `Good` oder `Suspect` (3.5), also `<= SUSPECT`.
    let ok = m.inst(&format!("icmp sle {} {}, {}", q.ty, q.value, crate::image::quality::SUSPECT));
    let weiter = format!("gueltig{}", m.next_label());
    m.void_inst(&format!("br i1 {ok}, label %{weiter}, label %{target}"));
    m.label(&weiter);
    Ok(())
}

/// `x[a..b]` (3.9): ein Teilstueck als eigene Sammlung.
///
/// Die Grenzen prueft der `Checked`-Knoten der MIR (4.1); hier steht die
/// Kopie. Sie geht ueber `llvm.memcpy`, weil die Laenge erst zur Laufzeit
/// feststeht — eine Schleife braeuchte eine Schranke, und die waere die
/// Kapazitaet, nicht die Laenge.
fn slice(
    base: &Expr,
    from: &Expr,
    to: &Expr,
    want: &LlvmType,
    p: &Program,
    m: &mut Module,
    vars: &dyn Vars,
) -> Result<Lowered, NotYet> {
    let x = lower(base, p, m, vars)?;
    let a = lower(from, p, m, vars)?;
    let b = lower(to, p, m, vars)?;
    let src = crate::collection::layout_of(&x.ty).ok_or(NotYet { what: "Teilbereich einer Nicht-Sammlung" })?;
    let dst = crate::collection::layout_of(want).ok_or(NotYet { what: "Teilbereich ohne Zielsammlung" })?;
    // Quelle und Ziel liegen als Werte vor; `memcpy` liest und schreibt
    // Speicher (11.2: statischer Scratch).
    let src_ptr = m.inst(&format!("alloca {}", x.ty));
    m.void_inst(&format!("store {} {}, ptr {src_ptr}", x.ty, x.value));
    let out = m.inst(&format!("alloca {want}"));
    let len = m.inst(&format!("sub {} {}, {}", a.ty, b.value, a.value));
    let len32 = m.inst(&format!("trunc {} {len} to i32", a.ty));
    let len_ptr = m.inst(&format!("getelementptr inbounds {want}, ptr {out}, i32 0, i32 0"));
    m.void_inst(&format!("store i32 {len32}, ptr {len_ptr}"));
    let src_data = m.inst(&format!("getelementptr inbounds {}, ptr {src_ptr}, i32 0, i32 1", x.ty));
    let at = m.inst(&format!(
        "getelementptr inbounds [{} x {}], ptr {src_data}, i32 0, {} {}",
        src.cap, src.elem, a.ty, a.value
    ));
    let dst_data = m.inst(&format!("getelementptr inbounds {want}, ptr {out}, i32 0, i32 1"));
    let bytes = m.inst(&format!("mul {} {len}, {}", a.ty, dst.elem.size().max(1)));
    m.void_inst(&format!("call void @llvm.memcpy.p0.p0.i64(ptr {dst_data}, ptr {at}, i64 {bytes}, i1 false)"));
    let v = m.inst(&format!("load {want}, ptr {out}"));
    Ok(Lowered { value: v.to_string(), ty: want.clone() })
}

/// `interp(t, x)` (3.9): stueckweise lineare Interpolation.
///
/// **Die Stuetzstellen stehen als Literal am Aufruf.** `table<A, B>` ist
/// immer eine Konstante (3.9), und der Compiler faltet sie an die
/// Aufrufstelle — ihre Zahl ist damit bekannt, und die Suche wird
/// abgerollt. Das ist nicht nur schneller als eine Schleife: 4.1 verlangt
/// eine Schranke, und die abgerollte Form *ist* die Schranke.
///
/// Die Rechnung ist die des Interpreters, Operation fuer Operation:
/// `y0 + (y1 - y0) * (x - x0) / (x1 - x0)`, jede einzeln gerundet. Eine
/// andere Klammerung waere mathematisch gleich und in Fliesskomma eine
/// andere Zahl (Satz 9.4.4).
///
/// An den Raendern wird geklemmt, nicht extrapoliert — das haelt die
/// Funktion total (4.1) und vermeidet, dass ein Wert ausserhalb der
/// Kennlinie eine Zahl erfindet.
fn interp(args: &[Expr], want: &LlvmType, p: &Program, m: &mut Module, vars: &dyn Vars) -> Result<Lowered, NotYet> {
    let table = args.first().ok_or(NotYet { what: "`interp` ohne Tabelle" })?;
    let ExprKind::Array(points) = &table.kind else {
        return Err(NotYet { what: "`interp` ueber eine berechnete Tabelle" });
    };
    if points.len() < 2 {
        return Err(NotYet { what: "`interp` ueber weniger als zwei Stuetzstellen" });
    }
    let x = lower(args.get(1).ok_or(NotYet { what: "`interp` ohne Argument" })?, p, m, vars)?;

    // Die Stuetzstellen einzeln senken; sie sind Literale, belegen also
    // kein Register.
    let mut xs = Vec::with_capacity(points.len());
    let mut ys = Vec::with_capacity(points.len());
    for pt in points {
        let ExprKind::Tuple(a, b) = &pt.kind else { return Err(NotYet { what: "Stuetzstelle" }) };
        xs.push(lower(a, p, m, vars)?);
        ys.push(lower(b, p, m, vars)?);
    }

    // Von hinten nach vorn: Das Ergebnis ist der geklemmte rechte Rand,
    // und jede Stuetzstelle davor ueberschreibt es, wenn `x` in ihr
    // Segment faellt. So entsteht eine Kette von `select`, die den ersten
    // Treffer von links gewinnen laesst — dieselbe Reihenfolge wie die
    // Schleife des Interpreters, ohne Verzweigung.
    let mut result = ys.last().expect("nicht leer").value.clone();
    for i in (0..points.len() - 1).rev() {
        let (x0, x1) = (&xs[i], &xs[i + 1]);
        let (y0, y1) = (&ys[i], &ys[i + 1]);
        let dy = m.inst(&format!("fsub {want} {}, {}", y1.value, y0.value));
        let dx = m.inst(&format!("fsub {want} {}, {}", x1.value, x0.value));
        let dxi = m.inst(&format!("fsub {want} {}, {}", x.value, x0.value));
        let scaled = m.inst(&format!("fmul {want} {dy}, {dxi}"));
        let ratio = m.inst(&format!("fdiv {want} {scaled}, {dx}"));
        let segment = m.inst(&format!("fadd {want} {}, {ratio}", y0.value));
        // `x <= x1` waehlt dieses Segment; weiter links liegende
        // ueberschreiben es in der naechsten Runde.
        let in_segment = m.inst(&format!("fcmp ole {want} {}, {}", x.value, x1.value));
        result = m.inst(&format!("select i1 {in_segment}, {want} {segment}, {want} {result}")).to_string();
    }
    // Links vom ersten Stuetzpunkt wird geklemmt (3.9).
    let below = m.inst(&format!("fcmp ole {want} {}, {}", x.value, xs[0].value));
    let clamped = m.inst(&format!("select i1 {below}, {want} {}, {want} {result}", ys[0].value));
    Ok(Lowered { value: clamped.to_string(), ty: want.clone() })
}

/// Ein Index (3.9).
///
/// Die Grenze prueft der `Checked`-Knoten der MIR (4.1); hier steht der
/// Zugriff. Eine Sammlung wird ueber ihr `data`-Feld indiziert, ein Array
/// unmittelbar.
fn index_of(
    base: &Expr,
    index: &Expr,
    want: &LlvmType,
    p: &Program,
    m: &mut Module,
    vars: &dyn Vars,
) -> Result<Lowered, NotYet> {
    let x = lower(base, p, m, vars)?;
    let i = lower(index, p, m, vars)?;
    // Der Wert liegt als Register vor, nicht im Speicher; ein
    // `extractvalue` mit berechnetem Index gibt es nicht. Er bekommt
    // darum einen Platz (11.2: statischer Scratch), und LLVM entfernt
    // ihn, wo er unnoetig ist.
    let tmp = m.inst(&format!("alloca {}", x.ty));
    m.void_inst(&format!("store {} {}, ptr {tmp}", x.ty, x.value));
    let (array_ty, data) = match &x.ty {
        LlvmType::Struct(_) => {
            let l = crate::collection::layout_of(&x.ty).ok_or(NotYet { what: "Index auf diesem Struct" })?;
            let d = m.inst(&format!("getelementptr inbounds {}, ptr {tmp}, i32 0, i32 1", x.ty));
            (LlvmType::Array(Box::new(l.elem), l.cap), d)
        }
        LlvmType::Array(..) => (x.ty.clone(), tmp),
        _ => return Err(NotYet { what: "Index auf diesem Typ" }),
    };
    let at = m.inst(&format!("getelementptr inbounds {array_ty}, ptr {data}, i32 0, {} {}", i.ty, i.value));
    let v = m.inst(&format!("load {want}, ptr {at}"));
    Ok(Lowered { value: v.to_string(), ty: want.clone() })
}

/// `default` eines Typs (3.7): 0, `false`, leere Sammlung.
///
/// LLVM hat dafuer `zeroinitializer` — ein Wert, der kein Register belegt
/// und den der Linker in `.bss` legt. Das ist nicht nur kuerzer als Feld
/// fuer Feld zu schreiben, es ist auch das, was 12.3 fuer den
/// Speicherbedarf annimmt: Nullen kosten kein Flash.
fn default_of(want: &LlvmType) -> Result<Lowered, NotYet> {
    match want {
        LlvmType::Void => Err(NotYet { what: "`default` ohne Typ" }),
        // Eine leere Sammlung ist `{ len = 0, data = beliebig }`; die
        // Elemente jenseits der Laenge sind nicht lesbar (3.9).
        _ => Ok(Lowered { value: "zeroinitializer".into(), ty: want.clone() }),
    }
}

/// Ein Aufruf einer reinen Funktion (4.4).
///
/// Reine Funktionen haben keinen Zustand und keinen Seiteneffekt; der
/// Aufruf ist darum eine gewoehnliche `call`-Instruktion, und LLVM darf
/// sie einbetten oder mehrfache Aufrufe mit denselben Argumenten
/// zusammenfassen.
fn call(
    func: takt_mir::FnId,
    args: &[Expr],
    want: &LlvmType,
    p: &Program,
    m: &mut Module,
    vars: &dyn Vars,
) -> Result<Lowered, NotYet> {
    let f = p.fns.get(func.index()).ok_or(NotYet { what: "Funktion" })?;
    let mut operands = Vec::with_capacity(args.len());
    for a in args {
        let v = lower(a, p, m, vars)?;
        operands.push(format!("{} {}", v.ty, v.value));
    }
    let name = crate::fns::symbol(f);
    if *want == LlvmType::Void {
        m.void_inst(&format!("call void @{name}({})", operands.join(", ")));
        return Ok(Lowered { value: String::new(), ty: LlvmType::Void });
    }
    let r = m.inst(&format!("call {want} @{name}({})", operands.join(", ")));
    Ok(Lowered { value: r.to_string(), ty: want.clone() })
}

/// Ein Record-Literal (3.7).
///
/// LLVM baut einen Struct-Wert mit `insertvalue`, Feld fuer Feld, aus
/// `undef` heraus. Das ist die Umkehrung von `extractvalue` beim
/// Feldzugriff und bleibt wie dieser ein Wert — kein Speicherort, keine
/// Kopie (11.2: die Sprache hat keine Referenzen).
fn record(fields: &[Expr], want: &LlvmType, p: &Program, m: &mut Module, vars: &dyn Vars) -> Result<Lowered, NotYet> {
    let n = match want {
        LlvmType::Struct(types) => types.len(),
        LlvmType::Array(_, len) => *len as usize,
        _ => return Err(NotYet { what: "Literal ohne zusammengesetzten Typ" }),
    };
    if n != fields.len() {
        return Err(NotYet { what: "Literal mit anderer Elementzahl" });
    }
    let mut cur = "undef".to_string();
    for (i, f) in fields.iter().enumerate() {
        let v = lower(f, p, m, vars)?;
        cur = m.inst(&format!("insertvalue {want} {cur}, {} {}, {i}", v.ty, v.value)).to_string();
    }
    Ok(Lowered { value: cur, ty: want.clone() })
}

/// Ein Feldzugriff auf einen Record (3.7).
///
/// `extractvalue` statt `getelementptr` plus `load`: Der Record liegt als
/// Wert vor, nicht als Speicherort — die Sprache hat keine Referenzen
/// (11.2), jede Zuweisung ist eine Kopie, und LLVM faltet die Extraktion
/// aus einem geladenen Struct ohnehin zusammen.
fn field_of(
    base: &Expr,
    field: u32,
    want: &LlvmType,
    p: &Program,
    m: &mut Module,
    vars: &dyn Vars,
) -> Result<Lowered, NotYet> {
    // 3.8 (Dominanz): Ein Feld auf einem Wrapper meint das Feld des
    // Inhalts. Ausgepackt hat ihn schon der `Checked{Missing}`-Knoten,
    // den die MIR darum herum setzt — hier steht nur noch der Zugriff.
    let x = lower(base, p, m, vars)?;
    let LlvmType::Struct(_) = &x.ty else { return Err(NotYet { what: "Feldzugriff auf Nicht-Record" }) };
    let r = m.inst(&format!("extractvalue {} {}, {field}", x.ty, x.value));
    Ok(Lowered { value: r.to_string(), ty: want.clone() })
}

/// `x as T` (3.10).
///
/// Die Range-Pruefung steht als `Checked`-Knoten *um* den Cast (die MIR
/// setzt ihn), nicht hier — der Codegen erzeugt die Umwandlung, nicht ihre
/// Absicherung. Welche Instruktion es ist, entscheiden Quelle und Ziel:
/// `trunc` verkuerzt, `sext`/`zext` verlaengern (mit Vorzeichen oder ohne),
/// `fptosi`/`sitofp` wechseln die Domaene.
fn cast(
    e: &Expr,
    to: TypeId,
    want: &LlvmType,
    p: &Program,
    m: &mut Module,
    vars: &dyn Vars,
) -> Result<Lowered, NotYet> {
    let x = lower(e, p, m, vars)?;
    if x.ty == *want {
        return Ok(x);
    }
    let from_signed = int_is_signed_ty(e.ty, p);
    let to_signed = int_is_signed_ty(to, p);
    let op = match (&x.ty, want) {
        (LlvmType::Int(a), LlvmType::Int(b)) if a > b => "trunc",
        (LlvmType::Int(a), LlvmType::Int(b)) if a < b => {
            // Vorzeichen der *Quelle* entscheidet: Ein `u8` in einen `i32`
            // wird mit Nullen aufgefuellt, ein `i8` mit dem Vorzeichenbit.
            if from_signed { "sext" } else { "zext" }
        }
        (LlvmType::Int(_), LlvmType::F32 | LlvmType::F64) => {
            if from_signed {
                "sitofp"
            } else {
                "uitofp"
            }
        }
        (LlvmType::F32 | LlvmType::F64, LlvmType::Int(_)) => {
            // 4.1: `as` auf einen Integer schneidet ab; die Rundung ist
            // `round`/`floor`/`ceil` und damit eine eigene Primitive.
            if to_signed { "fptosi" } else { "fptoui" }
        }
        (LlvmType::F64, LlvmType::F32) => "fptrunc",
        (LlvmType::F32, LlvmType::F64) => "fpext",
        _ => return Err(NotYet { what: "`as` zwischen diesen Typen" }),
    };
    let r = m.inst(&format!("{op} {} {} to {want}", x.ty, x.value));
    Ok(Lowered { value: r.to_string(), ty: want.clone() })
}

/// Einheitenkonversion (3.2, 3.3).
///
/// Der Faktor zwischen zwei Einheiten derselben Dimension ist eine
/// rationale Zahl und zur Uebersetzungszeit bekannt; die Konversion ist
/// darum eine Multiplikation, keine Tabelle.
///
/// **Die Reihenfolge der Operationen ist dieselbe wie im Interpreter**
/// (`takt-interp/src/eval.rs`, `convert`): erst der affine Versatz der
/// Quelle, dann `* num`, dann `/ den`, dann der Versatz des Ziels. Eine
/// andere Reihenfolge waere mathematisch gleich, aber in Fliesskomma eine
/// andere Zahl — und Satz 9.4.4 verlangt dasselbe Bit.
fn convert(
    e: &Expr,
    kind: ConvertKind,
    unit: takt_mir::UnitId,
    want: &LlvmType,
    p: &Program,
    m: &mut Module,
    vars: &dyn Vars,
) -> Result<Lowered, NotYet> {
    let x = lower(e, p, m, vars)?;
    let dst = p.units.get(unit.index()).ok_or(NotYet { what: "Zieleinheit" })?;
    match kind {
        // `d.as(U)`: Nanosekunden je Einheit ist `factor * 1e9`.
        ConvertKind::As => {
            if !want.is_float() {
                return Err(NotYet { what: "`as(U)` ohne Fliesskommaziel" });
            }
            let per = i128::from(dst.factor.num) * 1_000_000_000 / i128::from(dst.factor.den);
            let as_float = m.inst(&format!("sitofp {} {} to {want}", x.ty, x.value));
            let divisor = float_literal(per as f64, want);
            let r = m.inst(&format!("fdiv {want} {as_float}, {divisor}"));
            Ok(Lowered { value: r.to_string(), ty: want.clone() })
        }
        ConvertKind::To => {
            let src = match p.types.list.get(e.ty.index()) {
                Some(Type::Float { unit: Some(u), .. }) => p.units.get(u.index()).ok_or(NotYet { what: "Einheit" })?,
                // 3.2: Einheiten auf Ganzzahlen sind M6.
                _ => return Err(NotYet { what: "`to(U)` ohne Quelleinheit" }),
            };
            let mut cur = x.value;
            if let Some(off) = src.affine_offset {
                let o = float_literal(off.num as f64 / off.den as f64, want);
                cur = m.inst(&format!("fadd {want} {cur}, {o}")).to_string();
            }
            let num = i128::from(src.factor.num) * i128::from(dst.factor.den);
            let den = i128::from(src.factor.den) * i128::from(dst.factor.num);
            cur = m.inst(&format!("fmul {want} {cur}, {}", float_literal(num as f64, want))).to_string();
            cur = m.inst(&format!("fdiv {want} {cur}, {}", float_literal(den as f64, want))).to_string();
            if let Some(off) = dst.affine_offset {
                let o = float_literal(off.num as f64 / off.den as f64, want);
                cur = m.inst(&format!("fsub {want} {cur}, {o}")).to_string();
            }
            Ok(Lowered { value: cur, ty: want.clone() })
        }
        ConvertKind::ToFloat => Err(NotYet { what: "`to_float(U)` (M6)" }),
    }
}

/// Ist der Typ eine vorzeichenbehaftete Ganzzahl?
fn int_is_signed_ty(ty: TypeId, p: &Program) -> bool {
    match p.types.list.get(ty.index()) {
        Some(Type::Int { width, .. }) => ty::signed(*width),
        Some(Type::Duration { .. }) => true,
        _ => true,
    }
}

/// Ein einstelliger Operator.
fn unary(
    op: UnaryOp,
    expr: &Expr,
    want: &LlvmType,
    p: &Program,
    m: &mut Module,
    vars: &dyn Vars,
) -> Result<Lowered, NotYet> {
    let x = lower(expr, p, m, vars)?;
    let value = match op {
        // LLVM hat kein `fneg`-Flag-Problem: `fneg` ist exakt (nur das
        // Vorzeichenbit) und traegt darum ohnehin nie ein Flag.
        UnaryOp::Neg if x.ty.is_float() => m.inst(&format!("fneg {} {}", x.ty, x.value)),
        UnaryOp::Neg => m.inst(&format!("sub {} 0, {}", x.ty, x.value)),
        // `not` ist logisch und nur auf `bool` definiert, `~` bitweise.
        UnaryOp::Not => m.inst(&format!("xor i1 {}, true", x.value)),
        UnaryOp::BitNot => m.inst(&format!("xor {} {}, -1", x.ty, x.value)),
    };
    Ok(Lowered { value: value.to_string(), ty: want.clone() })
}

/// Ein zweistelliger Operator.
fn binary(
    op: BinaryOp,
    lhs: &Expr,
    rhs: &Expr,
    want: &LlvmType,
    p: &Program,
    m: &mut Module,
    vars: &dyn Vars,
) -> Result<Lowered, NotYet> {
    // `and`/`or` sind in Takt nicht kurzschluessig verschieden von `&`/`|`
    // auf `bool`: Beide Operanden sind total (4.1), also darf beides eine
    // Instruktion sein. Ein Kurzschluss waere hier eine Verhaltensaenderung
    // ohne Gewinn — es gibt keine Seiteneffekte, die er spaeren koennte.
    let a = lower(lhs, p, m, vars)?;
    let b = lower(rhs, p, m, vars)?;
    let float = a.ty.is_float();
    let signed = int_is_signed(lhs.ty, p);

    let text = match op {
        BinaryOp::Add if float => format!("fadd {} {}, {}", a.ty, a.value, b.value),
        BinaryOp::Sub if float => format!("fsub {} {}, {}", a.ty, a.value, b.value),
        BinaryOp::Mul if float => format!("fmul {} {}, {}", a.ty, a.value, b.value),
        BinaryOp::Div if float => format!("fdiv {} {}, {}", a.ty, a.value, b.value),
        BinaryOp::Rem if float => format!("frem {} {}, {}", a.ty, a.value, b.value),
        // Ganzzahlueberlauf ist ein Fault (4.1), kein Wraparound — und der
        // `Checked`-Knoten darueber prueft ihn. `nsw`/`nuw` waeren hier
        // falsch: Sie erklaeren den Ueberlauf fuer undefiniert, und dann
        // duerfte LLVM die Pruefung wegoptimieren, die ihn fangen soll.
        BinaryOp::Add => format!("add {} {}, {}", a.ty, a.value, b.value),
        BinaryOp::Sub => format!("sub {} {}, {}", a.ty, a.value, b.value),
        BinaryOp::Mul => format!("mul {} {}, {}", a.ty, a.value, b.value),
        BinaryOp::Div if signed => format!("sdiv {} {}, {}", a.ty, a.value, b.value),
        BinaryOp::Div => format!("udiv {} {}, {}", a.ty, a.value, b.value),
        BinaryOp::Rem if signed => format!("srem {} {}, {}", a.ty, a.value, b.value),
        BinaryOp::Rem => format!("urem {} {}, {}", a.ty, a.value, b.value),
        BinaryOp::BitAnd | BinaryOp::And => format!("and {} {}, {}", a.ty, a.value, b.value),
        BinaryOp::BitOr | BinaryOp::Or => format!("or {} {}, {}", a.ty, a.value, b.value),
        BinaryOp::BitXor => format!("xor {} {}, {}", a.ty, a.value, b.value),
        BinaryOp::Shl => format!("shl {} {}, {}", a.ty, a.value, b.value),
        // 3.10: `>>` auf vorzeichenbehafteten Werten ist arithmetisch.
        BinaryOp::Shr if signed => format!("ashr {} {}, {}", a.ty, a.value, b.value),
        BinaryOp::Shr => format!("lshr {} {}, {}", a.ty, a.value, b.value),
        // Vergleiche: `o`-Praefix heisst „geordnet", also falsch bei NaN.
        // NaN kann in Takt nicht entstehen (4.1, `finite()`), aber die
        // geordnete Form ist die, die ohne diese Zusage richtig bleibt.
        BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge | BinaryOp::Eq | BinaryOp::Ne => {
            compare(op, &a, &b, float, signed)
        }
    };
    let r = m.inst(&text);
    let out = if matches!(op, BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge | BinaryOp::Eq | BinaryOp::Ne) {
        LlvmType::Int(1)
    } else {
        want.clone()
    };
    Ok(Lowered { value: r.to_string(), ty: out })
}

/// Ein Vergleich.
fn compare(op: BinaryOp, a: &Lowered, b: &Lowered, float: bool, signed: bool) -> String {
    if float {
        let cc = match op {
            BinaryOp::Lt => "olt",
            BinaryOp::Le => "ole",
            BinaryOp::Gt => "ogt",
            BinaryOp::Ge => "oge",
            BinaryOp::Eq => "oeq",
            _ => "one",
        };
        return format!("fcmp {cc} {} {}, {}", a.ty, a.value, b.value);
    }
    let cc = match (op, signed) {
        (BinaryOp::Lt, true) => "slt",
        (BinaryOp::Lt, false) => "ult",
        (BinaryOp::Le, true) => "sle",
        (BinaryOp::Le, false) => "ule",
        (BinaryOp::Gt, true) => "sgt",
        (BinaryOp::Gt, false) => "ugt",
        (BinaryOp::Ge, true) => "sge",
        (BinaryOp::Ge, false) => "uge",
        (BinaryOp::Eq, _) => "eq",
        _ => "ne",
    };
    format!("icmp {cc} {} {}, {}", a.ty, a.value, b.value)
}

/// Ist der Typ eine vorzeichenbehaftete Ganzzahl?
///
/// `bool` und die Dauern zaehlen als vorzeichenbehaftet: Dauern sind
/// Nanosekunden in `i64` und duerfen negativ sein (3.2).
fn int_is_signed(ty: TypeId, p: &Program) -> bool {
    match p.types.list.get(ty.index()) {
        Some(Type::Int { width, .. }) => ty::signed(*width),
        Some(Type::Duration { .. }) => true,
        _ => true,
    }
}

/// Die Breite eines Integertyps, wenn es einer ist.
pub fn int_width(ty: TypeId, p: &Program) -> Option<IntWidth> {
    match p.types.list.get(ty.index())? {
        Type::Int { width, .. } => Some(*width),
        _ => None,
    }
}

/// Der Name eines MIR-Knotens fuer die Meldung.
///
/// Eine Meldung, die nur „Ausdruck" sagt, zeigt nicht, was fehlt — und
/// genau danach fragt, wer den Codegen weiterbaut.
fn node_name(e: &ExprKind) -> &'static str {
    match e {
        ExprKind::Bool(_) | ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Duration(_) => "Literal",
        ExprKind::Str(_) => "Zeichenkette",
        ExprKind::None => "`none`",
        ExprKind::Default => "`default`",
        ExprKind::Variant { .. } => "Variante",
        ExprKind::Record { .. } => "Record",
        ExprKind::Array(_) => "Array-Literal",
        ExprKind::Tuple(_, _) => "Stuetzstelle",
        ExprKind::BlockInit { .. } => "Blockinstanz",
        ExprKind::Command(_) => "Command",
        ExprKind::Published { .. } => "Psi",
        ExprKind::StateOf(_) => "`m.state`",
        ExprKind::Signal { .. } => "Signal",
        ExprKind::Builtin(_) => "eingebauter Bezeichner",
        ExprKind::Field { .. } => "Feldzugriff",
        ExprKind::Index { .. } => "Index",
        ExprKind::Accessor { .. } => "Zugriff (`.valid`, `.age`, ...)",
        ExprKind::Call { .. } => "Funktionsaufruf",
        ExprKind::NativeCall { .. } => "native Funktion",
        ExprKind::Convert { .. } => "Konversion",
        ExprKind::Cast { .. } => "`as`",
        ExprKind::Cond { .. } => "Bedingter Ausdruck",
        ExprKind::Slice { .. } => "Teilbereich",
        ExprKind::Matches { .. } => "`matches`",
        ExprKind::MatOp { .. } => "Matrixoperation",
        ExprKind::Decode { .. } => "`decode`",
        ExprKind::Format(_) => "Format-String",
        ExprKind::Stream(_) => "Strom",
        ExprKind::Lift(_) => "Anhebung",
        ExprKind::Ok(_) | ExprKind::Err(_) => "`ok`/`err`",
        _ => "Ausdruck",
    }
}
