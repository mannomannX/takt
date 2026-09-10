//! `eval(e, s) -> value | FAULT f` (Referenz 9.2): strukturelle Rekursion
//! ueber `ExprKind`, total. Ausdruecke sind seiteneffektfrei (4.4); der
//! Kontext liefert Variablen, Inputs, Ψ und die eingebauten Groessen.

use takt_diag::Span;
use takt_mir::expr::*;
use takt_mir::machine::FaultKind;
use takt_mir::stmt::Place;
use takt_mir::types::{Const, FloatWidth, IntWidth, Range, Type};
use takt_mir::{TypeId, VarId};

use crate::arith;
use crate::env::Outer;
use crate::loaded::Loaded;
use crate::value::{BlockState, EvalResult, Fault, Sample, Trap, Value, bug};

/// Hoechste Aufruftiefe; der Aufrufgraph ist azyklisch (SC-11), die Grenze
/// faengt nur Fehler des Verifiers ab.
pub const MAX_CALL_DEPTH: u32 = 256;

/// Funktionsrahmen: Parameter, dann Lokale (bei Blockmethoden davor die
/// Instanzvariablen, plan/mir.md Abschnitt 7).
#[derive(Clone, Debug, Default)]
pub struct Frame {
    /// Werte je `VarId`.
    pub vars: Vec<Value>,
    /// Funktion des Rahmens (Typen der Lokalen); `None` beim Bau einer Blockinstanz.
    pub func: Option<takt_mir::FnId>,
    /// Block, dessen Instanzvariablen den Rahmen beginnen.
    pub block: Option<takt_mir::BlockId>,
}

/// Auswertungskontext: geladenes Programm, Umgebung, Tick, Rahmen.
pub struct Ctx<'p, 'o> {
    /// Programm mit Tabellen.
    pub loaded: &'o Loaded<'p>,
    /// Umgebung (Maschine oder Konstantenauswertung).
    pub outer: &'o mut dyn Outer,
    /// Aktueller Tick (fuer Faults).
    pub tick: u64,
    /// Funktionsrahmen, innerster zuletzt.
    pub frames: Vec<Frame>,
    /// Laufende `for`-Schleifen, aeusserste zuerst: ihr aktueller Index.
    /// Eine Beobachtungsstelle in einer Schleife hat je Durchlauf eine
    /// eigene Flanke (5.6).
    pub loops: Vec<i64>,
    pub(crate) depth: u32,
}

impl<'p, 'o> Ctx<'p, 'o> {
    /// Neuer Kontext ohne Rahmen.
    pub fn new(loaded: &'o Loaded<'p>, outer: &'o mut dyn Outer, tick: u64) -> Self {
        Ctx { loaded, outer, tick, frames: Vec::new(), loops: Vec::new(), depth: 0 }
    }

    /// Fault an einer Stelle.
    pub fn fault(&self, kind: FaultKind, message: impl Into<String>, span: Span) -> Trap {
        Trap::Fault(Fault::new(kind, message, span, self.tick))
    }

    fn int_width(&self, ty: TypeId, span: Span) -> EvalResult<IntWidth> {
        self.loaded.int_width(ty).ok_or_else(|| Trap::Bug(format!("Ganzzahlbreite fuer Typ {} an {span:?}", ty.0)))
    }

    fn float_width(&self, ty: TypeId) -> FloatWidth {
        self.loaded.float_width(ty).unwrap_or_else(|| self.loaded.program_float())
    }

    /// Variable lesen: Rahmen zuerst, sonst Umgebung.
    pub fn var(&self, v: VarId) -> EvalResult<&Value> {
        match self.frames.last() {
            Some(f) => f.vars.get(v.index()).ok_or_else(|| Trap::Bug(format!("Variable {} nicht im Rahmen", v.0))),
            None => self.outer.var(v),
        }
    }

    /// Variable schreibend.
    pub fn var_mut(&mut self, v: VarId) -> EvalResult<&mut Value> {
        match self.frames.last_mut() {
            Some(f) => f.vars.get_mut(v.index()).ok_or_else(|| Trap::Bug(format!("Variable {} nicht im Rahmen", v.0))),
            None => self.outer.var_mut(v),
        }
    }

    /// Typ einer Variablen (Rahmen oder Umgebung).
    pub fn var_type(&self, v: VarId) -> EvalResult<TypeId> {
        let Some(frame) = self.frames.last() else { return self.outer.var_type(v) };
        let p = self.loaded.program;
        let mut i = v.index();
        if let Some(b) = frame.block {
            let def = &p.blocks[b.index()];
            if i < def.params.len() {
                return Ok(def.params[i].ty);
            }
            i -= def.params.len();
            if i < def.state_vars.len() {
                return Ok(def.state_vars[i].ty);
            }
            i -= def.state_vars.len();
        }
        match frame.func {
            Some(f) => p.fns[f.index()]
                .locals
                .get(i)
                .map(|l| l.ty)
                .ok_or_else(|| Trap::Bug(format!("Variable {} ohne Typ", v.0))),
            None => bug(format!("Variable {} ohne Typ", v.0)),
        }
    }

    /// Bool-Ausdruck.
    pub fn eval_bool(&mut self, e: &Expr) -> EvalResult<bool> {
        match self.eval(e)? {
            Value::Bool(b) => Ok(b),
            v => bug(format!("Bool erwartet, {} gefunden", v.kind_name())),
        }
    }

    /// Ganzzahl-Ausdruck.
    pub fn eval_int(&mut self, e: &Expr) -> EvalResult<i128> {
        let v = self.eval(e)?;
        v.as_int().ok_or_else(|| Trap::Bug(format!("Ganzzahl erwartet, {} gefunden", v.kind_name())))
    }

