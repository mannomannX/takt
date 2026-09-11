//! Aufrufe: reine Funktionen, Blockmethoden, Primitive (`Intrinsic`, 4.1)
//! und native Funktionen (4.5).
//!
//! Die Mathematik laeuft in Programmbreite und kommt aus `libtaktm`, nicht
//! aus der Standardbibliothek der Plattform: `f64::sin` ruft die libm des
//! Systems, und glibc, musl und die ARM-Bibliotheken unterscheiden sich im
//! letzten Bit — Satz 9.4.4 waere damit unbelegbar (plan/m4.md 1).
//!
//! Was `libtaktm` noch nicht kuratiert hat (13.8), rechnet hier auch nicht.
//! Die Ablehnung steht in `takt-sema` als Stufenmeldung, damit sie den
//! Programmierer vor dem Lauf erreicht; der Trap hier ist der Rueckhalt
//! fuer eine MIR, die an der Pruefung vorbeikam.

use takt_diag::Span;
use takt_mir::expr::{BinaryOp, Intrinsic};
use takt_mir::machine::{ArithKind, FaultKind};
use takt_mir::types::{FloatWidth, IntWidth, Type};
use takt_mir::{FnId, NativeId, TypeId};

use crate::arith;
use crate::eval::{Ctx, Frame, MAX_CALL_DEPTH};
use crate::exec::Out;
use crate::value::{BlockState, EvalResult, Trap, Value, bug};

impl Ctx<'_, '_> {
    /// Ruft eine Funktion (3.9): Rahmen aus Argumenten und Lokalen, Rumpf
    /// ausfuehren, `return` liefert den Wert.
    pub fn call_fn(&mut self, id: FnId, args: Vec<Value>, span: Span) -> EvalResult<Value> {
        let f = &self.loaded.program.fns[id.index()];
        if args.len() != f.params.len() {
            return bug(format!("`{}`: {} Argumente fuer {} Parameter", f.name, args.len(), f.params.len()));
        }
        self.run_frame(id, args, span)
    }

    /// Fuehrt den Rumpf von `id` in einem Rahmen aus, der mit `prefix`
    /// beginnt (Argumente oder Instanzvariablen plus Argumente).
    fn run_frame(&mut self, id: FnId, prefix: Vec<Value>, span: Span) -> EvalResult<Value> {
        if self.depth >= MAX_CALL_DEPTH {
            return bug(format!("Aufruftiefe {MAX_CALL_DEPTH} ueberschritten an {span:?}"));
        }
        let f = &self.loaded.program.fns[id.index()];
        let skip = f.params.len();
        self.depth += 1;
        self.frames.push(Frame { vars: prefix, func: Some(id), block: None });
        let result = (|| {
            for v in &f.locals[skip..] {
                let value = match &v.init {
                    Some(init) => self.eval(init)?,
                    None => Value::default_for(v.ty, self.loaded.program),
                };
                self.frames.last_mut().expect("Rahmen").vars.push(value);
            }
            match self.exec_block(&f.body, crate::exec::Mode::Run)? {
                Out::Return(v) => Ok(v),
                Out::Normal => bug(format!("`{}` endet ohne return", f.name)),
                Out::Goto(_) | Out::Break => bug(format!("`{}`: Sprung aus einer Funktion", f.name)),
            }
        })();
        self.frames.pop();
        self.depth -= 1;
        result
    }

    /// Blockmethode (`step`, weitere Methoden): der Rahmen beginnt mit den
    /// Instanzvariablen, die nach dem Aufruf zurueckgeschrieben werden.
    pub fn call_block_method(
        &mut self,
        instance: &mut BlockState,
        id: FnId,
        args: Vec<Value>,
        span: Span,
    ) -> EvalResult<Value> {
        let f = &self.loaded.program.fns[id.index()];
        if args.len() != f.params.len() {
            return bug(format!("`{}`: {} Argumente fuer {} Parameter", f.name, args.len(), f.params.len()));
        }
        let n = instance.vars.len();
        let mut prefix = std::mem::take(&mut instance.vars);
        prefix.extend(args);
        match self.run_frame_keep(id, instance.block, prefix, span) {
            Ok((value, frame)) => {
                instance.vars = frame.vars.into_iter().take(n).collect();
                Ok(value)
            }
            Err((e, frame)) => {
                if let Some(frame) = frame {
                    instance.vars = frame.vars.into_iter().take(n).collect();
                }
                Err(e)
            }
        }
    }

