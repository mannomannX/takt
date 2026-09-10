//! `exec(stmt, s, mode) -> (s', out)` (Referenz 9.2): Big-Step ueber die
//! Anweisungen mit Ausgang `Normal | Goto | Break | Return`; Faults sind
//! `Err`. Modus `Entry` macht `->` wirkungslos (Entry-Tick-Regel, 5.2).

use takt_diag::Span;
use takt_mir::machine::{FaultKind, Target};
use takt_mir::stmt::*;

use crate::env::Observation;
use crate::eval::Ctx;
use crate::format::render;
use crate::value::{EvalResult, Fault, Trap, Value, bug};

/// Modus der Ausfuehrung (9.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Gewoehnlicher Schritt.
    Run,
    /// Entry-Tick: `->` ist wirkungslos, Handler laufen nicht.
    Entry,
}

/// Ausgang einer Anweisung.
#[derive(Clone, Debug, PartialEq)]
pub enum Out {
    /// Weiter.
    Normal,
    /// `-> q` (nur im Modus `Run`).
    Goto(Target),
    /// `break`, von `for` konsumiert.
    Break,
    /// `return` in einer Funktion.
    Return(Value),
}

impl Ctx<'_, '_> {
    /// `exec(s1; s2)`: Folge, endet beim ersten Ausgang ungleich `Normal`.
    pub fn exec_block(&mut self, b: &Block, mode: Mode) -> EvalResult<Out> {
        for s in &b.stmts {
            match self.exec(s, mode)? {
                Out::Normal => {}
                out => return Ok(out),
            }
        }
        Ok(Out::Normal)
    }