    /// Dauer-Ausdruck.
    pub fn eval_duration(&mut self, e: &Expr) -> EvalResult<i64> {
        let v = self.eval(e)?;
        v.as_duration().ok_or_else(|| Trap::Bug(format!("Dauer erwartet, {} gefunden", v.kind_name())))
    }

    /// Auswertung eines Ausdrucks.
    pub fn eval(&mut self, e: &Expr) -> EvalResult<Value> {
        let span = e.span;
        match &e.kind {
            ExprKind::Bool(b) => Ok(Value::Bool(*b)),
            ExprKind::Int(i) => Ok(match self.loaded.int_width(e.ty) {
                Some(w) if !w.signed() => Value::UInt(*i as u64),
                _ => Value::Int(*i),
            }),
            ExprKind::Float(f) => Ok(Value::float(self.float_width(e.ty), *f)),
            ExprKind::Duration(d) => Ok(Value::Duration(*d)),
            ExprKind::Str(s) => Ok(Value::Str(s.clone())),
            ExprKind::None => Ok(Value::Optional(None)),
            ExprKind::Default => Ok(Value::default_for(e.ty, self.loaded.program)),
            ExprKind::Variant { variant, fields, .. } => {
                let fields = fields.iter().map(|f| self.eval(f)).collect::<EvalResult<Vec<_>>>()?;
                Ok(Value::Enum { variant: *variant, fields })
            }
            ExprKind::Record { fields, .. } => {
                Ok(Value::Record(fields.iter().map(|f| self.eval(f)).collect::<EvalResult<Vec<_>>>()?))
            }
            ExprKind::Array(items) => {
                let values = items.iter().map(|x| self.eval(x)).collect::<EvalResult<Vec<_>>>()?;
                Ok(match self.loaded.ty(e.ty) {
                    Type::Table { .. } => Value::Table(
                        values
                            .into_iter()
                            .map(|v| match v {
                                Value::Array(mut pair) if pair.len() == 2 => {
                                    let b = pair.pop().expect("Paar");
                                    let a = pair.pop().expect("Paar");
                                    Ok((a, b))
                                }
                                other => bug(format!("Stuetzstelle erwartet, {} gefunden", other.kind_name())),
                            })
                            .collect::<EvalResult<Vec<_>>>()?,
                    ),
                    Type::Vec { .. } => Value::Vec(values),
                    // 8.3: ein Modell speist einen oversampelten Kanal mit
                    // dem Tick-Array (8.9).
                    Type::Samples { .. } => Value::Samples(values),
                    Type::Mat { rows, cols, .. } => {
                        let mut data = Vec::with_capacity(crate::value::mat_len(*rows, *cols));
                        for row in values {
                            match row {
                                Value::Array(items) => data.extend(items),
                                other => return bug(format!("Matrixzeile erwartet, {} gefunden", other.kind_name())),
                            }
                        }
                        Value::Mat { rows: *rows, cols: *cols, data }
                    }
                    _ => Value::Array(values),
                })
            }
            ExprKind::Tuple(a, b) => Ok(Value::Array(vec![self.eval(a)?, self.eval(b)?])),
            ExprKind::BlockInit { block, args, count } => {
                let args = args.iter().map(|a| self.eval(a)).collect::<EvalResult<Vec<_>>>()?;
                match count {
                    None => Ok(Value::Block(Box::new(self.instantiate_block(*block, &args)?))),
                    Some(n) => Ok(Value::Array(
                        (0..*n)
                            .map(|_| Ok(Value::Block(Box::new(self.instantiate_block(*block, &args)?))))
                            .collect::<EvalResult<Vec<_>>>()?,
                    )),
                }
            }
            ExprKind::Var(v) => self.var(*v).cloned(),
            ExprKind::Param(p) => self.outer.param(*p).cloned(),
            ExprKind::Command(c) => Ok(Value::Bool(self.outer.command(*c)?)),
            ExprKind::Input { channel, .. } => {
                let sample = self.outer.input(*channel)?;
                match (&sample.value, sample.valid()) {
                    (Some(v), true) => Ok(v.clone()),
                    _ => {
                        let name = self.loaded.program.channels[channel.index()].name.clone();
                        Err(self.fault(FaultKind::SensorFault, format!("`{name}` ungueltig"), span))
                    }
                }
            }
            ExprKind::Output(c) => self.outer.output(*c).cloned(),
            ExprKind::Published { machine, var } => {
                self.machine_index(machine)?;
                self.outer.published(machine.machine, *var).cloned()
            }
            ExprKind::StateOf(m) => {
                self.machine_index(m)?;
                self.outer.state_of(m.machine)
            }
            ExprKind::Signal { machine, signal } => {
                self.machine_index(machine)?;
                Ok(Value::Bool(self.outer.signal(machine.machine, *signal)?))
            }
            ExprKind::Builtin(b) => self.outer.builtin(*b),
            ExprKind::Field { base, field } => {
                let v = self.eval(base)?;
                field_of(v, *field)
            }
            ExprKind::Index { base, index } => {
                let v = self.eval(base)?;
                let i = self.eval_int(index)?;
                self.index_value(v, i, span)
            }
            ExprKind::Index2 { base, row, col } => {
                let v = self.eval(base)?;
                let r = self.eval_int(row)?;
                let c = self.eval_int(col)?;
                match v {
                    Value::Mat { rows, cols, data } => {
                        if r < 0 || c < 0 || r >= i128::from(rows) || c >= i128::from(cols) {
                            return Err(self.fault(
                                FaultKind::Range,
                                format!("Matrixindex ({r}, {c}) ausserhalb"),
                                span,
                            ));
                        }
                        Ok(data[(r as u32 * cols + c as u32) as usize].clone())
                    }
                    other => bug(format!("Matrix erwartet, {} gefunden", other.kind_name())),
                }
            }
            ExprKind::Slice { base, from, to } => {
                let v = self.eval(base)?;
                let a = self.eval_int(from)?;
                let b = self.eval_int(to)?;
                let len = match &v {
                    Value::Bytes(x) => x.len(),
                    Value::Vec(x) | Value::Array(x) => x.len(),
                    other => return bug(format!("Slice auf {}", other.kind_name())),
                };
                if a < 0 || b < a || b > len as i128 {
                    return Err(self.fault(FaultKind::Range, format!("Slice {a}..{b} ausserhalb 0..{len}"), span));
                }
                let (a, b) = (a as usize, b as usize);
                Ok(match v {
                    Value::Bytes(x) => Value::Bytes(x[a..b].to_vec()),
                    Value::Vec(x) => Value::Vec(x[a..b].to_vec()),
                    Value::Array(x) => Value::Array(x[a..b].to_vec()),
                    _ => unreachable!(),
                })
            }
            ExprKind::Accessor { base, accessor, args } => self.accessor(base, *accessor, args, span),
            ExprKind::Unary { op, expr } => {
                let v = self.eval(expr)?;
                arith::unary(*op, &v, self.loaded.int_width(e.ty), span, self.tick)
            }
            ExprKind::Binary { op, lhs, rhs } => self.binary(*op, lhs, rhs, e.ty, span),
            ExprKind::Cond { cond, then, otherwise } => {
                if self.eval_bool(cond)? {
                    self.eval(then)
                } else {
                    self.eval(otherwise)
                }
            }
            ExprKind::Cast { expr, to } => {
                let v = self.eval(expr)?;
                self.cast(v, *to, span)
            }
            ExprKind::Convert { expr, kind, unit } => {
                let v = self.eval(expr)?;
                self.convert(v, *kind, *unit, expr.ty, e.ty, span)
            }
            ExprKind::Format(f) => {
                // 8.8: der Text entsteht in einem festen Puffer; die
                // Hoechstlaenge steht im Knoten.
                let text = crate::format::render(f, self);
                Ok(match self.loaded.ty(e.ty) {
                    Type::Line { .. } => Value::Line { text, truncated: false },
                    _ => Value::Str(text),
                })
            }
            ExprKind::Stream(_) => {
                // Ein Strom hat keinen Wert; nur seine Zaehler sind lesbar
                // (8.6), und die faengt `accessor` ab.
                bug("ein Strom ist kein Wert")
            }
            ExprKind::Matches { subject, kind, pattern, binding } => {
                self.matches(subject, *kind, pattern, *binding, span)
            }
            ExprKind::Call { callee, args } => {
                let args = args.iter().map(|a| self.eval(a)).collect::<EvalResult<Vec<_>>>()?;
                self.call_fn(*callee, args, span)
            }
            ExprKind::NativeCall { native, args } => {
                let args = args.iter().map(|a| self.eval(a)).collect::<EvalResult<Vec<_>>>()?;
                self.call_native(*native, args, span)
            }
            ExprKind::MatOp { .. } => bug("Matrixoperationen ab M6"),
            ExprKind::Decode { record, bytes } => {
                // `R.decode(b) -> R?` (3.7): `none` bei zu kurzem Puffer,
                // Konstantenverstoss oder Range-Verletzung; nie ein Fault.
                let Value::Bytes(b) = self.eval(bytes)? else {
                    return bug("`decode` verlangt `bytes<N>`");
                };
                Ok(Value::Optional(crate::wire::decode(self.loaded, *record, &b).map(Box::new)))
            }
            ExprKind::Checked { expr, kind } => self.checked(expr, kind, span),
            ExprKind::Lift(inner) => Ok(Value::Optional(Some(Box::new(self.eval(inner)?)))),
            ExprKind::Ok(inner) => Ok(Value::Result(Ok(Box::new(self.eval(inner)?)))),
            ExprKind::Err(inner) => Ok(Value::Result(Err(Box::new(self.eval(inner)?)))),
            ExprKind::Intrinsic { op, args } => {
                let values = args.iter().map(|a| self.eval(a)).collect::<EvalResult<Vec<_>>>()?;
                self.intrinsic(*op, values, e.ty, args.first().map_or(e.ty, |a| a.ty), span)
            }
        }
    }