    /// Wie `run_frame`, liefert den Rahmen zurueck (Instanzvariablen).
    fn run_frame_keep(
        &mut self,
        id: FnId,
        block: takt_mir::BlockId,
        prefix: Vec<Value>,
        span: Span,
    ) -> Result<(Value, Frame), (Trap, Option<Frame>)> {
        if self.depth >= MAX_CALL_DEPTH {
            return Err((Trap::Bug(format!("Aufruftiefe {MAX_CALL_DEPTH} ueberschritten an {span:?}")), None));
        }
        let f = &self.loaded.program.fns[id.index()];
        let skip = f.params.len();
        self.depth += 1;
        self.frames.push(Frame { vars: prefix, func: Some(id), block: Some(block) });
        let result: EvalResult<Value> = (|| {
            for v in &f.locals[skip..] {
                let value = match &v.init {
                    Some(init) => self.eval(init)?,
                    None => Value::default_for(v.ty, self.loaded.program),
                };
                self.frames.last_mut().expect("Rahmen").vars.push(value);
            }
            match self.exec_block(&f.body, crate::exec::Mode::Run)? {
                Out::Return(v) => Ok(v),
                Out::Normal => Ok(Value::Bool(true)),
                Out::Goto(_) | Out::Break => bug(format!("`{}`: Sprung aus einer Methode", f.name)),
            }
        })();
        let frame = self.frames.pop().expect("Rahmen");
        self.depth -= 1;
        match result {
            Ok(v) => Ok((v, frame)),
            Err(e) => Err((e, Some(frame))),
        }
    }

    /// Native Funktionen (4.5): die kuratierte Menge kommt mit `takt-native` (M4).
    pub fn call_native(&mut self, id: NativeId, _args: Vec<Value>, _span: Span) -> EvalResult<Value> {
        let n = &self.loaded.program.natives[id.index()];
        bug(format!("native Funktion `{}` ist im Interpreter noch nicht verfuegbar (M4)", n.name))
    }