    /// Eine Anweisung.
    pub fn exec(&mut self, s: &Stmt, mode: Mode) -> EvalResult<Out> {
        let span = s.span;
        match &s.kind {
            StmtKind::Assign { target, value } => {
                let v = self.eval(value)?;
                *self.place_mut(target, span)? = v;
                Ok(Out::Normal)
            }
            StmtKind::Check { cond, message, confirm, target, kind, .. } => {
                let ok = self.eval_bool(cond)?;
                let fault_kind = match kind {
                    CheckKind::Check => FaultKind::CheckFailed,
                    CheckKind::Expect => FaultKind::Expect,
                };
                let failed = match confirm {
                    None => !ok,
                    Some(c) => {
                        let d = self.eval_duration(&c.duration)?;
                        let period = self.outer.period()?;
                        let viol = self.outer.viol(c.site)?;
                        if ok {
                            *viol = 0;
                            false
                        } else if *viol + period >= d {
                            *viol = 0;
                            true
                        } else {
                            *viol += period;
                            false
                        }
                    }
                };
                if !failed {
                    return Ok(Out::Normal);
                }
                let text = match message {
                    Some(m) => render(m, self),
                    None => String::from("check verletzt"),
                };
                let mut fault = Fault::new(fault_kind, text, span, self.tick);
                fault.target = *target;
                Err(Trap::Fault(fault))
            }
            StmtKind::Goto(t) => Ok(if mode == Mode::Run { Out::Goto(*t) } else { Out::Normal }),
            StmtKind::Abort { message } => {
                let text = match message {
                    Some(m) => render(m, self),
                    None => String::from("abort"),
                };
                self.outer.abort()?;
                Err(Trap::Fault(Fault::new(FaultKind::Abort, text, span, self.tick)))
            }
            StmtKind::If { cond, then, otherwise } => {
                if self.eval_bool(cond)? {
                    self.exec_block(then, mode)
                } else {
                    self.exec_block(otherwise, mode)
                }
            }
            StmtKind::ForRange { var, count, body } => {
                let n = self.eval_int(count)?;
                let mut i: i128 = 0;
                while i < n {
                    *self.var_mut(*var)? = Value::Int(i as i64);
                    match self.exec_block(body, mode)? {
                        Out::Normal => {}
                        Out::Break => break,
                        out => return Ok(out),
                    }
                    i += 1;
                }
                Ok(Out::Normal)
            }
            StmtKind::ForEach { vars, iter, body } => {
                let items: Vec<Value> = match self.eval(iter)? {
                    Value::Array(x) | Value::Vec(x) | Value::Samples(x) => x,
                    Value::Map(pairs) => pairs.into_iter().map(|(k, v)| Value::Array(vec![k, v])).collect(),
                    Value::Bytes(b) => b.into_iter().map(|x| Value::UInt(u64::from(x))).collect(),
                    other => return bug(format!("for ueber {}", other.kind_name())),
                };
                for item in items {
                    match vars {
                        ForVars::One(v) => *self.var_mut(*v)? = item,
                        ForVars::Pair(k, v) => match item {
                            Value::Array(mut pair) if pair.len() == 2 => {
                                let value = pair.pop().expect("Paar");
                                let key = pair.pop().expect("Paar");
                                *self.var_mut(*k)? = key;
                                *self.var_mut(*v)? = value;
                            }
                            other => return bug(format!("Paar erwartet, {} gefunden", other.kind_name())),
                        },
                    }
                    match self.exec_block(body, mode)? {
                        Out::Normal => {}
                        Out::Break => break,
                        out => return Ok(out),
                    }
                }
                Ok(Out::Normal)
            }
            StmtKind::Match { subject, arms } => {
                let v = self.eval(subject)?;
                for arm in arms {
                    match &arm.pattern {
                        ArmPattern::Wild => return self.exec_block(&arm.body, mode),
                        ArmPattern::Variant { variant, fields } => {
                            let matched = match &v {
                                Value::Enum { variant: got, .. } => got == variant,
                                Value::Result(Ok(_)) => *variant == 0,
                                Value::Result(Err(_)) => *variant == 1,
                                Value::Optional(Some(_)) => *variant == 0,
                                Value::Optional(None) => *variant == 1,
                                other => return bug(format!("match ueber {}", other.kind_name())),
                            };
                            if !matched {
                                continue;
                            }
                            let payload: Vec<Value> = match &v {
                                Value::Enum { fields, .. } => fields.clone(),
                                Value::Result(Ok(x)) | Value::Result(Err(x)) => vec![(**x).clone()],
                                Value::Optional(Some(x)) => vec![(**x).clone()],
                                _ => Vec::new(),
                            };
                            for (slot, value) in fields.iter().zip(payload) {
                                *self.var_mut(*slot)? = value;
                            }
                            return self.exec_block(&arm.body, mode);
                        }
                        ArmPattern::Values(values) => {
                            for cv in values {
                                let lo = self.eval(&cv.lo)?;
                                let hit = match &cv.hi {
                                    None => lo == v,
                                    Some(hi) => {
                                        let hi = self.eval(hi)?;
                                        v.compare(&lo).is_some_and(|o| o.is_ge())
                                            && v.compare(&hi).is_some_and(|o| o.is_le())
                                    }
                                };
                                if hit {
                                    return self.exec_block(&arm.body, mode);
                                }
                            }
                        }
                    }
                }
                bug("match ohne passenden Zweig (nicht erschoepfend)")
            }
            StmtKind::Return(e) => Ok(Out::Return(self.eval(e)?)),
            StmtKind::Send { .. } => bug("send ab M2"),
            StmtKind::At { .. } => bug("at ab M2"),
            StmtKind::Cancel(_) => bug("cancel ab M2"),
            StmtKind::Raise(sig) => {
                self.outer.raise(*sig)?;
                Ok(Out::Normal)
            }
            StmtKind::Job { .. } => bug("job ab M6"),
            StmtKind::Every { period, counter, body } => {
                let d = self.eval_duration(period)?;
                let t = match self.outer.builtin(takt_mir::expr::Builtin::TimeInState)? {
                    Value::Duration(t) => t,
                    other => return bug(format!("time_in_state ist {}", other.kind_name())),
                };
                let next = self.outer.every(*counter)?;
                if t >= *next {
                    *next += d;
                    self.exec_block(body, mode)
                } else {
                    Ok(Out::Normal)
                }
            }
            StmtKind::Break => Ok(Out::Break),
            StmtKind::Observe(o) => {
                self.observe(o, span)?;
                Ok(Out::Normal)
            }
            StmtKind::Arm { .. } => bug("arm ab M8"),
            StmtKind::MethodCall { target, receiver, method, args } => {
                let args = args.iter().map(|a| self.eval(a)).collect::<EvalResult<Vec<_>>>()?;
                let result = self.method_call(receiver, *method, args, span)?;
                if let Some(t) = target {
                    *self.place_mut(t, span)? = result;
                }
                Ok(Out::Normal)
            }
            StmtKind::Pass => Ok(Out::Normal),
        }
    }

    /// Beobachtung (5.6, 13.5): faultet nie; ungueltige Werte werden gemeldet.
    fn observe(&mut self, o: &Observe, span: Span) -> EvalResult<()> {
        let event = match o {
            Observe::Log(f) => Observation::Log(render(f, self)),
            Observe::Alert { cond, message, confirm } => {
                let ok = matches!(self.eval(cond), Ok(Value::Bool(true)));
                let violated = match confirm {
                    None => !ok,
                    Some(c) => {
                        let d = self.eval_duration(&c.duration)?;
                        let period = self.outer.period()?;
                        let viol = self.outer.viol(c.site)?;
                        if ok {
                            *viol = 0;
                            false
                        } else {
                            *viol = (*viol + period).min(d);
                            *viol >= d
                        }
                    }
                };
                let message = render(message, self);
                Observation::Alert { span, violated, message }
            }
            Observe::Measure { name, value } => {
                Observation::Measure { name: name.clone(), value: self.eval(value).ok() }
            }
            Observe::Verify { cond, message, req } => {
                let ok = matches!(self.eval(cond), Ok(Value::Bool(true)));
                Observation::Verify { span, ok, message: render(message, self), req: req.clone() }
            }
            Observe::Verdict { pass, message } => {
                Observation::Verdict { pass: *pass, message: message.as_ref().map(|m| render(m, self)) }
            }
        };
        self.outer.observe(event)
    }

