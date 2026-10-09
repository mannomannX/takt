//! Faults im Modell (5.3): Jeder Fault-Ausgang traegt seine Ursache. Die Art
//! steht je Maschine fuer den laufenden Tick im Zustand — die Zeile `fault`
//! eines Laufs (`Model::run`); Art, Meldung, Zeile und Tick des letzten
//! Faults (`last_fault`, `system::last_fault_value`) fuehrt nur eine
//! Maschine, die `last_fault` liest.

use takt_diag::Span;
use takt_mir::machine::FaultKind;
use takt_mir::pattern::Format;
use takt_mir::types::Type;
use takt_mir::{MachineId, TypeId};

use super::text::Text;
use super::value::V;
use super::{Cx, Enc, Env, Flow, R, no};
use crate::term::{Op, Term};

/// So viele Byte fasst `LastFault.message` (Prelude, `str<128>`).
const MESSAGE: i64 = 128;

/// Was ein Fault in `last_fault` hinterlaesst: seine Art, die Meldung einer
/// Anweisung (`check`, `expect`, `abort`) — ohne sie den Namen der Art —
/// und seine Zeile; der Tick ist der laufende.
#[derive(Clone, Debug)]
pub(super) struct Cause {
    kind: FaultKind,
    message: Option<Text>,
    line: i64,
}

impl Enc<'_> {
    /// Die Ursache eines Faults an `span`.
    pub(super) fn cause(&self, kind: FaultKind, span: Span) -> Cause {
        Cause { kind, message: None, line: i64::from(self.p.line_of(span)) }
    }

    /// Die Ursache eines Faults einer Anweisung mit ihrer Meldung (`render`);
    /// ohne Meldung `fallback`. Gerendert wird nur, wenn die Maschine
    /// `last_fault` liest.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn stated(
        &mut self,
        kind: FaultKind,
        message: Option<&Format>,
        fallback: &str,
        cx: &Cx<'_>,
        env: &Env,
        flow: &Flow,
        span: Span,
    ) -> R<Cause> {
        let mut cause = self.cause(kind, span);
        if cx.m.is_some_and(|m| self.reads_last_fault(m)) {
            cause.message = Some(match message {
                Some(f) => self.render(f, cx, env, flow)?,
                None => Text::literal(fallback),
            });
        }
        Ok(cause)
    }

    /// Die Maschinen, die `last_fault` lesen.
    pub(super) fn last_fault_readers(&self) -> Vec<MachineId> {
        (0..self.p.machines.len())
            .map(|i| MachineId(i as u32))
            .filter(|m| takt_mir::visit::reads_last_fault(self.machine(*m)))
            .collect()
    }

    pub(super) fn reads_last_fault(&self, m: MachineId) -> bool {
        self.last_fault.contains(&m)
    }

    /// Die Art des Faults der Maschine in diesem Tick als Nummer ([`code`]).
    pub(super) fn loc_fault(&self, m: MachineId) -> String {
        format!("s.{}.fault", self.machine(m).name)
    }

    pub(super) fn loc_last_fault(&self, m: MachineId) -> String {
        format!("s.{}.last_fault", self.machine(m).name)
    }

    /// Der Tick, in dem gerade gerechnet wird.
    pub(super) fn tick_index(&self) -> Term {
        Term::bin(Op::Div, self.now.clone(), Term::int(self.p.config.tick.max(1)))
    }

    /// Der Typ `LastFault` aus dem Prelude.
    pub(super) fn last_fault_type(&self, span: Span) -> R<TypeId> {
        let r = self.p.records.iter().position(|r| r.name == "LastFault");
        let ty =
            r.and_then(|r| self.p.types.list.iter().position(|t| *t == Type::Record(takt_mir::RecordId(r as u32))));
        match ty {
            Some(t) => Ok(TypeId(t as u32)),
            None => no("`last_fault` ohne Typ `LastFault`", span),
        }
    }

    /// `last_fault` vor dem ersten Fault: alles null.
    pub(super) fn last_fault_initial(&self, m: MachineId, env: &mut Env) -> R<()> {
        let ty = self.last_fault_type(Span::default())?;
        let zero = self.zero_of(ty, Span::default())?;
        self.init_loc(env, &self.loc_last_fault(m), ty, zero, Span::default())
    }

    /// Haelt in `out` fest, was der Fault `cause` hinterlaesst.
    pub(super) fn record_fault(&self, m: MachineId, cause: &Cause, out: &mut Env) -> R<()> {
        out.insert(self.loc_fault(m), Term::int(code(cause.kind)));
        if !self.reads_last_fault(m) {
            return Ok(());
        }
        let span = Span::default();
        let ty = self.last_fault_type(span)?;
        let Type::Record(r) = self.p.types.get(ty) else { return no("`LastFault`", span) };
        let fields = self.p.records[r.index()].fields.clone();
        let message = match &cause.message {
            Some(t) => t.truncate(MESSAGE),
            None => Text::literal(cause.kind.prelude_name()),
        };
        let mut parts = vec![
            self.fault_kind_value(cause.kind, fields.first().map_or(ty, |f| f.ty), span)?,
            self.text_value(&message, fields.get(1).map_or(ty, |f| f.ty), span)?,
            V::Leaf(Term::int(cause.line)),
            V::Leaf(self.tick_index()),
        ];
        parts.truncate(fields.len());
        for f in fields.iter().skip(parts.len()) {
            parts.push(self.zero_of(f.ty, span)?);
        }
        Enc::store(out, &self.loc_last_fault(m), &self.shape(ty, span)?, V::Node(parts));
        Ok(())
    }

    /// Kein Fault in diesem Tick: zu Beginn jedes Ticks und im Anfangszustand.
    pub(super) fn faults_cleared(&self, env: &mut Env) {
        for &m in &self.order {
            env.insert(self.loc_fault(m), Term::int(0));
        }
    }

    /// Die Art als Wert von `FaultKind` (`fault_kind_value`): die Variante
    /// nach ihrem Namen im Prelude, die Unterart als Feld.
    fn fault_kind_value(&self, kind: FaultKind, ty: TypeId, span: Span) -> R<V> {
        let Type::Enum(e) = self.p.types.get(ty) else { return no("`FaultKind`", span) };
        let name = kind.prelude_name();
        let variant = self.p.enums[e.index()].variants.iter().position(|v| v.name == name).unwrap_or(0);
        let fields = match kind {
            FaultKind::Arithmetic(k) => vec![V::Leaf(Term::int(k as i64))],
            FaultKind::Runtime(k) => vec![V::Leaf(Term::int(k as i64))],
            FaultKind::CheckFailed
            | FaultKind::Expect
            | FaultKind::Timeout
            | FaultKind::SensorFault
            | FaultKind::MissingValue
            | FaultKind::Range
            | FaultKind::StreamOverflow
            | FaultKind::Timing
            | FaultKind::ScheduleOverflow
            | FaultKind::Abort => Vec::new(),
        };
        self.variant(ty, variant as u32, fields, span)
    }
}

/// Die Nummer einer Art im Zustand: eins plus ihr Platz in `FaultKind::all`;
/// null heisst kein Fault.
fn code(kind: FaultKind) -> i64 {
    FaultKind::all().iter().position(|k| *k == kind).map_or(0, |i| i as i64 + 1)
}

/// Die Art zu einer Nummer im Zustand.
pub fn kind_of(code: i64) -> Option<FaultKind> {
    FaultKind::all().get(usize::try_from(code).ok()?.checked_sub(1)?).copied()
}