    fn machine_index(&mut self, m: &MachineRef) -> EvalResult<()> {
        if m.index.is_some() {
            return bug("Instanz-Arrays mit Indexausdruck ab M8");
        }
        Ok(())
    }

    /// Element eines Arrays, Vektors, Bytes oder Strings mit Range-Pruefung.
    pub fn index_value(&self, v: Value, i: i128, span: Span) -> EvalResult<Value> {
        let len = match &v {
            Value::Array(x) | Value::Vec(x) | Value::Samples(x) => x.len(),
            Value::Bytes(x) => x.len(),
            other => return bug(format!("Index auf {}", other.kind_name())),
        };
        if i < 0 || i >= len as i128 {
            return Err(self.fault(
                FaultKind::Range,
                format!("Index {i} ausserhalb 0..{}", len.saturating_sub(1)),
                span,
            ));
        }
        Ok(match v {
            Value::Array(mut x) | Value::Vec(mut x) | Value::Samples(mut x) => x.swap_remove(i as usize),
            Value::Bytes(x) => Value::UInt(u64::from(x[i as usize])),
            _ => unreachable!(),
        })
    }

    fn binary(&mut self, op: BinaryOp, lhs: &Expr, rhs: &Expr, ty: TypeId, span: Span) -> EvalResult<Value> {
        if op == BinaryOp::And {
            return Ok(Value::Bool(self.eval_bool(lhs)? && self.eval_bool(rhs)?));
        }
        if op == BinaryOp::Or {
            return Ok(Value::Bool(self.eval_bool(lhs)? || self.eval_bool(rhs)?));
        }
        let a = self.eval(lhs)?;
        let b = self.eval(rhs)?;
        match (&a, &b) {
            (Value::Int(_) | Value::UInt(_), Value::Int(_) | Value::UInt(_)) => {
                let width = match op {
                    BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge | BinaryOp::Eq | BinaryOp::Ne => {
                        self.loaded.int_width(lhs.ty).unwrap_or(IntWidth::I64)
                    }
                    _ => self.int_width(ty, span)?,
                };
                arith::int_binary(op, a.as_int().expect("int"), b.as_int().expect("int"), width, span, self.tick)
            }
            (Value::F32(_) | Value::F64(_), Value::F32(_) | Value::F64(_)) => {
                arith::float_binary(op, &a, &b, span, self.tick)
            }
            (Value::Duration(_), _) | (_, Value::Duration(_)) => arith::duration_binary(op, &a, &b, span, self.tick),
            _ => match op {
                BinaryOp::Eq => Ok(Value::Bool(a == b)),
                BinaryOp::Ne => Ok(Value::Bool(a != b)),
                BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => {
                    let ord = a
                        .compare(&b)
                        .ok_or_else(|| Trap::Bug(format!("Vergleich von {} und {}", a.kind_name(), b.kind_name())))?;
                    Ok(Value::Bool(match op {
                        BinaryOp::Lt => ord.is_lt(),
                        BinaryOp::Le => ord.is_le(),
                        BinaryOp::Gt => ord.is_gt(),
                        _ => ord.is_ge(),
                    }))
                }
                _ => bug(format!("{op:?} auf {} und {}", a.kind_name(), b.kind_name())),
            },
        }
    }