    /// Mutierende Methode auf einer Stelle (5.7, 3.9).
    fn method_call(&mut self, receiver: &Place, method: Method, args: Vec<Value>, span: Span) -> EvalResult<Value> {
        let program = self.loaded.program;
        match method {
            Method::Step | Method::Reset | Method::Block(_) => {
                let mut instance = match std::mem::replace(self.place_mut(receiver, span)?, Value::Handle) {
                    Value::Block(b) => b,
                    other => {
                        let kind = other.kind_name();
                        *self.place_mut(receiver, span)? = other;
                        return bug(format!("Blockmethode auf {kind}"));
                    }
                };
                let result = match method {
                    Method::Step => {
                        let def = &program.blocks[instance.block.index()];
                        match def.step {
                            Some(step) if !instance.stepped => {
                                instance.stepped = true;
                                self.call_block_method(&mut instance, step, args, span)
                            }
                            Some(_) => bug(format!("`{}`: step zweimal in einer Aktivierung", def.name)),
                            None => bug(format!("`{}` hat kein step", def.name)),
                        }
                    }
                    Method::Reset => {
                        let def = &program.blocks[instance.block.index()];
                        let params: Vec<Value> = instance.vars.iter().take(def.params.len()).cloned().collect();
                        self.instantiate_block(instance.block, &params).map(|fresh| {
                            *instance = fresh;
                            Value::Bool(true)
                        })
                    }
                    Method::Block(f) => self.call_block_method(&mut instance, f, args, span),
                    _ => unreachable!(),
                };
                *self.place_mut(receiver, span)? = Value::Block(instance);
                result
            }
            Method::Push => {
                let cap = self.capacity(receiver, span)?;
                let target = self.place_mut(receiver, span)?;
                match (target, args.into_iter().next()) {
                    (Value::Vec(x), Some(v)) => Ok(Value::Bool(push_bounded(x, v, cap))),
                    (Value::Bytes(b), Some(Value::UInt(v))) => {
                        if b.len() < cap {
                            b.push(v as u8);
                            Ok(Value::Bool(true))
                        } else {
                            Ok(Value::Bool(false))
                        }
                    }
                    (other, _) => bug(format!("push auf {}", other.kind_name())),
                }
            }
            Method::Clear => match self.place_mut(receiver, span)? {
                Value::Vec(x) => {
                    x.clear();
                    Ok(Value::Bool(true))
                }
                Value::Bytes(b) => {
                    b.clear();
                    Ok(Value::Bool(true))
                }
                Value::Map(m) => {
                    m.clear();
                    Ok(Value::Bool(true))
                }
                other => bug(format!("clear auf {}", other.kind_name())),
            },
            Method::Insert | Method::Remove => bug("map ab M6"),
            Method::Skip => bug("Streams ab M2"),
        }
    }

    /// Kapazitaet einer Sammlung an einer Stelle: aus dem Typ der Variablen,
    /// bei Feldern und Elementen aus dem Typ entlang des Pfads.
    fn capacity(&mut self, receiver: &Place, _span: Span) -> EvalResult<usize> {
        let ty = self.place_type(receiver)?;
        Ok(match self.loaded.ty(ty) {
            takt_mir::types::Type::Vec { cap, .. }
            | takt_mir::types::Type::Bytes { cap }
            | takt_mir::types::Type::Map { cap, .. } => *cap as usize,
            _ => usize::MAX,
        })
    }

    /// Statischer Typ einer Stelle.
    fn place_type(&mut self, place: &Place) -> EvalResult<takt_mir::TypeId> {
        use takt_mir::types::Type;
        let p = self.loaded.program;
        match place {
            Place::Var(v) => self.var_type(*v),
            Place::Output(c) => Ok(p.channels[c.index()].ty),
            Place::Field(b, f) => {
                let base = self.place_type(b)?;
                match self.loaded.ty(base) {
                    Type::Record(r) => Ok(p.records[r.index()].fields[*f as usize].ty),
                    other => bug(format!("Feld auf Typ {other:?}")),
                }
            }
            Place::Index(b, _) => {
                let base = self.place_type(b)?;
                match self.loaded.ty(base) {
                    Type::Array { elem, .. } | Type::Vec { elem, .. } => Ok(*elem),
                    other => bug(format!("Index auf Typ {other:?}")),
                }
            }
            Place::Index2(b, _, _) => self.place_type(b),
        }
    }
}

fn push_bounded(x: &mut Vec<Value>, v: Value, cap: usize) -> bool {
    if x.len() < cap {
        x.push(v);
        true
    } else {
        false
    }
}
