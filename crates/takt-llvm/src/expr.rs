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
use takt_mir::expr::{Accessor, BinaryOp, ConvertKind, Expr, ExprKind, Intrinsic, JobField, UnaryOp};
use takt_mir::program::Program;
use takt_mir::stmt::Place;
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

    /// Die Adresse einer Variablen, wo es eine gibt.
    ///
    /// Wer nur adressiert — `.len`, ein Feld, ein Index — nimmt sie statt
    /// des Werts: Ein `load` des ganzen Aggregats fuer vier Byte Laenge
    /// kostet bei `bytes<1024>` das Tausendfache (FB-214).
    fn address(&self, _id: takt_mir::VarId, _m: &mut Module) -> Option<(crate::emit::Reg, LlvmType)> {
        None
    }

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

    /// Die Nummer der Maschine im Programm; `None` ausserhalb einer Maschine.
    fn machine_index(&self) -> Option<u32> {
        None
    }

    /// `armed` und Cursor eines Triggers im Zustand seines Besitzers (7.5).
    fn trigger_slots(&self, _t: takt_mir::TriggerId, _m: &mut Module) -> Option<(crate::emit::Reg, crate::emit::Reg)> {
        None
    }

    /// Cursor und `examined` eines Stroms im Zustand der Maschine (9.6),
    /// als Zeiger; `None` ausserhalb einer Maschine.
    fn stream_slots(
        &self,
        _stream: takt_mir::expr::StreamRef,
        _m: &mut Module,
    ) -> Option<(crate::emit::Reg, crate::emit::Reg)> {
        None
    }

    /// Eine eingebaute Groesse (3.3, 5.3).
    ///
    /// Sie haengt an der Quelle: `now` kommt von der Runtime, die die
    /// Uhr fuehrt (12.1); `time_in_state` steht im Zustand der Maschine;
    /// `tick` ist eine Konstante des Programms. Eine reine Funktion hat
    /// keine davon — sie sieht nur ihre Argumente (4.4).
    /// `tick` ist eine Konstante des Programms und ueberall lesbar; die
    /// uebrigen eingebauten Bezeichner kennt nur die Maschine.
    fn builtin(&self, b: takt_mir::expr::Builtin, p: &Program, _m: &mut Module) -> Option<Lowered> {
        match b {
            takt_mir::expr::Builtin::Tick => Some(Lowered { value: p.config.tick.to_string(), ty: LlvmType::Int(64) }),
            _ => None,
        }
    }

    /// Liest ein Feld des Abbild-Eintrags eines Channels (3.5).
    ///
    /// `.valid`, `.suspect`, `.stale`, `.age` und `.reason` lesen die
    /// Qualitaet neben dem Wert; `crate::image` beschreibt den Aufbau.
    fn quality(&self, _channel: takt_mir::ChannelId, _slot: crate::image::Slot, _m: &mut Module) -> Option<Lowered> {
        None
    }

    /// `v.done` / `v.result` eines Job-Handles (4.5): nur die Maschine
    /// liest sie, aus ihrem Slot im Abbild.
    fn job(&self, _handle: takt_mir::VarId, _field: JobField, _p: &Program, _m: &mut Module) -> Option<Lowered> {
        None
    }

    /// `m.x`, `m.state` oder ein Signal einer anderen Maschine aus Ψ (7.2).
    fn published(&self, _target: takt_mir::MachineId, _field: crate::psi::Field, _m: &mut Module) -> Option<Lowered> {
        None
    }

    /// Dasselbe mit einem Index in ein Instanz-Array (5.11).
    fn published_at(
        &self,
        _first: takt_mir::MachineId,
        _field: crate::psi::Field,
        _len: u32,
        _index: &Lowered,
        _m: &mut Module,
    ) -> Option<Lowered> {
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
        // Ein Textliteral (3.9): `{ i32 len, [N x i8] }`, wie `str<N>`.
        // Es steht als Konstante in der IR — der Inhalt ist zur
        // Uebersetzungszeit bekannt, und ein Puffer im Zustand waere
        // Speicher fuer etwas, das sich nie aendert.
        ExprKind::Str(s) => text_literal(s, &want),
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
        ExprKind::Published { machine, var } => psi_read(machine, crate::psi::Field::Var(*var), p, vars, m),
        ExprKind::StateOf(machine) => psi_read(machine, crate::psi::Field::State, p, vars, m),
        ExprKind::Signal { machine, signal } => psi_read(machine, crate::psi::Field::Signal(*signal), p, vars, m),
        ExprKind::Command(id) => vars.command(*id, m).ok_or(NotYet { what: "Command" }),
        ExprKind::JobState { handle, field } => vars.job(*handle, *field, p, m).ok_or(NotYet { what: "Job-Zustand" }),
        ExprKind::Builtin(b) => vars.builtin(*b, p, m).ok_or(NotYet { what: crate::scope::builtin_name(*b) }),
        ExprKind::Armed(t) => {
            let (armed, _) = vars.trigger_slots(*t, m).ok_or(NotYet { what: "`armed` eines Triggers" })?;
            let v = m.inst(&format!("load i1, ptr {armed}"));
            Ok(Lowered { value: v.to_string(), ty: LlvmType::Int(1) })
        }
        // 12.10: `load volatile` an der Adresse — sofort, nicht umgeordnet.
        ExprKind::PortRead(id) => {
            let port = p.ports.get(id.index()).ok_or(NotYet { what: "Port" })?;
            let ptr = m.inst(&format!("inttoptr i64 {} to ptr", port.address));
            let v = m.inst(&format!("load volatile {want}, ptr {ptr}"));
            Ok(Lowered { value: v.to_string(), ty: want.clone() })
        }
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
        ExprKind::NativeCall { native, args } => native_call(*native, args, &want, p, m, vars),
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
            // 4.1: Die uebrigen Pruefungen stehen *hinter* dem Wert — sie
            // pruefen ihn. Wo M3 sie wegbeweisen konnte, steht hier kein
            // `Checked`-Knoten (plan/m4.md 4.2), und es entsteht kein
            // Zweig.
            runtime_check(kind, &inner, m, vars)?;
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
        ExprKind::MatOp { op, args } => crate::matrix::op(*op, args, &want, p, m, vars),
        ExprKind::Index2 { base, row, col } => crate::matrix::index(base, row, col, &want, p, m, vars),
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
    if which == Accessor::Sent {
        return stream_sent(base, want, m);
    }
    if which == Accessor::Peek {
        return stream_peek(base, want, p, m, vars);
    }
    if which == Accessor::Count && stream_of(base, p).is_some() {
        return stream_count(base, want, m, vars);
    }
    // `.len` braucht vier Byte; ein Vollload kostet bei `bytes<1024>` das
    // Tausendfache (FB-214).
    if which == Accessor::Len
        && let Some((ptr, ty)) = address_of(base, m, vars)
        && matches!(&ty, LlvmType::Struct(_))
    {
        let at = m.inst(&format!("getelementptr inbounds {ty}, ptr {ptr}, i32 0, i32 0"));
        let n = m.inst(&format!("load i32, ptr {at}"));
        let wide = m.inst(&format!("sext i32 {n} to {want}"));
        return Ok(Lowered { value: wide.to_string(), ty: want.clone() });
    }
    // Ein Feld an seiner Stelle wird reduziert, nicht geladen (FB-214).
    let signed = elem_signed(base.ty, p);
    if crate::reduce::reduces(which)
        && let Some((ptr, ty)) = address_of(base, m, vars)
        && matches!(ty, LlvmType::Array(..))
        && let Some(r) = crate::reduce::access(which, crate::reduce::Field::At(ptr, ty), signed, want, m)
    {
        return r;
    }
    let x = lower(base, p, m, vars)?;
    // 8.9: `[t, pre, post, rate, samples]` in fester Reihenfolge.
    if let Some(Type::Capture { .. }) = p.types.list.get(base.ty.index()) {
        if let Some(i) = capture_field(which) {
            let v = m.inst(&format!("extractvalue {} {}, {i}", x.ty, x.value));
            // `pre` und `post` liegen als `i32` im Element, `int` ist i64.
            let v = match (&want, matches!(which, Accessor::Pre | Accessor::Post)) {
                (LlvmType::Int(64), true) => m.inst(&format!("sext i32 {v} to i64")),
                _ => v,
            };
            return Ok(Lowered { value: v.to_string(), ty: want.clone() });
        }
    }
    if let Some(Type::Map { key, value, cap }) = p.types.list.get(base.ty.index()) {
        return map_access(x, (*key, *value, *cap), (which, args), want, p, m, vars);
    }
    let arg = |i: usize, m: &mut Module| -> Result<Lowered, NotYet> {
        let e = args.get(i).ok_or(NotYet { what: "Argument fehlt" })?;
        lower(e, p, m, vars)
    };
    match which {
        // `x.bit(i)` = (x >> i) & 1, als `bool`.
        // `x.wrap_u8()` (3.10): der Wert in der Breite, ohne Pruefung —
        // eine Verengung schneidet, eine Erweiterung folgt dem Vorzeichen
        // der Quelle, wie `arith::wrap` im Interpreter.
        Accessor::Wrap(w) => {
            let bits = ty::bits(w);
            let LlvmType::Int(have) = x.ty else { return Err(NotYet { what: "`wrap` auf einem Nicht-Integer" }) };
            let signed = matches!(p.types.list.get(base.ty.index()), Some(Type::Int { width, .. }) if width.signed());
            let v = match have.cmp(&bits) {
                std::cmp::Ordering::Greater => m.inst(&format!("trunc i{have} {} to i{bits}", x.value)).to_string(),
                std::cmp::Ordering::Less if signed => {
                    m.inst(&format!("sext i{have} {} to i{bits}", x.value)).to_string()
                }
                std::cmp::Ordering::Less => m.inst(&format!("zext i{have} {} to i{bits}", x.value)).to_string(),
                std::cmp::Ordering::Equal => x.value.clone(),
            };
            Ok(Lowered { value: v, ty: LlvmType::Int(bits) })
        }
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
            if let Some(channel) = channel_of(base) {
                return quality_of(channel, which, want, m, vars);
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
            let valid = match channel_of(base) {
                Some(channel) => quality_of(channel, Accessor::Valid, &LlvmType::Int(1), m, vars)?,
                // Auf einem `T?`/`T!E` ist es das Flag des Wrappers (3.8).
                None => {
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
            let value_of = match (&base.kind, &x.ty) {
                (ExprKind::Input { .. }, _) => x.clone(),
                (_, LlvmType::Struct(_)) => {
                    let v = m.inst(&format!("extractvalue {} {}, 0", x.ty, x.value));
                    Lowered { value: v.to_string(), ty: want.clone() }
                }
                _ => x.clone(),
            };
            let r = m.inst(&format!(
                "select i1 {}, {} {}, {} {}",
                valid.value, value_of.ty, value_of.value, fallback.ty, fallback.value
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
        // Reduktionen ueber ein Feld (8.9): `min`, `max`, `mean`, `rms`,
        // `count`, `last`. Sie stehen in `reduce`, weil sie zusammen
        // gehoeren und eine gemeinsame Zusage tragen — die Reihenfolge
        // der Summation ist dort Semantik (4.2).
        _ => match crate::reduce::access(which, crate::reduce::Field::Of(x), signed, want, m) {
            Some(r) => r,
            None => Err(NotYet { what: crate::scope::accessor_name(which) }),
        },
    }
}

/// Der Channel hinter einem Ausdruck, sofern es einer ist.
///
/// Ein Channel-Array traegt *eine* Abtastung fuer alle Elemente (8.1):
/// `tcs[i].valid` fragt nach der Qualitaet der Lieferung, nicht nach der
/// des i-ten Werts — die Karte liefert alle Kanaele zugleich oder
/// keinen. Der Index waehlt darum nur den Wert, und die Qualitaet steht
/// am Channel. Der Interpreter loest es in `sample_of` ebenso.
fn channel_of(e: &Expr) -> Option<takt_mir::ChannelId> {
    match &e.kind {
        ExprKind::Input { channel, .. } => Some(*channel),
        ExprKind::Checked { expr, .. } => channel_of(expr),
        ExprKind::Index { base, .. } => channel_of(base),
        _ => None,
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
        Intrinsic::Abs => {
            let x = a(0)?;
            m.needs_intrinsic(&format!("{} @llvm.fabs.{}({})", x.ty, x.ty, x.ty));
            m.inst(&format!("call {} @llvm.fabs.{}({} {})", x.ty, x.ty, x.ty, x.value))
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
        // Ganzzahl ohne Einheit, wie im Interpreter: `f.round()` (halb weg
        // von null, `llvm.round`) und ausserhalb von i64 ein `RangeFault`.
        Intrinsic::Round | Intrinsic::Floor | Intrinsic::Ceil => {
            let x = a(0)?;
            let name = match op {
                Intrinsic::Round => "round",
                Intrinsic::Floor => "floor",
                _ => "ceil",
            };
            m.needs_intrinsic(&format!("{} @llvm.{name}.{}({})", x.ty, x.ty, x.ty));
            let r = m.inst(&format!("call {} @llvm.{name}.{}({} {})", x.ty, x.ty, x.ty, x.value));
            let Some(target) = vars.fault_label() else {
                return Err(NotYet { what: "Rundung ohne Fault-Pfad" });
            };
            let lo = float_literal(i64::MIN as f64, &x.ty);
            let hi = float_literal(i64::MAX as f64, &x.ty);
            let ge = m.inst(&format!("fcmp oge {} {r}, {lo}", x.ty));
            let lt = m.inst(&format!("fcmp olt {} {r}, {hi}", x.ty));
            let ok = m.inst(&format!("and i1 {ge}, {lt}"));
            let go_on = format!("gerundet_{}", m.next_label());
            m.void_inst(&format!("br i1 {ok}, label %{go_on}, label %{target}"));
            m.label(&go_on);
            m.inst(&format!("fptosi {} {r} to i64", x.ty))
        }
        _ => return Err(NotYet { what: op.name() }),
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
    let (tmp, bty) = match address_of(bytes, m, vars) {
        Some(at) => at,
        None => {
            let b = lower(bytes, p, m, vars)?;
            let t = m.alloca(&b.ty);
            m.write(&b.ty, &b.value, &t.to_string());
            (t, b.ty)
        }
    };
    let LlvmType::Struct(_) = &bty else { return Err(NotYet { what: "`decode` auf einer Nicht-Sammlung" }) };
    // Die Laenge steht im Kopf der Sammlung (3.9), die Daten dahinter.
    let len_ptr = m.inst(&format!("getelementptr inbounds {bty}, ptr {tmp}, i32 0, i32 0"));
    let len = m.inst(&format!("load i32, ptr {len_ptr}"));
    let data = m.inst(&format!("getelementptr inbounds {bty}, ptr {tmp}, i32 0, i32 1"));
    let label = m.next_label();
    crate::wire::decode(data, &len.to_string(), record, want, p, m, label)
}

/// Prueft das Fault-Flag nach einem Aufruf (4.1).
///
/// Wer keinen Fault-Pfad hat — eine Funktion, die eine andere ruft —
/// reicht ihn weiter: Ihr eigenes Ziel ist der Ausgang, der das Flag
/// setzt, und es steht bereits.
fn propagate_fault(m: &mut Module, vars: &dyn Vars) -> Result<(), NotYet> {
    let Some(target) = vars.fault_label() else {
        return Err(NotYet { what: "Aufruf ohne Fault-Pfad" });
    };
    let flag = m.inst(&format!("load i8, ptr @{}", crate::abi::Abi::FAULT_FLAG));
    let ok = m.inst(&format!("icmp eq i8 {flag}, 0"));
    let go_on = format!("nach_aufruf{}", m.next_label());
    m.void_inst(&format!("br i1 {ok}, label %{go_on}, label %{target}"));
    m.label(&go_on);
    Ok(())
}

/// Die Laufzeitpruefungen aus 4.1.
///
/// **Sie stehen hinter dem Wert, nicht davor.** Ein Ueberlauf ist erst am
/// Ergebnis zu erkennen, eine Range-Verletzung auch. Nur die
/// Gueltigkeitspruefung (3.5) geht voran, weil der Wert eines `Bad`-Kanals
/// gar nicht erst gelesen werden darf.
///
/// **Was hier fehlt, hat M3 wegbewiesen.** Die Intervallanalyse setzt
/// einen `Checked`-Knoten nur, wo sie die Schranke nicht zeigen konnte
/// (3.4) — der Codegen erzeugt also genau die Zweige, die noetig sind,
/// und keinen mehr. Das ist die Auszahlung der Reihenfolge M3 → M4.
fn runtime_check(
    kind: &takt_mir::expr::CheckedKind,
    value: &Lowered,
    m: &mut Module,
    vars: &dyn Vars,
) -> Result<(), NotYet> {
    use takt_mir::expr::CheckedKind as K;
    let condition = match kind {
        // Die Range steht am Knoten; beide Grenzen einschliesslich (3.4).
        K::Range(r) => match &value.ty {
            LlvmType::Int(bits) => {
                let (Some(lo), Some(hi)) = (const_i64(&r.lo), const_i64(&r.hi)) else {
                    return Ok(());
                };
                // Eine Grenze, die in die Breite des Werts nicht passt,
                // ist keine: `slt i8 x, 255` vergleicht gegen -1, und die
                // Pruefung schluege immer fehl. Der Wert *kann* sie dann
                // nicht verletzen — die Analyse hat die Schranke schon im
                // Typ (3.4), und ein Zweig waere toter Code.
                let fits = |v: i64| {
                    let b = i64::from(*bits);
                    b >= 64 || (v >= -(1i64 << (b - 1)) && v < (1i64 << (b - 1)))
                };
                if !fits(lo) || !fits(hi) {
                    return Ok(());
                }
                m.void_inst(&format!("; Range {lo}..{hi} auf {}", value.ty));
                let a = m.inst(&format!("icmp sge {} {}, {lo}", value.ty, value.value));
                let b = m.inst(&format!("icmp sle {} {}, {hi}", value.ty, value.value));
                m.inst(&format!("and i1 {a}, {b}")).to_string()
            }
            // Die Grenzen stehen als Bitmuster, nicht dezimal: `0.1`
            // dezimal waere eine andere Zahl als die im Programm (4.2),
            // und eine Grenze, die um ein Bit danebenliegt, laesst genau
            // den Wert durch, den sie fangen soll.
            LlvmType::F32 | LlvmType::F64 => {
                let (Some(lo), Some(hi)) = (const_f64(&r.lo), const_f64(&r.hi)) else {
                    return Ok(());
                };
                let (l, h) = (float_literal(lo, &value.ty), float_literal(hi, &value.ty));
                let a = m.inst(&format!("fcmp oge {} {}, {l}", value.ty, value.value));
                let b = m.inst(&format!("fcmp ole {} {}, {h}", value.ty, value.value));
                m.inst(&format!("and i1 {a}, {b}")).to_string()
            }
            _ => return Ok(()),
        },
        // 4.1: Ein Index ausserhalb `0..len-1` ist ein `RangeFault`. Der
        // Knoten umschliesst aber den *Zugriff*, nicht den Index — der
        // geprueft Wert waere hier das Element. Die Pruefung gehoert
        // darum in `index_of`, wo der Index vorliegt; hier stuende sie auf
        // dem falschen Wert.
        //
        // Bei einer Sammlung ist die Schranke ausserdem die *Laenge* zur
        // Laufzeit, nicht die Kapazitaet (3.9): Das Sema traegt dann 0 ein
        // und meint „lies sie aus dem Wert".
        K::Index { .. } => return Ok(()),
        // Ein Divisor von null ist ein `ArithmeticFault` (4.1). Geprueft
        // wird der *Divisor*; die MIR setzt den Knoten um ihn.
        K::DivZero => m.inst(&format!("icmp ne {} {}, 0", value.ty, value.value)).to_string(),
        // Ueberlauf, Domaene und Konversion brauchen den Operator, den der
        // Knoten nicht nennt — sie kommen mit dem Kostenmodell, das sie
        // ohnehin braucht. Bis dahin steht hier kein Zweig, und das ist
        // sichtbar: Der Knoten wird gemeldet, nicht uebergangen.
        K::Overflow | K::NonFinite | K::Domain | K::Convert | K::Shift => return Ok(()),
        // `Valid` steht vor dem Wert (siehe oben), `Missing` ist das
        // Auspacken (3.8).
        K::Valid | K::Missing => return Ok(()),
    };
    let Some(target) = vars.fault_label() else {
        return Err(NotYet { what: "Laufzeitpruefung ohne Fault-Pfad" });
    };
    let go_on = format!("geprueft_{}_{}", kind_name(kind), m.next_label());
    m.void_inst(&format!("br i1 {condition}, label %{go_on}, label %{target}"));
    m.label(&go_on);
    Ok(())
}

/// Der Name einer Pruefungsart, fuer die Marke.
fn kind_name(k: &takt_mir::expr::CheckedKind) -> &'static str {
    use takt_mir::expr::CheckedKind as K;
    match k {
        K::Range(_) => "range",
        K::Index { .. } => "index",
        K::DivZero => "div",
        K::Overflow => "ovf",
        K::NonFinite => "fin",
        K::Domain => "dom",
        K::Convert => "conv",
        K::Shift => "shift",
        K::Valid => "valid",
        K::Missing => "missing",
    }
}

/// Eine Grenze als Fliesskommazahl.
fn const_f64(c: &takt_mir::types::Const) -> Option<f64> {
    match c {
        takt_mir::types::Const::Int(i) => Some(*i as f64),
        takt_mir::types::Const::Duration(d) => Some(*d as f64),
        takt_mir::types::Const::Float(f) => Some(*f),
        takt_mir::types::Const::Bool(_) => None,
    }
}

/// Eine Grenze als ganze Zahl.
fn const_i64(c: &takt_mir::types::Const) -> Option<i64> {
    match c {
        takt_mir::types::Const::Int(i) => Some(*i),
        takt_mir::types::Const::Duration(d) => Some(*d),
        takt_mir::types::Const::Float(f) => Some(*f as i64),
        takt_mir::types::Const::Bool(_) => None,
    }
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
    let go_on = format!("gueltig{}", m.next_label());
    m.void_inst(&format!("br i1 {ok}, label %{go_on}, label %{target}"));
    m.label(&go_on);
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
    let out = m.alloca(want);
    slice_into(base, from, to, want, &out.to_string(), p, m, vars)?;
    let v = m.inst(&format!("load {want}, ptr {out}"));
    Ok(Lowered { value: v.to_string(), ty: want.clone() })
}

/// Ein Ausdruck, der sein Aggregat direkt an `dst` schreibt, wo er eine
/// Form dafuer hat (FB-214). `false`: der Aufrufer nimmt den Wert.
///
/// `target` ist die Stelle hinter `dst`, wenn sie eine Variable oder ein
/// Output ist: Was sie liest, darf nicht stueckweise hineinschreiben.
pub(crate) fn lower_into(
    e: &Expr,
    dst: &str,
    target: Option<&Place>,
    p: &Program,
    m: &mut Module,
    vars: &dyn Vars,
) -> Result<bool, NotYet> {
    match &e.kind {
        ExprKind::Call { callee, args } => call_into(*callee, args, dst, target, p, m, vars),
        ExprKind::Slice { base, from, to } => {
            let want = ty::lower(e.ty, p).ok_or(NotYet { what: "Teilbereich" })?;
            slice_into(base, from, to, &want, dst, p, m, vars)?;
            Ok(true)
        }
        // Ein Literal, das sein Ziel liest, bleibt ein Wert: Das zweite
        // Element laese sonst das erste schon ueberschrieben.
        ExprKind::Record { fields, .. } | ExprKind::Array(fields) => {
            let want = ty::lower(e.ty, p).ok_or(NotYet { what: "Literal" })?;
            if !want.indirect() || target.is_some_and(|t| reads(e, t)) {
                return Ok(false);
            }
            literal_into(fields, &want, dst, p, m, vars)
        }
        _ => Ok(false),
    }
}

/// Ein Literal an seine Stelle, Element fuer Element (FB-214).
fn literal_into(
    fields: &[Expr],
    want: &LlvmType,
    dst: &str,
    p: &Program,
    m: &mut Module,
    vars: &dyn Vars,
) -> Result<bool, NotYet> {
    let n = match want {
        LlvmType::Struct(types) => types.len(),
        LlvmType::Array(_, len) => *len as usize,
        _ => return Ok(false),
    };
    if n != fields.len() {
        return Err(NotYet { what: "Literal mit anderer Elementzahl" });
    }
    for (i, f) in fields.iter().enumerate() {
        let at = m.inst(&format!("getelementptr inbounds {want}, ptr {dst}, i32 0, i32 {i}"));
        store(f, &at.to_string(), None, p, m, vars)?;
    }
    Ok(true)
}

/// Schreibt `e` an `dst`: unmittelbar, als Kopie von seiner Stelle oder
/// als Wert — in dieser Reihenfolge (FB-214).
pub(crate) fn store(
    e: &Expr,
    dst: &str,
    target: Option<&Place>,
    p: &Program,
    m: &mut Module,
    vars: &dyn Vars,
) -> Result<(), NotYet> {
    if lower_into(e, dst, target, p, m, vars)? {
        return Ok(());
    }
    if let Some(want) = ty::lower(e.ty, p).filter(LlvmType::indirect)
        && let Some((src, _)) = address_of(e, m, vars)
    {
        m.copy(&want, &src.to_string(), dst);
        return Ok(());
    }
    let v = lower(e, p, m, vars)?;
    m.write(&v.ty, &v.value, dst);
    Ok(())
}

/// Die Wurzel einer Stelle: die Variable oder der Output darunter.
pub(crate) fn root(place: &Place) -> &Place {
    match place {
        Place::Field(base, _) | Place::Index(base, _) | Place::Index2(base, _, _) => root(base),
        _ => place,
    }
}

/// Ob `e` die Wurzel von `target` liest, auch in Teilausdruecken.
pub(crate) fn reads(e: &Expr, target: &Place) -> bool {
    let hit = match (&e.kind, root(target)) {
        (ExprKind::Var(v), Place::Var(w)) => v == w,
        (ExprKind::Output(c), Place::Output(d)) => c == d,
        _ => false,
    };
    hit || e.children().into_iter().any(|c| reads(c, target))
}

#[allow(clippy::too_many_arguments)]
fn slice_into(
    base: &Expr,
    from: &Expr,
    to: &Expr,
    want: &LlvmType,
    out: &str,
    p: &Program,
    m: &mut Module,
    vars: &dyn Vars,
) -> Result<(), NotYet> {
    // Die Quelle wird adressiert, wo sie eine Stelle ist; nur ein
    // gerechneter Wert braucht einen Slot (FB-214).
    let (src_ptr, xty) = match address_of(base, m, vars) {
        Some(at) => at,
        None => {
            let x = lower(base, p, m, vars)?;
            let t = m.alloca(&x.ty);
            m.write(&x.ty, &x.value, &t.to_string());
            (t, x.ty)
        }
    };
    let a = lower(from, p, m, vars)?;
    let b = lower(to, p, m, vars)?;
    let src = crate::collection::layout_of(&xty).ok_or(NotYet { what: "Teilbereich einer Nicht-Sammlung" })?;
    let dst = crate::collection::layout_of(want).ok_or(NotYet { what: "Teilbereich ohne Zielsammlung" })?;
    let len = m.inst(&format!("sub {} {}, {}", a.ty, b.value, a.value));
    let len32 = m.inst(&format!("trunc {} {len} to i32", a.ty));
    let len_ptr = m.inst(&format!("getelementptr inbounds {want}, ptr {out}, i32 0, i32 0"));
    m.void_inst(&format!("store i32 {len32}, ptr {len_ptr}"));
    let src_data = m.inst(&format!("getelementptr inbounds {xty}, ptr {src_ptr}, i32 0, i32 1"));
    let at = m.inst(&format!(
        "getelementptr inbounds [{} x {}], ptr {src_data}, i32 0, {} {}",
        src.cap, src.elem, a.ty, a.value
    ));
    let dst_data = m.inst(&format!("getelementptr inbounds {want}, ptr {out}, i32 0, i32 1"));
    let bytes = m.inst(&format!("mul {} {len}, {}", a.ty, dst.elem.aligned_size().max(1)));
    // `memmove`: `xs = xs[1..]` ueberlappt.
    m.void_inst(&format!("call void @llvm.memmove.p0.p0.i64(ptr {dst_data}, ptr {at}, i64 {bytes}, i1 false)"));
    Ok(())
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
    // Eine Stelle wird adressiert; nur ein gerechneter Wert braucht den
    // Umweg ueber einen Scratch (FB-214).
    let place = address_of(base, m, vars);
    let x_ty = match &place {
        Some((_, ty)) => ty.clone(),
        None => ty::lower(base.ty, p).ok_or(NotYet { what: "Index auf diesem Typ" })?,
    };
    let x = match &place {
        Some(_) => None,
        None => Some(lower(base, p, m, vars)?),
    };
    let i = lower(index, p, m, vars)?;
    // 4.1: Der Index liegt in `0..len-1`. Die Schranke ist bei einer
    // Sammlung ihre *Laenge* zur Laufzeit (3.9), bei einem Array seine
    // statische Groesse.
    //
    // Geprueft wird immer, auch wo M3 die Schranke gezeigt hat: Der
    // `Checked{Index}`-Knoten umschliesst den *Zugriff*, nicht den Index,
    // und `index_of` sieht ihn darum nicht. Das kostet einen Zweig, den
    // LLVM meist wegoptimiert — und es ist die konservative Seite. Die
    // Verbindung herzustellen hiesse, den Knoten an den Index zu haengen;
    // das gehoert in die MIR, nicht in den Codegen.
    {
        let grenze = match (&x_ty, &place, &x) {
            (LlvmType::Struct(_), Some((ptr, ty)), _) => {
                let at = m.inst(&format!("getelementptr inbounds {ty}, ptr {ptr}, i32 0, i32 0"));
                let l = m.inst(&format!("load i32, ptr {at}"));
                m.inst(&format!("sext i32 {l} to {}", i.ty)).to_string()
            }
            (LlvmType::Struct(_), None, Some(x)) => {
                let l = m.inst(&format!("extractvalue {} {}, 0", x.ty, x.value));
                m.inst(&format!("sext i32 {l} to {}", i.ty)).to_string()
            }
            (LlvmType::Array(_, n), _, _) => n.to_string(),
            _ => return Err(NotYet { what: "Index auf diesem Typ" }),
        };
        if let Some(target) = vars.fault_label() {
            // Ein `icmp ult` faengt beide Enden: Ein negativer Index ist
            // vorzeichenlos gelesen groesser als jede Laenge. Das spart
            // zwei der drei Instruktionen, die `sge`+`slt`+`and` kostete.
            let ok = m.inst(&format!("icmp ult {} {}, {grenze}", i.ty, i.value));
            let go_on = format!("index_ok{}", m.next_label());
            m.void_inst(&format!("br i1 {ok}, label %{go_on}, label %{target}"));
            m.label(&go_on);
        }
    }
    let base_ptr = match (&place, &x) {
        (Some((ptr, _)), _) => *ptr,
        // Ein gerechneter Wert liegt als Register vor, nicht im Speicher;
        // ein `extractvalue` mit berechnetem Index gibt es nicht. Er
        // bekommt darum einen Platz (11.2).
        (None, Some(x)) => {
            let tmp = m.alloca(&x.ty);
            m.write(&x.ty, &x.value, &tmp.to_string());
            tmp
        }
        _ => return Err(NotYet { what: "Index auf diesem Typ" }),
    };
    let (array_ty, data) = match &x_ty {
        LlvmType::Struct(_) => {
            let l = crate::collection::layout_of(&x_ty).ok_or(NotYet { what: "Index auf diesem Struct" })?;
            let d = m.inst(&format!("getelementptr inbounds {x_ty}, ptr {base_ptr}, i32 0, i32 1"));
            (LlvmType::Array(Box::new(l.elem), l.cap), d)
        }
        LlvmType::Array(..) => (x_ty.clone(), base_ptr),
        _ => return Err(NotYet { what: "Index auf diesem Typ" }),
    };
    let at = m.inst(&format!("getelementptr inbounds {array_ty}, ptr {data}, i32 0, {} {}", i.ty, i.value));
    let v = m.inst(&format!("load {want}, ptr {at}"));
    Ok(Lowered { value: v.to_string(), ty: want.clone() })
}

/// Ein Aufruf einer nativen Funktion (4.5).
///
/// Sie liegt als Symbol in der Runtime, nicht im erzeugten Code: Ihre
/// Implementierung gehoert zur TCB (9.5) und ist in Rust geschrieben. Der
/// Aufruf ist darum eine `declare` plus `call` — dieselbe Form wie die
/// Runtime-ABI.
///
/// **Sie kann nicht faulten.** 4.5 verlangt `total`: keine Panics,
/// Terminierung, bitreproduzierbare Ergebnisse. Ein Zweig wie bei den
/// reinen Funktionen (FB-87) waere hier falsch — er behauptete eine
/// Moeglichkeit, die der Vertrag ausschliesst.
fn native_call(
    native: takt_mir::NativeId,
    args: &[Expr],
    want: &LlvmType,
    p: &Program,
    m: &mut Module,
    vars: &dyn Vars,
) -> Result<Lowered, NotYet> {
    let n = p.natives.get(native.index()).ok_or(NotYet { what: "native Funktion" })?;
    let mut ops = Vec::with_capacity(args.len() + 1);
    let mut sig = Vec::with_capacity(args.len() + 1);
    for a in args {
        let vty = ty::lower(a.ty, p).ok_or(NotYet { what: "Argumenttyp" })?;
        match p.types.list.get(a.ty.index()) {
            // Ein Byteblock geht als Zeiger und Laenge; sein Wert waere eine
            // Kopie von bis zu mehreren KiB je Aufruf.
            Some(Type::Bytes { .. }) => {
                let tmp = m.alloca(&vty);
                match address_of(a, m, vars) {
                    Some((src, _)) => m.copy(&vty, &src.to_string(), &tmp.to_string()),
                    None => {
                        let v = lower(a, p, m, vars)?;
                        m.write(&v.ty, &v.value, &tmp.to_string());
                    }
                }
                let len_ptr = m.inst(&format!("getelementptr inbounds {vty}, ptr {tmp}, i32 0, i32 0"));
                let len = m.inst(&format!("load i32, ptr {len_ptr}"));
                let data = m.inst(&format!("getelementptr inbounds {vty}, ptr {tmp}, i32 0, i32 1"));
                ops.push(format!("ptr {data}"));
                ops.push(format!("i32 {len}"));
                sig.push("ptr".to_string());
                sig.push("i32".to_string());
            }
            // Ein Record geht in kanonischer Byteform (5.9): die TCB kennt
            // kein Ziel-Layout.
            Some(Type::Record(_)) => {
                let v = lower(a, p, m, vars)?;
                let tmp = m.alloca(&v.ty);
                m.write(&v.ty, &v.value, &tmp.to_string());
                let buf = canonical_buffer(p, a.ty, m)?;
                let len = crate::persist::encode_canonical(p, a.ty, tmp, buf, m)?;
                let len32 = m.inst(&format!("trunc i64 {len} to i32"));
                ops.push(format!("ptr {buf}"));
                ops.push(format!("i32 {len32}"));
                sig.push("ptr".to_string());
                sig.push("i32".to_string());
            }
            _ => {
                let v = lower(a, p, m, vars)?;
                ops.push(format!("{} {}", v.ty, v.value));
                sig.push(v.ty.to_string());
            }
        }
    }
    let symbol = format!("takt_native_{}", crate::fns::sanitized(&n.name));
    // Ein Skalar kommt als Wert zurueck; einen Byteblock oder ein Record
    // schreibt die Funktion in kanonischer Form in einen Puffer des
    // Aufrufers.
    match p.types.list.get(n.ret.index()) {
        Some(Type::Bytes { .. } | Type::Record(_)) => {
            let buf = canonical_buffer(p, n.ret, m)?;
            ops.push(format!("ptr {buf}"));
            sig.push("ptr".to_string());
            m.needs_intrinsic(&format!("void @{symbol}({})", sig.join(", ")));
            m.void_inst(&format!("call void @{symbol}({})", ops.join(", ")));
            let dst = m.alloca(want);
            crate::persist::decode_canonical(p, n.ret, buf, dst, m)?;
            let v = m.inst(&format!("load {want}, ptr {dst}"));
            Ok(Lowered { value: v.to_string(), ty: want.clone() })
        }
        _ => {
            m.needs_intrinsic(&format!("{want} @{symbol}({})", sig.join(", ")));
            let r = m.inst(&format!("call {want} @{symbol}({})", ops.join(", ")));
            Ok(Lowered { value: r.to_string(), ty: want.clone() })
        }
    }
}

/// Ein Puffer fuer die kanonische Form eines Typs, in seiner oberen
/// Schranke (`bytes::max_size`).
fn canonical_buffer(p: &Program, ty: TypeId, m: &mut Module) -> Result<crate::emit::Reg, NotYet> {
    let cap = takt_mir::bytes::max_size(p, ty).map_err(|_| NotYet { what: "Typ ohne Byteform" })?;
    Ok(m.inst(&format!("alloca [{cap} x i8]")))
}

/// `o.sent` (8.8): der Treiber schreibt den zuletzt abgeholten Ausschnitt
/// in den Puffer des Aufrufers; leer heisst `none`.
fn stream_sent(base: &Expr, want: &LlvmType, m: &mut Module) -> Result<Lowered, NotYet> {
    let ExprKind::Input { channel: c, .. } = base.kind else { return Err(NotYet { what: "`sent` ohne Ausgabestrom" }) };
    let LlvmType::Struct(fields) = want else { return Err(NotYet { what: "`sent` ohne Wrapper-Typ" }) };
    let inner = fields.first().ok_or(NotYet { what: "Wrapper ohne Wert" })?.clone();
    let buf = m.alloca(&inner);
    m.write(&inner, "zeroinitializer", &buf.to_string());
    let n = m.inst(&format!("call i32 @{}(i32 {}, ptr {buf})", crate::stream::Streams::SENT, c.0));
    let v = m.inst(&format!("load {inner}, ptr {buf}"));
    let some = m.inst(&format!("icmp sgt i32 {n}, 0"));
    let with_value = m.inst(&format!("insertvalue {want} undef, {inner} {v}, 0"));
    let r = m.inst(&format!("insertvalue {want} {with_value}, i1 {some}, 1"));
    Ok(Lowered { value: r.to_string(), ty: want.clone() })
}

/// Der Strom hinter einem Ausdruck: ein Channel mit Stromtyp oder ein
/// interner Strom; `None` fuer jeden anderen Wert (etwa `samples`).
fn stream_of(base: &Expr, p: &Program) -> Option<takt_mir::expr::StreamRef> {
    match &base.kind {
        ExprKind::Input { channel, .. } => {
            let ty = p.channels.get(channel.index())?.ty;
            matches!(p.types.list.get(ty.index()), Some(Type::Stream(_)))
                .then_some(takt_mir::expr::StreamRef::Channel(*channel))
        }
        ExprKind::Stream(s) => Some(takt_mir::expr::StreamRef::Internal(*s)),
        _ => None,
    }
}

/// `s.count` (8.6): wie viele Elemente das Fenster dieser Maschine hat.
fn stream_count(base: &Expr, want: &LlvmType, m: &mut Module, vars: &dyn Vars) -> Result<Lowered, NotYet> {
    let stream = match &base.kind {
        ExprKind::Input { channel, .. } => takt_mir::expr::StreamRef::Channel(*channel),
        ExprKind::Stream(s) => takt_mir::expr::StreamRef::Internal(*s),
        _ => return Err(NotYet { what: "`count` ohne festen Strom" }),
    };
    let sid = crate::stream::number(stream).ok_or(NotYet { what: "Strom ohne feste Nummer" })?;
    let (cur_ptr, _) = vars.stream_slots(stream, m).ok_or(NotYet { what: "Cursor eines Stroms" })?;
    let cur = m.inst(&format!("load i64, ptr {cur_ptr}"));
    let n = m.inst(&format!("call i32 @{}(i32 {sid}, i64 {cur})", crate::stream::Streams::COUNT));
    let wide = m.inst(&format!("sext i32 {n} to {want}"));
    Ok(Lowered { value: wide.to_string(), ty: want.clone() })
}

/// `s.peek()` (8.6, FB-15): das naechste Element als `E?`. Es gilt als
/// untersucht; der Cursor rueckt am Ende des Schritts (Lemma 9.6.1).
fn stream_peek(base: &Expr, want: &LlvmType, p: &Program, m: &mut Module, vars: &dyn Vars) -> Result<Lowered, NotYet> {
    let stream = match &base.kind {
        ExprKind::Input { channel, .. } => takt_mir::expr::StreamRef::Channel(*channel),
        ExprKind::Stream(s) => takt_mir::expr::StreamRef::Internal(*s),
        _ => return Err(NotYet { what: "`peek` ohne festen Strom" }),
    };
    let elem = crate::stream::element(p, stream).ok_or(NotYet { what: "Elementtyp eines Stroms" })?;
    let sid = crate::stream::number(stream).ok_or(NotYet { what: "Strom ohne feste Nummer" })?;
    let LlvmType::Struct(fields) = want else { return Err(NotYet { what: "`peek` ohne Wrapper-Typ" }) };
    let inner = fields.first().ok_or(NotYet { what: "Wrapper ohne Wert" })?.clone();
    let (cur_ptr, ex_ptr) = vars.stream_slots(stream, m).ok_or(NotYet { what: "Cursor eines Stroms" })?;
    let mi = vars.machine_index().ok_or(NotYet { what: "`peek` ausserhalb einer Maschine" })?;
    let out = m.alloca(&inner);
    m.write(&inner, "zeroinitializer", &out.to_string());
    let buf = crate::stream::scratch(p, elem, m)?;
    let cur = m.inst(&format!("load i64, ptr {cur_ptr}"));
    let n = m.inst(&format!("call i32 @{}(i32 {sid}, i64 {cur})", crate::stream::Streams::COUNT));
    let some = m.inst(&format!("icmp sgt i32 {n}, 0"));
    let k = m.next_label();
    let (read, done) = (format!("peek{k}_lesen"), format!("peek{k}_fertig"));
    m.void_inst(&format!("br i1 {some}, label %{read}, label %{done}"));
    m.label(&read);
    let seq = m.inst(&format!("call i64 @{}(i32 {sid}, i64 {cur}, i32 0, ptr {buf})", crate::stream::Streams::AT));
    m.void_inst(&format!("call void @{}(i32 {sid}, i32 {mi}, i64 {seq})", crate::stream::Streams::EXAMINED));
    crate::stream::note_examined(ex_ptr, seq, m);
    crate::stream::copy_payload(buf, out, elem, p, m)?;
    m.void_inst(&format!("br label %{done}"));
    m.label(&done);
    let v = m.inst(&format!("load {inner}, ptr {out}"));
    let with_value = m.inst(&format!("insertvalue {want} undef, {inner} {v}, 0"));
    let r = m.inst(&format!("insertvalue {want} {with_value}, i1 {some}, 1"));
    Ok(Lowered { value: r.to_string(), ty: want.clone() })
}

/// `m.len` und `m.get(k)` einer `map` (3.9) ueber `takt_native_map_*`:
/// Der Wert der Map wird abgelegt, die Native sondiert ueber den Slots.
fn map_access(
    x: Lowered,
    (key, value, cap): (TypeId, TypeId, u32),
    (which, args): (Accessor, &[Expr]),
    want: &LlvmType,
    p: &Program,
    m: &mut Module,
    vars: &dyn Vars,
) -> Result<Lowered, NotYet> {
    let (klen, vlen) = crate::persist::map_widths(p, key, value)?;
    let slots = m.alloca(&x.ty);
    m.write(&x.ty, &x.value, &slots.to_string());
    match which {
        Accessor::Len => {
            m.needs_intrinsic("i32 @takt_native_map_len(ptr, i32, i32, i32)");
            let n = m.inst(&format!("call i32 @takt_native_map_len(ptr {slots}, i32 {cap}, i32 {klen}, i32 {vlen})"));
            let wide = m.inst(&format!("sext i32 {n} to {want}"));
            Ok(Lowered { value: wide.to_string(), ty: want.clone() })
        }
        Accessor::Get => {
            let k = args.first().ok_or(NotYet { what: "`get` ohne Schluessel" })?;
            let kbuf = crate::persist::encode_padded(k, klen, p, m, vars)?;
            let out = m.inst(&format!("alloca [{vlen} x i8]"));
            m.write(&LlvmType::Array(Box::new(LlvmType::Int(8)), vlen), "zeroinitializer", &out.to_string());
            m.needs_intrinsic("i1 @takt_native_map_get(ptr, i32, i32, i32, ptr, ptr)");
            let hit = m.inst(&format!(
                "call i1 @takt_native_map_get(ptr {slots}, i32 {cap}, i32 {klen}, i32 {vlen}, ptr {kbuf}, ptr {out})"
            ));
            // `V?` wie `wrap` es baut: Wert, dann das Flag.
            let LlvmType::Struct(fields) = want else { return Err(NotYet { what: "`get` ohne Wrapper-Typ" }) };
            let inner = fields.first().ok_or(NotYet { what: "Wrapper ohne Wert" })?.clone();
            let dst = m.alloca(&inner);
            m.write(&inner, "zeroinitializer", &dst.to_string());
            crate::persist::decode_canonical(p, value, out, dst, m)?;
            let v = m.inst(&format!("load {inner}, ptr {dst}"));
            let with_value = m.inst(&format!("insertvalue {want} undef, {inner} {v}, 0"));
            let r = m.inst(&format!("insertvalue {want} {with_value}, i1 {hit}, 1"));
            Ok(Lowered { value: r.to_string(), ty: want.clone() })
        }
        _ => Err(NotYet { what: "Zugriff auf eine `map`" }),
    }
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
    let sig = crate::fns::signature(f, p).ok_or(NotYet { what: "Signatur" })?;
    let out = sig.sret.then(|| m.alloca(&sig.ret).to_string());
    let value = call_with(f, &sig, args, out.as_deref(), None, p, m, vars)?;
    Ok(match (out, value) {
        (Some(out), _) => Lowered { value: m.inst(&format!("load {want}, ptr {out}")).to_string(), ty: want.clone() },
        (None, Some(r)) => Lowered { value: r.to_string(), ty: want.clone() },
        (None, None) => Lowered { value: String::new(), ty: LlvmType::Void },
    })
}

/// Ein Aufruf, dessen `sret`-Ergebnis direkt an `dst` geht (FB-214).
/// `false`: kein `sret` — der Aufrufer nimmt den Wert ueber [`lower`].
pub(crate) fn call_into(
    func: takt_mir::FnId,
    args: &[Expr],
    dst: &str,
    target: Option<&Place>,
    p: &Program,
    m: &mut Module,
    vars: &dyn Vars,
) -> Result<bool, NotYet> {
    let f = p.fns.get(func.index()).ok_or(NotYet { what: "Funktion" })?;
    let sig = crate::fns::signature(f, p).ok_or(NotYet { what: "Signatur" })?;
    if !sig.sret {
        return Ok(false);
    }
    call_with(f, &sig, args, Some(dst), target, p, m, vars)?;
    Ok(true)
}

/// Der Aufruf selbst; `out` ist der `sret`-Platz. Liefert das Register des
/// Ergebnisses, wenn die Funktion eines per Wert gibt.
#[allow(clippy::too_many_arguments)]
fn call_with(
    f: &takt_mir::fns::Fn,
    sig: &crate::fns::Signature,
    args: &[Expr],
    out: Option<&str>,
    target: Option<&Place>,
    p: &Program,
    m: &mut Module,
    vars: &dyn Vars,
) -> Result<Option<crate::emit::Reg>, NotYet> {
    let mut operands = Vec::with_capacity(args.len() + 1);
    if let Some(out) = out {
        operands.push(format!("ptr sret({}) {out}", sig.ret));
    }
    for a in args {
        // Ein grosses Argument geht als Zeiger auf seine Stelle; was der
        // Gerufene aendert, kopiert er sich (`prologue`). Einen Slot
        // brauchen nur ein gerechneter Wert und eine Stelle, die das Ziel
        // des Aufrufs liest — `sret` schriebe sonst in seine Quelle.
        let ty = ty::lower(a.ty, p);
        if let Some(ty) = ty.filter(LlvmType::indirect) {
            let place = if target.is_some_and(|t| reads(a, t)) { None } else { address_of(a, m, vars) };
            let at = match place {
                Some((src, _)) => src,
                None => {
                    let tmp = m.alloca(&ty);
                    store(a, &tmp.to_string(), None, p, m, vars)?;
                    tmp
                }
            };
            operands.push(format!("ptr {at}"));
        } else {
            let v = lower(a, p, m, vars)?;
            operands.push(format!("{} {}", v.ty, v.value));
        }
    }
    let name = crate::fns::symbol(f);
    let ret = sig.llvm_ret();
    let value = if ret == LlvmType::Void {
        m.void_inst(&format!("call void @{name}({})", operands.join(", ")));
        None
    } else {
        Some(m.inst(&format!("call {ret} @{name}({})", operands.join(", "))))
    };
    // 4.1: Eine reine Funktion faultet den Aufrufer. Sie setzt dafuer das
    // Flag; hier wird es geprueft, und der Aufrufer nimmt seinen eigenen
    // Fault-Pfad (`abi::Abi::FAULT_FLAG`).
    propagate_fault(m, vars)?;
    Ok(value)
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
    let mut vals = Vec::with_capacity(n);
    for f in fields {
        vals.push(lower(f, p, m, vars)?);
    }
    // Lauter Konstanten sind eine Konstante: LLVM legt sie ab, statt sie
    // Element fuer Element zu bauen.
    if vals.iter().all(|v| !v.value.starts_with('%')) {
        let items = vals.iter().map(|v| format!("{} {}", v.ty, v.value)).collect::<Vec<_>>().join(", ");
        let value = if matches!(want, LlvmType::Array(..)) { format!("[{items}]") } else { format!("{{ {items} }}") };
        return Ok(Lowered { value, ty: want.clone() });
    }
    let mut cur = "undef".to_string();
    for (i, v) in vals.iter().enumerate() {
        cur = m.inst(&format!("insertvalue {want} {cur}, {} {}, {i}", v.ty, v.value)).to_string();
    }
    Ok(Lowered { value: cur, ty: want.clone() })
}

/// Die Adresse eines Ausdrucks, wo er eine Stelle bezeichnet (FB-214).
///
/// Nur Variablen und Wege darin; alles andere ist ein gerechneter Wert
/// und hat keinen Ort. `None` heisst „nimm den Wert" — der Aufrufer
/// faellt dann auf den alten Weg zurueck.
pub(crate) fn address_of(e: &Expr, m: &mut Module, vars: &dyn Vars) -> Option<(crate::emit::Reg, LlvmType)> {
    match &e.kind {
        ExprKind::Var(id) => vars.address(*id, m),
        ExprKind::Field { base, field } => {
            let (ptr, ty) = address_of(base, m, vars)?;
            let LlvmType::Struct(fields) = &ty else { return None };
            let inner = fields.get(*field as usize)?.clone();
            Some((m.inst(&format!("getelementptr inbounds {ty}, ptr {ptr}, i32 0, i32 {field}")), inner))
        }
        _ => None,
    }
}

/// Ein Feldzugriff auf einen Record (3.7).
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
    // Eine Stelle wird adressiert, nicht geladen: `s.f` auf einem
    // `bytes<1024>` liest sonst 1028 Byte fuer eines (FB-214). Der
    // Schreibpfad tut das laengst (`place`), der Lesepfad jetzt auch.
    if let Some((ptr, ty)) = address_of(base, m, vars)
        && let LlvmType::Struct(fields) = &ty
        && fields.get(field as usize).is_some()
    {
        let at = m.inst(&format!("getelementptr inbounds {ty}, ptr {ptr}, i32 0, i32 {field}"));
        let v = m.inst(&format!("load {want}, ptr {at}"));
        return Ok(Lowered { value: v.to_string(), ty: want.clone() });
    }
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
        // `to` auf Ganzzahlen loest das Sema in eine Multiplikation auf (3.2);
        // `to_float` rechnet ab dem exakten Ganzzahlwert wie `to` auf Fliesskomma.
        ConvertKind::To | ConvertKind::ToFloat => {
            let to_float = matches!(kind, ConvertKind::ToFloat);
            let src = match (to_float, p.types.list.get(e.ty.index())) {
                (_, Some(Type::Float { unit: Some(u), .. })) | (true, Some(Type::Int { unit: Some(u), .. })) => {
                    Some(p.units.get(u.index()).ok_or(NotYet { what: "Einheit" })?)
                }
                (true, Some(Type::Int { unit: None, .. })) => None,
                _ => return Err(NotYet { what: "`to(U)` ohne Quelleinheit" }),
            };
            let mut cur = if to_float {
                let op = if int_is_signed_ty(e.ty, p) { "sitofp" } else { "uitofp" };
                m.inst(&format!("{op} {} {} to {want}", x.ty, x.value)).to_string()
            } else {
                x.value
            };
            let (factor, offset) =
                src.map_or((takt_mir::types::Rational::int(1), None), |u| (u.factor, u.affine_offset));
            if let Some(off) = offset {
                let o = float_literal(off.num as f64 / off.den as f64, want);
                cur = m.inst(&format!("fadd {want} {cur}, {o}")).to_string();
            }
            let num = i128::from(factor.num) * i128::from(dst.factor.den);
            let den = i128::from(factor.den) * i128::from(dst.factor.num);
            cur = m.inst(&format!("fmul {want} {cur}, {}", float_literal(num as f64, want))).to_string();
            cur = m.inst(&format!("fdiv {want} {cur}, {}", float_literal(den as f64, want))).to_string();
            if let Some(off) = dst.affine_offset {
                let o = float_literal(off.num as f64 / off.den as f64, want);
                cur = m.inst(&format!("fsub {want} {cur}, {o}")).to_string();
            }
            Ok(Lowered { value: cur, ty: want.clone() })
        }
    }
}

/// Ψ-Lesevorgang (7.2); Instanz-Arrays mit Index sind v1.2 (5.11).
fn psi_read(
    machine: &takt_mir::expr::MachineRef,
    field: crate::psi::Field,
    p: &Program,
    vars: &dyn Vars,
    m: &mut Module,
) -> Result<Lowered, NotYet> {
    let Some(index) = &machine.index else {
        return vars.published(machine.machine, field, m).ok_or(NotYet { what: "Psi" });
    };
    // 5.11: Das Array beginnt bei `machine.machine`; seine Laenge steht
    // an der Instanz.
    let len = match &p.machines.get(machine.machine.index()).map(|x| &x.kind) {
        Some(takt_mir::machine::MachineKind::Instance(info)) => info.array.map(|(_, n)| n),
        _ => None,
    }
    .ok_or(NotYet { what: "Instanz-Array ohne Laenge" })?;
    let i = lower(index, p, m, vars)?;
    vars.published_at(machine.machine, field, len, &i, m).ok_or(NotYet { what: "Psi mit Index" })
}

/// Ist der Typ eine vorzeichenbehaftete Ganzzahl?
fn elem_signed(ty: TypeId, p: &Program) -> bool {
    match p.types.list.get(ty.index()) {
        Some(Type::Array { elem, .. } | Type::Samples { elem, .. }) => int_is_signed_ty(*elem, p),
        _ => true,
    }
}

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
    if crate::matrix::shape(&a.ty).is_some() || crate::matrix::shape(&b.ty).is_some() {
        return crate::matrix::binary(op, &a, &b, want, m);
    }
    let float = a.ty.is_float();
    let signed = int_is_signed(lhs.ty, p);

    // Text vergleicht sich nach Inhalt, nicht als Bitmuster (3.9): Was
    // hinter der Laenge steht, ist bei einem Capture Rest des vorigen
    // Elements.
    if matches!(op, BinaryOp::Eq | BinaryOp::Ne) && matches!(a.ty, LlvmType::Struct(_)) && a.ty == b.ty {
        let same = text_equal(&a, &b, m)?;
        if op == BinaryOp::Eq {
            return Ok(same);
        }
        let neg = m.inst(&format!("xor i1 {}, true", same.value));
        return Ok(Lowered { value: neg.to_string(), ty: LlvmType::Int(1) });
    }

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
/// Ein Textliteral als Wert (3.9).
///
/// Der Aufbau ist der von `str<N>`: Laenge, dann die Bytes. Laenger als
/// `N` wird abgeschnitten — der Rand begrenzt, und das Sema hat es
/// geprueft.
fn text_literal(s: &str, want: &LlvmType) -> Result<Lowered, NotYet> {
    let LlvmType::Struct(fields) = want else { return Err(NotYet { what: "Textliteral ohne Texttyp" }) };
    let Some(LlvmType::Array(_, cap)) = fields.get(1) else {
        return Err(NotYet { what: "Textliteral ohne Puffer" });
    };
    let bytes = s.as_bytes();
    let n = bytes.len().min(*cap as usize);
    let mut inhalt: Vec<String> = bytes[..n].iter().map(|b| format!("i8 {b}")).collect();
    inhalt.resize(*cap as usize, "i8 0".to_string());
    // `line<N>` traegt hinter dem Puffer noch `truncated` (3.9).
    let rest = if fields.len() > 2 { ", i1 false" } else { "" };
    let value = format!("{{ i32 {n}, [{cap} x i8] [{}]{rest} }}", inhalt.join(", "));
    Ok(Lowered { value, ty: want.clone() })
}

/// Gleichheit zweier Texte (3.9).
///
/// **Byteweise, nicht als Struct.** Ein `icmp eq` auf dem ganzen Wert
/// verglich auch die Bytes hinter der Laenge, und die sind bei einem
/// Capture Reste des vorigen Elements. Der Interpreter vergleicht den
/// *Inhalt* (`Value::Str`), also tut es der Codegen auch.
fn text_equal(a: &Lowered, b: &Lowered, m: &mut Module) -> Result<Lowered, NotYet> {
    let LlvmType::Struct(fields) = &a.ty else { return Err(NotYet { what: "Textvergleich ohne Texttyp" }) };
    let Some(LlvmType::Array(_, cap)) = fields.get(1) else {
        return Err(NotYet { what: "Textvergleich ohne Puffer" });
    };
    let la = m.inst(&format!("extractvalue {} {}, 0", a.ty, a.value));
    let lb = m.inst(&format!("extractvalue {} {}, 0", b.ty, b.value));
    let mut same = m.inst(&format!("icmp eq i32 {la}, {lb}")).to_string();
    // Die Bytes bis zur Laenge; darueber hinaus zaehlt nichts. `N` ist
    // typisch klein, also abgerollt — eine Schleife braeuchte beide Werte
    // im Speicher (4.1 verlangt ohnehin eine feste Schranke).
    for i in 0..*cap {
        let x = m.inst(&format!("extractvalue {} {}, 1, {i}", a.ty, a.value));
        let y = m.inst(&format!("extractvalue {} {}, 1, {i}", b.ty, b.value));
        let eq = m.inst(&format!("icmp eq i8 {x}, {y}"));
        // Nur Positionen unterhalb der Laenge zaehlen.
        let within = m.inst(&format!("icmp slt i32 {i}, {la}"));
        let relevant = m.inst(&format!("xor i1 {within}, true"));
        let ok = m.inst(&format!("or i1 {eq}, {relevant}"));
        same = m.inst(&format!("and i1 {same}, {ok}")).to_string();
    }
    Ok(Lowered { value: same, ty: LlvmType::Int(1) })
}

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
        ExprKind::Builtin(_) => "eingebauter Bezeichner (`tick`, `last_fault`, ...)",
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

/// Das Feld eines Capture-Elements zu seinem Zugriff (8.9).
fn capture_field(which: Accessor) -> Option<u32> {
    Some(match which {
        Accessor::T => 0,
        Accessor::Pre => 1,
        Accessor::Post => 2,
        Accessor::Rate => 3,
        Accessor::Samples => 4,
        _ => return None,
    })
}