    /// Abtastung, wenn der Ausdruck ein Input-Lesen ist (auch unter `Checked`).
    fn sample_of(&mut self, e: &Expr) -> EvalResult<Option<Sample>> {
        match &e.kind {
            ExprKind::Input { channel, .. } => Ok(Some(self.outer.input(*channel)?.clone())),
            ExprKind::Checked { expr, .. } => self.sample_of(expr),
            ExprKind::Index { base, index } if matches!(base.kind, ExprKind::Input { .. }) => {
                // Ein Channel-Array traegt eine Abtastung je Element (8.1);
                // der Index ist ein beliebiger Ausdruck (Schleifenvariable).
                let ExprKind::Input { channel, .. } = base.kind else { unreachable!() };
                let i = match self.eval(index)? {
                    Value::Int(i) => i,
                    Value::UInt(u) => i64::try_from(u).unwrap_or(-1),
                    _ => return Ok(None),
                };
                let sample = self.outer.input(channel)?;
                let at = usize::try_from(i).ok();
                Ok(Some(match (&sample.value, at) {
                    (Some(Value::Array(items)), Some(i)) if i < items.len() => {
                        Sample { value: Some(items[i].clone()), ..sample.clone() }
                    }
                    _ => sample.clone(),
                }))
            }
            _ => Ok(None),
        }
    }

    /// `x matches P [as m]` und `x has P` (8.7). Ein Muster faultet nie; ein
    /// Wert, der nicht passt, ergibt `false`. Trifft das Muster und traegt es
    /// eine Bindung, entsteht der Bindungsrecord aus den Captures; sichtbar
    /// ist er nur im dominierten Zweig (8.7, 4.4).
    pub(crate) fn matches(
        &mut self,
        subject: &Expr,
        kind: MatchKind,
        pattern: &takt_mir::pattern::Pattern,
        binding: Option<VarId>,
        span: Span,
    ) -> EvalResult<Value> {
        let value = self.eval(subject)?;
        // Konstante Feldwerte eines Record-Musters vorab auswerten.
        let consts = match pattern {
            takt_mir::pattern::Pattern::Record { fields, .. } => {
                fields.iter().map(|(_, e)| self.eval(e)).collect::<EvalResult<Vec<_>>>()?
            }
            takt_mir::pattern::Pattern::Text { .. } => Vec::new(),
        };
        let Some(caps) = crate::pattern::match_value(pattern, kind, &value, &consts) else {
            return Ok(Value::Bool(false));
        };
        if let Some(var) = binding {
            let record = self.binding_record(&value, caps, var, span)?;
            *self.var_mut(var)? = record;
        }
        Ok(Value::Bool(true))
    }

    /// Baut den Bindungsrecord: die Captures in Musterreihenfolge, gefolgt
    /// von den Feldern eines Stream-Elements, soweit der Typ sie fuehrt.
    fn binding_record(&mut self, subject: &Value, caps: Vec<Value>, var: VarId, span: Span) -> EvalResult<Value> {
        let ty = self.outer.var_type(var).or_else(|_| self.local_type(var, span))?;
        let Type::Record(r) = self.loaded.ty(ty) else {
            return bug(format!("Bindung {} ist kein Record", var.0));
        };
        let defs = self.loaded.program.records[r.index()].fields.clone();
        let mut fields = caps;
        for def in defs.iter().skip(fields.len()) {
            // Ausserhalb eines Streams gibt es weder `.t` noch `.seq`; die
            // Felder bekommen ihren Standardwert (8.7).
            let v = match def.name.as_str() {
                "text" | "data" => subject.clone(),
                _ => Value::default_for(def.ty, self.loaded.program),
            };
            fields.push(v);
        }
        fields.truncate(defs.len());
        Ok(Value::Record(fields))
    }

