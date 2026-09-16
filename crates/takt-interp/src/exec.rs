//! `exec(stmt, s, mode) -> (s', out)` (Referenz 9.2): Big-Step ueber die
//! Anweisungen mit Ausgang `Normal | Goto | Break | Return`; Faults sind
//! `Err`. Modus `Entry` macht `->` wirkungslos (Entry-Tick-Regel, 5.2).

use takt_diag::Span;
use takt_mir::expr::{Expr, ExprKind, StreamRef};
use takt_mir::machine::{FaultKind, Target};
use takt_mir::stmt::*;
use takt_mir::types::Type;

use crate::env::{CoverKind, Observation};
use crate::eval::Ctx;
use crate::format::render;
use crate::loaded::Loaded;
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
                self.assign(target, v, span)?;
                // 12.7: ein irreversibler Output zaehlt fuer die Abdeckung durch Szenarien.
                if let Place::Output(c) = target {
                    let channel = &self.loaded.program.channels[c.index()];
                    if channel.attrs.irreversible {
                        let name = channel.name.clone();
                        self.outer.cover(CoverKind::Irreversible, name);
                    }
                }
                Ok(Out::Normal)
            }
            StmtKind::Check { cond, message, confirm, target, kind, .. } => {
                let ok = self.eval_bool(cond)?;
                let site = format!("{} @{}", if *kind == CheckKind::Check { "check" } else { "expect" }, span.start);
                self.outer.cover(CoverKind::Check, site.clone());
                if !ok {
                    self.outer.cover(CoverKind::CheckFailed, site);
                }
                let fault_kind = match kind {
                    CheckKind::Check => FaultKind::CheckFailed,
                    CheckKind::Expect => FaultKind::Expect,
                };
                let failed = match confirm {
                    None => !ok,
                    Some(c) => {
                        let d = self.eval_duration(&c.duration)?;
                        let period = self.outer.period()?;
                        let index = self.loops.clone();
                        let viol = self.outer.viol(c.site, &index)?;
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
                self.loops.push(0);
                let result = (|| {
                    let mut i: i128 = 0;
                    while i < n {
                        *self.var_mut(*var)? = Value::Int(i as i64);
                        *self.loops.last_mut().expect("Schleife offen") = i as i64;
                        match self.exec_block(body, mode)? {
                            Out::Normal => {}
                            Out::Break => break,
                            out => return Ok(out),
                        }
                        i += 1;
                    }
                    Ok(Out::Normal)
                })();
                self.loops.pop();
                result
            }
            StmtKind::ForEach { vars, iter, body } => {
                // 8.7: ueber ein Stream-Fenster laeuft die Schleife ueber
                // Elemente, nicht ueber Werte; jedes betrachtete Element gilt
                // als konsumiert.
                if let Some(stream) = stream_of(self.loaded, iter) {
                    return self.for_window(vars, stream, body, mode);
                }
                let items: Vec<Value> = match self.eval(iter)? {
                    Value::Array(x) | Value::Vec(x) | Value::Samples(x) => x,
                    Value::Map(pairs) => pairs.into_iter().map(|(k, v)| Value::Array(vec![k, v])).collect(),
                    Value::Bytes(b) => b.into_iter().map(|x| Value::UInt(u64::from(x))).collect(),
                    other => return bug(format!("for ueber {}", other.kind_name())),
                };
                self.loops.push(0);
                let result = (|| {
                    for (index, item) in items.into_iter().enumerate() {
                        *self.loops.last_mut().expect("Schleife offen") = index as i64;
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
                })();
                self.loops.pop();
                result
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
            StmtKind::Send { stream, value, len_max } => {
                let v = self.eval(value)?;
                self.outer.send(*stream, v, *len_max, span)?;
                Ok(Out::Normal)
            }
            StmtKind::At { time, body } => {
                // 9.8: die rechten Seiten werden jetzt ausgewertet, die
                // Schreibvorgaenge gesammelt und nach `T` eingeplant.
                let t = self.eval_duration(time)?;
                for stmt in &body.stmts {
                    let StmtKind::Assign { target: Place::Output(c), value } = &stmt.kind else {
                        return bug("`at`-Block enthaelt mehr als Output-Zuweisungen");
                    };
                    let v = self.eval(value)?;
                    self.outer.schedule(*c, t, v, stmt.span)?;
                }
                Ok(Out::Normal)
            }
            StmtKind::Skip(s) => {
                // 8.6: `s.skip()` untersucht das ganze Fenster und verwirft
                // es; der Cursor rueckt hinter das letzte Element.
                if let Some(last) = self.outer.stream_window(*s)?.last() {
                    self.outer.stream_examined(*s, last.seq)?;
                }
                Ok(Out::Normal)
            }
            StmtKind::Cancel(c) => {
                self.outer.cancel(*c)?;
                Ok(Out::Normal)
            }
            StmtKind::Raise(sig) => {
                self.outer.raise(*sig)?;
                Ok(Out::Normal)
            }
            StmtKind::Job { handle, native, args } => {
                // 4.5: Die Argumente werden kopiert, das Ergebnis der reinen
                // Funktion steht fest; das Modell liefert es nach `duration`.
                let args = args.iter().map(|a| self.eval(a)).collect::<EvalResult<Vec<_>>>()?;
                let value = self.call_native(*native, args, span)?;
                let t0 = self.loaded.program.config.tick.max(1);
                let d = self.loaded.program.natives[native.index()].duration.unwrap_or(0).max(0);
                let ticks = d.saturating_add(t0 - 1) / t0;
                let due = self.tick.saturating_add(u64::try_from(ticks).unwrap_or(u64::MAX));
                self.outer.job_start(*handle, value, due)?;
                Ok(Out::Normal)
            }
            StmtKind::Every { period, counter, body } => {
                let d = self.eval_duration(period)?;
                let t = match self.outer.every_clock(*counter)? {
                    Value::Duration(t) => t,
                    other => return bug(format!("Uhr eines `every` ist {}", other.kind_name())),
                };
                let index = self.loops.clone();
                // Der Zaehler beginnt bei `d` (5.8): mit 0 lief der Block
                // schon im Eintritts-Tick, und ein Zustand, der kuerzer als
                // `d` aktiv ist, fuehrte ihn bei jedem Eintritt aus statt nie.
                let next = self.outer.every(*counter, &index, d)?;
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
                // Die Bedingung eines Alerts nennt das zu meldende Ereignis
                // (5.6); anders als bei `check` ist sie keine Invariante. Ein
                // ungueltiger Input laesst den Alert feuern, statt die
                // Steuerung zu beeinflussen (3.5).
                let (raw, invalid) = match self.eval(cond) {
                    Ok(Value::Bool(b)) => (b, false),
                    Ok(other) => return bug(format!("Alert-Bedingung ist {}", other.kind_name())),
                    Err(Trap::Bug(msg)) => return Err(Trap::Bug(msg)),
                    Err(Trap::Fault(_)) => (true, true),
                };
                let active = match confirm {
                    None => raw,
                    Some(c) => {
                        let d = self.eval_duration(&c.duration)?;
                        let period = self.outer.period()?;
                        let index = self.loops.clone();
                        let viol = self.outer.viol(c.site, &index)?;
                        if raw {
                            *viol = (*viol + period).min(d);
                            *viol >= d
                        } else {
                            *viol = 0;
                            false
                        }
                    }
                };
                let message = render(message, self);
                Observation::Alert { span, index: self.loops.clone(), active, message, invalid }
            }
            Observe::Measure { name, value } => {
                Observation::Measure { name: name.clone(), value: self.eval(value).ok(), ty: value.ty }
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
            // 3.9: alles oder nichts. Ein Teilanhang liesse einen halben
            // Rahmen im Puffer zurueck, den niemand als Fehler erkennt.
            Method::Append => {
                let cap = self.capacity(receiver, span)?;
                let src = args.into_iter().next();
                let target = self.place_mut(receiver, span)?;
                match (target, src) {
                    (Value::Vec(x), Some(Value::Vec(v))) => {
                        if x.len() + v.len() <= cap {
                            x.extend(v);
                            Ok(Value::Bool(true))
                        } else {
                            Ok(Value::Bool(false))
                        }
                    }
                    (Value::Bytes(b), Some(Value::Bytes(v))) => {
                        if b.len() + v.len() <= cap {
                            b.extend_from_slice(&v);
                            Ok(Value::Bool(true))
                        } else {
                            Ok(Value::Bool(false))
                        }
                    }
                    (other, _) => bug(format!("append auf {}", other.kind_name())),
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
        }
    }

    /// `for ev in s:` ueber das Fenster eines Stroms (8.7, 9.6). Die Schleife
    /// ist durch CAP beschraenkt; jedes betrachtete Element gilt als
    /// konsumiert, ein `break` laesst den Rest im Puffer.
    fn for_window(&mut self, vars: &ForVars, stream: StreamRef, body: &Block, mode: Mode) -> EvalResult<Out> {
        let ForVars::One(var) = vars else { return bug("`for` ueber ein Fenster bindet genau eine Variable") };
        let window = self.outer.stream_window(stream)?;
        self.loops.push(0);
        let result = (|| {
            for (index, element) in window.into_iter().enumerate() {
                *self.loops.last_mut().expect("Schleife offen") = index as i64;
                let value = self.outer.element_value(*var, &element, Vec::new())?;
                *self.var_mut(*var)? = value;
                self.outer.stream_examined(stream, element.seq)?;
                match self.exec_block(body, mode)? {
                    Out::Normal => {}
                    Out::Break => break,
                    out => return Ok(out),
                }
            }
            Ok(Out::Normal)
        })();
        self.loops.pop();
        result
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

/// Der Strom, ueber den eine Schleife laeuft; `None` bei einer gewoehnlichen
/// Sammlung.
fn stream_of(loaded: &Loaded<'_>, iter: &Expr) -> Option<StreamRef> {
    if !matches!(loaded.ty(iter.ty), Type::Stream(_)) {
        return None;
    }
    match &iter.kind {
        ExprKind::Input { channel, .. } => Some(StreamRef::Channel(*channel)),
        ExprKind::Stream(s) => Some(StreamRef::Internal(*s)),
        _ => None,
    }
}
