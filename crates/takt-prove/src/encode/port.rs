//! Registerports im Simulator (12.10, `system::port_read`, `port_write`):
//! Gelesen wird der `sim`-Output `mmio/ADR/r`, den ein Modell stellt, mit
//! Unit-Delay — im Modell sein Wert am Tick-Anfang —, ohne Modell der
//! Default. Ein Schreibvorgang, auch auf ein Feld, setzt den gelesenen
//! Record neu zusammen und stellt ihn als Element in den Eingabestrom
//! `mmio/ADR/w` wie ein `send`, das nicht faultet (FB-475): Ein voller
//! Strom verliert es und zaehlt den Ueberlauf.

use takt_diag::Span;
use takt_mir::expr::StreamRef;
use takt_mir::pattern::Address;
use takt_mir::program::{Binding, Direction};
use takt_mir::types::Type;
use takt_mir::{ChannelId, PortId};

use super::value::V;
use super::{Enc, Env, Flow, R, no};
use crate::term::Term;

impl Enc<'_> {
    /// Der Kanal `mmio/ADR/{side}` eines Ports in Richtung `dir`.
    fn port_channel(&self, p: PortId, side: &str, dir: Direction) -> Option<ChannelId> {
        let want = Address::simple(&format!("mmio/{:#x}/{side}", self.p.ports[p.index()].address));
        self.p
            .channels
            .iter()
            .position(|c| c.dir == dir && matches!(&c.binding, Binding::Sim(a) if *a == want))
            .map(|i| ChannelId(i as u32))
    }

    /// Der Wert eines Ports (`port_read`).
    pub(super) fn port_value(&mut self, p: PortId, span: Span) -> R<V> {
        let ty = self.p.ports[p.index()].ty;
        let Some(c) = self.port_channel(p, "r", Direction::Output) else { return self.zero_of(ty, span) };
        if matches!(self.p.types.get(self.p.channels[c.index()].ty), Type::Stream(_)) {
            return no("Port, dessen Modell ein Strom ist", span);
        }
        let (shape, at) = (self.shape(ty, span)?, self.loc_out(c));
        let committed = std::mem::take(&mut self.committed);
        let v = self.load(&committed, &at, &shape, span);
        self.committed = committed;
        v
    }

    /// Ein Schreibvorgang auf einen Port (`port_write`): der ganze Record als
    /// Element des Schreibstroms, in kanonischer Form, wenn dieser Bytes
    /// traegt — ein `send`, das nicht faultet: Was der Strom abweist, zaehlt
    /// als `overflowed` (`Image::queue_port_write`).
    pub(super) fn port_write(&mut self, p: PortId, v: V, env: &mut Env, flow: &Flow, span: Span) -> R<()> {
        let Some(c) = self.port_channel(p, "w", Direction::Input) else { return Ok(()) };
        let Type::Stream(elem) = self.p.types.get(self.p.channels[c.index()].ty) else { return Ok(()) };
        let (elem, ty) = (*elem, self.p.ports[p.index()].ty);
        let value = match self.p.types.get(elem) {
            Type::Bytes { cap } => {
                let cap = *cap as usize;
                let form = self.canonical(ty, v, span)?;
                if form.bytes.len() > cap {
                    return no("Port breiter als die Elemente seines Schreibstroms", span);
                }
                let mut bytes = form.bytes;
                bytes.resize(cap, Term::int(0));
                V::Node(std::iter::once(form.len).chain(bytes).map(V::Leaf).collect())
            }
            _ if elem == ty => v,
            _ => return no("Schreibstrom eines Ports mit Elementen dieser Art", span),
        };
        let stream = self.stream_index(StreamRef::Channel(c), span)?;
        self.offer(stream, value, env, &flow.alive, span)
    }
}