    /// Typ einer lokalen Variablen aus dem Rahmen.
    fn local_type(&self, var: VarId, span: Span) -> EvalResult<TypeId> {
        let func = self.frames.last().and_then(|f| f.func);
        match func {
            Some(f) => self.loaded.program.fns[f.index()]
                .locals
                .get(var.index())
                .map(|v| v.ty)
                .ok_or_else(|| Trap::Bug(format!("Variable {} an {span:?}", var.0))),
            None => bug(format!("Typ der Variable {} unbekannt", var.0)),
        }
    }

    fn accessor(&mut self, base: &Expr, acc: Accessor, args: &[Expr], span: Span) -> EvalResult<Value> {
        // Zaehler und freier Platz eines Stroms lesen den Puffer, nicht den
        // Wert des Ausdrucks (8.6, 8.8).
        if matches!(self.loaded.ty(base.ty), Type::Stream(_)) {
            let r = match &base.kind {
                ExprKind::Input { channel, .. } => Some(StreamRef::Channel(*channel)),
                ExprKind::Stream(s) => Some(StreamRef::Internal(*s)),
                _ => None,
            };
            if let Some(r) = r {
                if let Some(v) = self.outer.stream_stat(r, acc)? {
                    return Ok(v);
                }
            }
        }
        // Wrapper-Zugriffe auf Inputs lesen die Abtastung, nicht den Wert (3.5).
        if let Some(sample) = self.sample_of(base)? {
            match acc {
                Accessor::Valid => return Ok(Value::Bool(sample.valid())),
                Accessor::Suspect => return Ok(Value::Bool(sample.quality == crate::value::Quality::Suspect)),
                Accessor::Stale => return Ok(Value::Bool(sample.quality == crate::value::Quality::Stale)),
                Accessor::Age => return Ok(Value::Duration(sample.age)),
                Accessor::Reason => {
                    let variant = match sample.reason {
                        Some(crate::value::Reason::Stale) => 0,
                        Some(crate::value::Reason::OutOfRange) => 1,
                        Some(crate::value::Reason::Implausible) => 2,
                        Some(crate::value::Reason::Driver) => 3,
                        Some(crate::value::Reason::Node) => 4,
                        None => return Ok(Value::Optional(None)),
                    };
                    return Ok(Value::Optional(Some(Box::new(Value::Enum { variant, fields: Vec::new() }))));
                }
                Accessor::Or => {
                    let valid = sample.valid();
                    return match (sample.value, valid) {
                        (Some(v), true) => Ok(v),
                        _ => self.eval(&args[0]),
                    };
                }
                _ => {}
            }
        }
        let v = self.eval(base)?;
        match (acc, v) {
            (Accessor::Valid, Value::Optional(o)) => Ok(Value::Bool(o.is_some())),
            (Accessor::Or, Value::Optional(Some(v))) => Ok(*v),
            (Accessor::Or, Value::Optional(None)) => self.eval(&args[0]),
            (Accessor::Ok, Value::Result(r)) => Ok(Value::Bool(r.is_ok())),
            (Accessor::Err, Value::Result(r)) => Ok(match r {
                Ok(_) => Value::Optional(None),
                Err(e) => Value::Optional(Some(e)),
            }),
            (Accessor::Or, Value::Result(Ok(v))) => Ok(*v),
            (Accessor::Or, Value::Result(Err(_))) => self.eval(&args[0]),
            (Accessor::Len, Value::Bytes(b)) => Ok(Value::Int(b.len() as i64)),
            (Accessor::Len, Value::Vec(x) | Value::Array(x)) => Ok(Value::Int(x.len() as i64)),
            (Accessor::Len, Value::Map(x)) => Ok(Value::Int(x.len() as i64)),
            (Accessor::Len, Value::Str(s) | Value::Line { text: s, .. }) => Ok(Value::Int(s.len() as i64)),
            (Accessor::Count, Value::Array(x) | Value::Samples(x)) => Ok(Value::Int(x.len() as i64)),
            (Accessor::Last, Value::Array(x) | Value::Samples(x)) => {
                x.last().cloned().ok_or_else(|| self.fault(FaultKind::MissingValue, "leeres Array", span))
            }
            (Accessor::Min | Accessor::Max | Accessor::Mean | Accessor::Rms, Value::Array(x) | Value::Samples(x)) => {
                self.reduce(acc, &x, base.ty, span)
            }
            (Accessor::Truncated, Value::Line { truncated, .. }) => Ok(Value::Bool(truncated)),
            (Accessor::StartsWith, Value::Str(s) | Value::Line { text: s, .. }) => match self.eval(&args[0])? {
                Value::Str(t) => Ok(Value::Bool(s.starts_with(&t))),
                other => bug(format!("starts_with mit {}", other.kind_name())),
            },
            (Accessor::Contains, Value::Str(s) | Value::Line { text: s, .. }) => match self.eval(&args[0])? {
                Value::Str(t) => Ok(Value::Bool(s.contains(&t))),
                other => bug(format!("contains mit {}", other.kind_name())),
            },
            (Accessor::Encode, v @ Value::Record(_)) => {
                // `f.encode() -> bytes<SIZE>` (3.7): der Plan steht im Typ,
                // Konstantenfelder werden dabei gesetzt.
                let Type::Record(r) = self.loaded.ty(base.ty) else {
                    return bug("`encode` auf einem Nicht-Record");
                };
                match crate::wire::encode(self.loaded, *r, &v) {
                    Some(bytes) => Ok(Value::Bytes(bytes)),
                    None => bug("`encode` ohne Byteplan"),
                }
            }
            (Accessor::Get, Value::Vec(x) | Value::Array(x)) => {
                let i = self.eval_int(&args[0])?;
                Ok(Value::Optional(if i >= 0 && (i as usize) < x.len() {
                    Some(Box::new(x[i as usize].clone()))
                } else {
                    None
                }))
            }
            (Accessor::Bit, v @ (Value::Int(_) | Value::UInt(_))) => {
                let width = self.int_width(base.ty, span)?;
                let i = self.eval_int(&args[0])?;
                if i < 0 || i >= i128::from(width.bits()) {
                    return Err(self.fault(FaultKind::Range, format!("Bitposition {i} ausserhalb"), span));
                }
                Ok(Value::Bool((v.as_int().expect("int") >> i) & 1 == 1))
            }
            (Accessor::Bits, v @ (Value::Int(_) | Value::UInt(_))) => {
                let width = self.int_width(base.ty, span)?;
                let hi = self.eval_int(&args[0])?;
                let lo = self.eval_int(&args[1])?;
                if lo < 0 || hi < lo || hi >= i128::from(width.bits()) {
                    return Err(self.fault(FaultKind::Range, format!("Bitbereich {hi}..{lo} ausserhalb"), span));
                }
                let mask = (1i128 << (hi - lo + 1)) - 1;
                Ok(Value::Int(((v.as_int().expect("int") >> lo) & mask) as i64))
            }
            (Accessor::WithBit, v @ (Value::Int(_) | Value::UInt(_))) => {
                let width = self.int_width(base.ty, span)?;
                let i = self.eval_int(&args[0])?;
                let b = self.eval_bool(&args[1])?;
                if i < 0 || i >= i128::from(width.bits()) {
                    return Err(self.fault(FaultKind::Range, format!("Bitposition {i} ausserhalb"), span));
                }
                let x = v.as_int().expect("int");
                let y = if b { x | (1i128 << i) } else { x & !(1i128 << i) };
                Ok(arith::wrap(width, y))
            }
            (Accessor::Wrap(w), v @ (Value::Int(_) | Value::UInt(_))) => Ok(arith::wrap(w, v.as_int().expect("int"))),
            (Accessor::Done | Accessor::Result, Value::Handle) => bug("Jobs ab M6"),
            (acc, v) => bug(format!("Zugriff {} auf {}", acc.name(), v.kind_name())),
        }
    }