    /// Primitive (4.1, 3.9, 3.10).
    pub fn intrinsic(
        &mut self,
        op: Intrinsic,
        args: Vec<Value>,
        ty: TypeId,
        arg_ty: TypeId,
        span: Span,
    ) -> EvalResult<Value> {
        let tick = self.tick;
        let domain = |what: &str| {
            Trap::Fault(crate::value::Fault::new(FaultKind::Arithmetic(ArithKind::Domain), what, span, tick))
        };
        let width_of = |t: TypeId| self.loaded.float_width(t).unwrap_or_else(|| self.loaded.program_float());
        let a = |i: usize| args.get(i).cloned().ok_or_else(|| Trap::Bug(format!("{}: Argument {i} fehlt", op.name())));
        match op {
            Intrinsic::Abs => match a(0)? {
                Value::Int(_) | Value::UInt(_) => {
                    let w = self.loaded.int_width(ty).unwrap_or(IntWidth::I64);
                    arith::fit(w, a(0)?.as_int().expect("int").abs(), span, tick)
                }
                Value::F32(x) => Ok(Value::F32(x.abs())),
                Value::F64(x) => Ok(Value::F64(x.abs())),
                Value::Duration(d) => d.checked_abs().map(Value::Duration).ok_or_else(|| {
                    Trap::Fault(crate::value::Fault::new(FaultKind::Arithmetic(ArithKind::Overflow), "abs", span, tick))
                }),
                other => bug(format!("abs auf {}", other.kind_name())),
            },
            Intrinsic::Min | Intrinsic::Max => {
                let (x, y) = (a(0)?, a(1)?);
                let ord = x.compare(&y).ok_or_else(|| Trap::Bug(format!("{} ohne Ordnung", op.name())))?;
                Ok(if (op == Intrinsic::Min) == ord.is_le() { x } else { y })
            }
            Intrinsic::Sqrt => {
                let x = a(0)?;
                let f = x.as_f64().ok_or_else(|| Trap::Bug("sqrt auf Nicht-Fliesskomma".into()))?;
                if f < 0.0 {
                    return Err(domain("sqrt eines negativen Werts"));
                }
                match x {
                    Value::F32(v) => arith::finite(FloatWidth::F32, f64::from(libtaktm::sqrt_f32(v)), span, tick),
                    _ => arith::finite(FloatWidth::F64, libtaktm::sqrt_f64(f), span, tick),
                }
            }
            Intrinsic::Sin
            | Intrinsic::Cos
            | Intrinsic::Tan
            | Intrinsic::Asin
            | Intrinsic::Acos
            | Intrinsic::Atan
            | Intrinsic::Exp
            | Intrinsic::Log => {
                let x = a(0)?;
                let f = x.as_f64().ok_or_else(|| Trap::Bug(format!("{} auf Nicht-Fliesskomma", op.name())))?;
                match op {
                    Intrinsic::Asin | Intrinsic::Acos if !(-1.0..=1.0).contains(&f) => {
                        return Err(domain("Argument ausserhalb [-1, 1]"));
                    }
                    Intrinsic::Log if f <= 0.0 => return Err(domain("log eines nicht positiven Werts")),
                    _ => {}
                }
                // Noch nicht kuratiert (13.8): keine korrekt gerundete
                // Implementierung, also auch kein Ergebnis. Die Plattform-libm
                // zu nehmen hiesse, eine Zusage zu behaupten, die sie nicht
                // haelt.
                let _ = f;
                uncurated(op)
            }
            Intrinsic::Atan2 | Intrinsic::Pow => {
                // Wie oben: noch nicht kuratiert. Die Argumente werden
                // trotzdem ausgewertet, damit ihre Faults (4.1) an derselben
                // Stelle entstehen wie mit Implementierung.
                let (_, _) = (a(0)?, a(1)?);
                uncurated(op)
            }
            Intrinsic::Fma => match (a(0)?, a(1)?, a(2)?) {
                (Value::F32(x), Value::F32(y), Value::F32(z)) => {
                    arith::finite(FloatWidth::F32, f64::from(libtaktm::fma_f32(x, y, z)), span, tick)
                }
                (Value::F64(x), Value::F64(y), Value::F64(z)) => {
                    arith::finite(FloatWidth::F64, libtaktm::fma_f64(x, y, z), span, tick)
                }
                _ => bug("fma auf gemischten Breiten"),
            },
            Intrinsic::Round | Intrinsic::Floor | Intrinsic::Ceil => {
                let x = a(0)?;
                let f = x.as_f64().ok_or_else(|| Trap::Bug(format!("{} auf Nicht-Fliesskomma", op.name())))?;
                let r = match op {
                    Intrinsic::Round => f.round(),
                    Intrinsic::Floor => f.floor(),
                    _ => f.ceil(),
                };
                if !((i64::MIN as f64)..(i64::MAX as f64)).contains(&r) {
                    return Err(Trap::Fault(crate::value::Fault::new(
                        FaultKind::Range,
                        format!("{r} passt nicht in int"),
                        span,
                        tick,
                    )));
                }
                Ok(Value::Int(r as i64))
            }
            Intrinsic::Rotl | Intrinsic::Rotr => {
                let w = self.loaded.int_width(arg_ty).unwrap_or(IntWidth::I64);
                let bits = i128::from(w.bits());
                let x = a(0)?.as_int().ok_or_else(|| Trap::Bug("rotl auf Nicht-Ganzzahl".into()))?;
                let n = a(1)?.as_int().ok_or_else(|| Trap::Bug("rotl-Betrag".into()))?.rem_euclid(bits);
                let mask = (1i128 << bits) - 1;
                let u = x & mask;
                let r = if op == Intrinsic::Rotl {
                    ((u << n) | (u >> (bits - n))) & mask
                } else {
                    ((u >> n) | (u << (bits - n))) & mask
                };
                Ok(arith::wrap(w, r))
            }
            Intrinsic::WrappingAdd | Intrinsic::WrappingSub | Intrinsic::WrappingMul => {
                let w = self.loaded.int_width(ty).unwrap_or(IntWidth::I64);
                let (x, y) = (a(0)?.as_int(), a(1)?.as_int());
                let (x, y) = (
                    x.ok_or_else(|| Trap::Bug("wrapping auf Nicht-Ganzzahl".into()))?,
                    y.ok_or_else(|| Trap::Bug("wrapping auf Nicht-Ganzzahl".into()))?,
                );
                Ok(arith::wrap(
                    w,
                    match op {
                        Intrinsic::WrappingAdd => x + y,
                        Intrinsic::WrappingSub => x - y,
                        _ => x * y,
                    },
                ))
            }
            Intrinsic::SaturatingAdd | Intrinsic::SaturatingSub => {
                let w = self.loaded.int_width(ty).unwrap_or(IntWidth::I64);
                let (x, y) = (a(0)?.as_int(), a(1)?.as_int());
                let (x, y) = (
                    x.ok_or_else(|| Trap::Bug("saturating auf Nicht-Ganzzahl".into()))?,
                    y.ok_or_else(|| Trap::Bug("saturating auf Nicht-Ganzzahl".into()))?,
                );
                let (lo, hi) = arith::bounds(w);
                let r = if op == Intrinsic::SaturatingAdd { x + y } else { x - y };
                Ok(Value::int(w, r.clamp(lo, hi)))
            }
            Intrinsic::Interp => {
                let (table, x) = (a(0)?, a(1)?);
                let Value::Table(points) = table else { return bug("interp ohne Tabelle") };
                let width = match self.loaded.ty(arg_ty) {
                    Type::Table { value, .. } => width_of(*value),
                    _ => self.loaded.program_float(),
                };
                let Some(first) = points.first() else { return bug("interp ueber leere Tabelle") };
                let last = points.last().expect("nicht leer");
                if x.compare(&first.0).is_some_and(|o| o.is_le()) {
                    return Ok(first.1.clone());
                }
                if x.compare(&last.0).is_some_and(|o| o.is_ge()) {
                    return Ok(last.1.clone());
                }
                for pair in points.windows(2) {
                    let ((x0, y0), (x1, y1)) = (&pair[0], &pair[1]);
                    if x.compare(x1).is_some_and(|o| o.is_le()) {
                        // y0 + (y1 - y0) * (x - x0) / (x1 - x0), jede Operation gerundet
                        let dy = arith::float_binary(BinaryOp::Sub, y1, y0, span, tick)?;
                        let dx = arith::float_binary(BinaryOp::Sub, &x, x0, span, tick)?;
                        let w = arith::float_binary(BinaryOp::Sub, x1, x0, span, tick)?;
                        let t = arith::float_binary(BinaryOp::Mul, &dy, &dx, span, tick)?;
                        let q = arith::float_binary(BinaryOp::Div, &t, &w, span, tick)?;
                        let r = arith::float_binary(BinaryOp::Add, y0, &q, span, tick)?;
                        return arith::finite(width, r.as_f64().expect("float"), span, tick);
                    }
                }
                Ok(last.1.clone())
            }
        }
    }
}

/// Eine Funktion, die `libtaktm` noch nicht kuratiert hat (13.8).
///
/// Der Weg dahin ist eine Stufenmeldung in `takt-sema`, nicht dieser
/// Trap — ein Programmierer soll es vor dem Lauf erfahren. Die Meldung
/// hier faengt eine MIR ab, die an der Pruefung vorbeikam, etwa weil sie
/// aus einer Datei gelesen wurde.
fn uncurated(op: Intrinsic) -> EvalResult<Value> {
    bug(format!(
        "`{}` ist noch nicht korrekt gerundet implementiert (libtaktm, 4.2); \
         ohne sie waere Satz 9.4.4 nicht belegbar",
        op.name()
    ))
}