    /// Reduktionen ueber Arrays und Samples (3.9): `min`, `max`, `mean`, `rms`.
    fn reduce(&self, acc: Accessor, items: &[Value], array_ty: TypeId, span: Span) -> EvalResult<Value> {
        if items.is_empty() {
            return Err(self.fault(FaultKind::MissingValue, "Reduktion ueber leeres Array", span));
        }
        let elem_ty = match self.loaded.ty(array_ty) {
            Type::Array { elem, .. } | Type::Samples { elem, .. } => *elem,
            _ => array_ty,
        };
        match acc {
            Accessor::Min | Accessor::Max => {
                let mut best = items[0].clone();
                for x in &items[1..] {
                    let ord = x.compare(&best).ok_or_else(|| Trap::Bug("Reduktion ohne Ordnung".into()))?;
                    if (acc == Accessor::Min && ord.is_lt()) || (acc == Accessor::Max && ord.is_gt()) {
                        best = x.clone();
                    }
                }
                Ok(best)
            }
            _ => {
                let width = self.float_width(elem_ty);
                let mut acc_v = Value::float(width, 0.0);
                for x in items {
                    let term = if acc == Accessor::Rms {
                        arith::float_binary(BinaryOp::Mul, x, x, span, self.tick)?
                    } else {
                        x.clone()
                    };
                    acc_v = arith::float_binary(BinaryOp::Add, &acc_v, &term, span, self.tick)?;
                }
                let n = Value::float_from_int(width, items.len() as i128);
                let mean = arith::float_binary(BinaryOp::Div, &acc_v, &n, span, self.tick)?;
                if acc == Accessor::Rms {
                    arith::finite(width, mean.as_f64().expect("float").sqrt(), span, self.tick)
                } else {
                    Ok(mean)
                }
            }
        }
    }

    /// `x as T` (3.10): Ganzzahlbreiten range-geprueft, aufwaerts frei, nach
    /// Fliesskomma gerundet (4.1).
    fn cast(&self, v: Value, to: TypeId, span: Span) -> EvalResult<Value> {
        match self.loaded.ty(to) {
            Type::Int { width, .. } => {
                let x =
                    v.as_int().ok_or_else(|| Trap::Bug(format!("as {} auf {}", arith::name(*width), v.kind_name())))?;
                let (lo, hi) = arith::bounds(*width);
                if x < lo || x > hi {
                    return Err(self.fault(
                        FaultKind::Range,
                        format!("{x} passt nicht in {}", arith::name(*width)),
                        span,
                    ));
                }
                Ok(Value::int(*width, x))
            }
            Type::Float { width, .. } => match v {
                Value::Int(_) | Value::UInt(_) => Ok(Value::float_from_int(*width, v.as_int().expect("int"))),
                Value::F32(_) | Value::F64(_) => arith::finite(*width, v.as_f64().expect("float"), span, self.tick),
                other => bug(format!("as float auf {}", other.kind_name())),
            },
            other => bug(format!("as auf Typ {other:?}")),
        }
    }

    /// `.to(U)`, `.to_float(U)`, `.as(U)` (3.2, 3.3): Faktoren als rationale
    /// Zahlen; jede Fliesskommaoperation in Programmbreite gerundet.
    fn convert(
        &self,
        v: Value,
        kind: ConvertKind,
        unit: takt_mir::UnitId,
        from_ty: TypeId,
        to_ty: TypeId,
        span: Span,
    ) -> EvalResult<Value> {
        let dst = self.loaded.unit(unit);
        match kind {
            ConvertKind::As => {
                let ns = v.as_duration().ok_or_else(|| Trap::Bug(format!("as(U) auf {}", v.kind_name())))?;
                let width = self.float_width(to_ty);
                // ns je Einheit = Faktor(U) * 1e9, fuer Zeiteinheiten ganzzahlig.
                let per = i128::from(dst.factor.num) * 1_000_000_000 / i128::from(dst.factor.den);
                let a = Value::float_from_int(width, i128::from(ns));
                let b = Value::float_from_int(width, per);
                arith::float_binary(BinaryOp::Div, &a, &b, span, self.tick)
            }
            ConvertKind::To => {
                let src_unit = match self.loaded.ty(from_ty) {
                    Type::Float { unit: Some(u), .. } => self.loaded.unit(*u),
                    Type::Int { .. } => return bug("Einheiten auf Ganzzahlen ab M6"),
                    _ => return bug("to(U) ohne Quelleinheit"),
                };
                let width = self.float_width(to_ty);
                if v.as_f64().is_none() {
                    return bug(format!("to(U) auf {}", v.kind_name()));
                }
                // Basiswert = (x + off_src) * f_src; Ergebnis = Basiswert / f_dst - off_dst
                let mut x = v;
                if let Some(off) = src_unit.affine_offset {
                    let o = rational(width, off.num, off.den);
                    x = arith::float_binary(BinaryOp::Add, &x, &o, span, self.tick)?;
                }
                let num = i128::from(src_unit.factor.num) * i128::from(dst.factor.den);
                let den = i128::from(src_unit.factor.den) * i128::from(dst.factor.num);
                let p = Value::float_from_int(width, num);
                let q = Value::float_from_int(width, den);
                x = arith::float_binary(BinaryOp::Mul, &x, &p, span, self.tick)?;
                x = arith::float_binary(BinaryOp::Div, &x, &q, span, self.tick)?;
                if let Some(off) = dst.affine_offset {
                    let o = rational(width, off.num, off.den);
                    x = arith::float_binary(BinaryOp::Sub, &x, &o, span, self.tick)?;
                }
                Ok(x)
            }
            ConvertKind::ToFloat => bug("to_float ab M6"),
        }
    }

    /// Eingefuegte Pruefung (M3-Knoten; die Semantik gilt ab M1).
    fn checked(&mut self, inner: &Expr, kind: &CheckedKind, span: Span) -> EvalResult<Value> {
        match kind {
            CheckedKind::Missing => match self.eval(inner)? {
                Value::Optional(Some(v)) => Ok(*v),
                Value::Optional(None) => Err(self.fault(FaultKind::MissingValue, "Wert fehlt", span)),
                Value::Result(Ok(v)) => Ok(*v),
                Value::Result(Err(e)) => {
                    let text = crate::format::display(&e, None, inner.ty, self);
                    Err(self.fault(FaultKind::MissingValue, format!("Fehlercode {text}"), span))
                }
                other => bug(format!("Missing-Pruefung auf {}", other.kind_name())),
            },
            CheckedKind::Range(r) => {
                let v = self.eval(inner)?;
                if in_range(&v, r) {
                    Ok(v)
                } else {
                    let text = crate::format::display(&v, None, inner.ty, self);
                    Err(self.fault(FaultKind::Range, format!("{text} ausserhalb der Range"), span))
                }
            }
            // Validitaet, Index, Division, Ueberlauf, Konversion, Shift: die
            // Operation selbst faultet (4.1); der Knoten ist Annotation.
            _ => self.eval(inner),
        }
    }

    /// Blockinstanz bauen (5.7): Parameter, dann Zustandsvariablen mit Initialwerten.
    pub fn instantiate_block(&mut self, block: takt_mir::BlockId, args: &[Value]) -> EvalResult<BlockState> {
        let def = &self.loaded.program.blocks[block.index()];
        if args.len() != def.params.len() {
            return bug(format!("Block `{}`: {} Argumente fuer {} Parameter", def.name, args.len(), def.params.len()));
        }
        self.frames.push(Frame { vars: args.to_vec(), func: None, block: Some(block) });
        for v in &def.state_vars {
            let value = match &v.init {
                Some(init) => self.eval(init),
                None => Ok(Value::default_for(v.ty, self.loaded.program)),
            };
            match value {
                Ok(value) => self.frames.last_mut().expect("Rahmen").vars.push(value),
                Err(e) => {
                    self.frames.pop();
                    return Err(e);
                }
            }
        }
        let frame = self.frames.pop().expect("Rahmen");
        Ok(BlockState { block, vars: frame.vars, stepped: false })
    }

    /// Ort einer Zuweisung als veraenderliche Referenz; Indizes werden vor dem
    /// Zugriff ausgewertet und geprueft (RangeFault).
    /// Schreibt an eine Stelle (9.2). Ein Byte in `bytes<N>` ist kein
    /// `Value` und braucht darum einen eigenen Weg; alles andere laeuft
    /// ueber `place_mut`.
    pub fn assign(&mut self, place: &Place, value: Value, span: Span) -> EvalResult<()> {
        if let Place::Index(base, index) = place {
            let i = self.eval_int(index)?;
            if matches!(self.place_kind(base, span)?, Some(PlaceKind::Bytes)) {
                let byte = match value {
                    Value::UInt(x) => u8::try_from(x).unwrap_or(0),
                    Value::Int(x) => u8::try_from(x).unwrap_or(0),
                    other => return bug(format!("Byte erwartet, {} gefunden", other.kind_name())),
                };
                let tick = self.tick;
                let Value::Bytes(b) = self.place_mut(base, span)? else { return bug("Bytes erwartet") };
                let len = b.len();
                let Ok(i) = usize::try_from(i) else {
                    return Err(Trap::Fault(Fault::new(FaultKind::Range, format!("Index {i} negativ"), span, tick)));
                };
                let Some(slot) = b.get_mut(i) else {
                    return Err(Trap::Fault(Fault::new(
                        FaultKind::Range,
                        format!("Index {i} ausserhalb 0..{}", len.saturating_sub(1)),
                        span,
                        tick,
                    )));
                };
                *slot = byte;
                return Ok(());
            }
        }
        *self.place_mut(place, span)? = value;
        Ok(())
    }

    /// Grobform des Werts an einer Stelle, ohne ihn auszuleihen.
    fn place_kind(&mut self, place: &Place, span: Span) -> EvalResult<Option<PlaceKind>> {
        Ok(match self.place_mut(place, span)? {
            Value::Bytes(_) => Some(PlaceKind::Bytes),
            _ => None,
        })
    }

    /// Loest eine Stelle zu einem veraenderbaren Wert auf (9.2).
    pub fn place_mut(&mut self, place: &Place, span: Span) -> EvalResult<&mut Value> {
        enum Step {
            Field(u32),
            Index(usize),
            Index2(usize, usize),
        }
        let mut steps = Vec::new();
        let mut cur = place;
        loop {
            match cur {
                Place::Var(_) | Place::Output(_) => break,
                Place::Field(b, f) => {
                    steps.push(Step::Field(*f));
                    cur = b;
                }
                Place::Index(b, i) => {
                    let i = self.eval_int(i)?;
                    if i < 0 {
                        return Err(self.fault(FaultKind::Range, format!("Index {i} negativ"), span));
                    }
                    steps.push(Step::Index(i as usize));
                    cur = b;
                }
                Place::Index2(b, r, c) => {
                    let r = self.eval_int(r)?;
                    let c = self.eval_int(c)?;
                    if r < 0 || c < 0 {
                        return Err(self.fault(FaultKind::Range, "Matrixindex negativ", span));
                    }
                    steps.push(Step::Index2(r as usize, c as usize));
                    cur = b;
                }
            }
        }
        let tick = self.tick;
        let mut v: &mut Value = match cur {
            Place::Var(v) => self.var_mut(*v)?,
            Place::Output(c) => self.outer.output_mut(*c)?,
            _ => unreachable!(),
        };
        for step in steps.into_iter().rev() {
            v = match (step, v) {
                (Step::Field(f), Value::Record(fields) | Value::Enum { fields, .. }) => {
                    fields.get_mut(f as usize).ok_or_else(|| Trap::Bug(format!("Feld {f} fehlt")))?
                }
                (Step::Index(i), Value::Array(x) | Value::Vec(x)) => {
                    let len = x.len();
                    x.get_mut(i).ok_or_else(|| {
                        Trap::Fault(Fault::new(
                            FaultKind::Range,
                            format!("Index {i} ausserhalb 0..{}", len.saturating_sub(1)),
                            span,
                            tick,
                        ))
                    })?
                }
                (Step::Index2(r, c), Value::Mat { rows, cols, data }) => {
                    if r >= *rows as usize || c >= *cols as usize {
                        return Err(Trap::Fault(Fault::new(
                            FaultKind::Range,
                            format!("Matrixindex ({r}, {c}) ausserhalb"),
                            span,
                            tick,
                        )));
                    }
                    &mut data[r * *cols as usize + c]
                }
                (_, other) => return bug(format!("Zuweisungsort in {}", other.kind_name())),
            };
        }
        Ok(v)
    }
}

/// Feld eines Records oder einer Variante.
pub fn field_of(v: Value, field: u32) -> EvalResult<Value> {
    match v {
        Value::Record(mut fields) | Value::Enum { mut fields, .. } => {
            if (field as usize) < fields.len() {
                Ok(fields.swap_remove(field as usize))
            } else {
                bug(format!("Feld {field} fehlt"))
            }
        }
        other => bug(format!("Feldzugriff auf {}", other.kind_name())),
    }
}

/// Liegt der Wert in der Range (Grenzen einschliesslich)?
pub fn in_range(v: &Value, r: &Range) -> bool {
    let (lo, hi) = (const_value(&r.lo), const_value(&r.hi));
    match (v, &lo, &hi) {
        (Value::Int(_) | Value::UInt(_), Value::Int(_) | Value::UInt(_), Value::Int(_) | Value::UInt(_)) => {
            let x = v.as_int().expect("int");
            x >= lo.as_int().expect("int") && x <= hi.as_int().expect("int")
        }
        // Bei `float32` wird in f32 verglichen: die Grenzen der Range stehen
        // als f64 in der MIR, und `0.1f32` ist als f64 groesser als `0.1f64`.
        // Ohne die Rundung waere die deklarierte Obergrenze in f32 nie
        // erreichbar, und ein Wert genau auf der Grenze faultete (3.4).
        (Value::F32(x), _, _) => {
            let x = f64::from(*x);
            lo.as_f64().is_some_and(|l| x >= f64::from(l as f32))
                && hi.as_f64().is_some_and(|h| x <= f64::from(h as f32))
        }
        (Value::F64(x), _, _) => {
            let x = *x;
            lo.as_f64().is_some_and(|l| x >= l) && hi.as_f64().is_some_and(|h| x <= h)
        }
        (Value::Duration(d), Value::Duration(l), Value::Duration(h)) => d >= l && d <= h,
        _ => true,
    }
}

fn const_value(c: &Const) -> Value {
    match c {
        Const::Int(i) => Value::Int(*i),
        Const::Float(f) => Value::F64(*f),
        Const::Duration(d) => Value::Duration(*d),
        Const::Bool(b) => Value::Bool(*b),
    }
}

fn rational(width: FloatWidth, num: i64, den: u64) -> Value {
    Value::float(width, num as f64 / den as f64)
}

/// Grobform eines Zuweisungsorts, soweit `assign` sie unterscheiden muss.
enum PlaceKind {
    /// `bytes<N>`: die Elemente sind Bytes, keine `Value`.
    Bytes,
}
