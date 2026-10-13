//! Was ein Stimulus sein kann (12.5; FB-688, FB-689): die Eingaben, die das
//! Produkt erleben und aufzeichnen kann. Der Interpreter lehnt jede andere
//! Zeile beim Lesen ab, das Modell waehlt nur unter diesen (13.3), und der
//! Testwirt hat fuer jede einen Weg. So meint Satz 9.4.4 fuer jeden
//! Ausfuehrer dieselben Input-Stroeme, und jeder Pfad des Solvers ist ein
//! Lauf, den es geben kann.
//!
//! Ob ein Pruefstand eine Eingabe auch herbeifuehren kann, ist seine Sache,
//! nicht die der Sprache: Eine Job-Fertigstellung erlebt das Produkt, auch
//! wo ein Testwirt sie nicht erzwingt. Die Scheibe einer Maschine
//! (Maschinen-Replay, 12.5) ist kein Stimulus eines Laufs.

use crate::machine::RuntimeKind;
use crate::pattern::Address;
use crate::program::{Binding, Channel, Direction, OverrunPolicy, Program};

/// Ein Ereignis von aussen, das an einer Tickgrenze wirkt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// Ein Operator-Abort (5.4).
    Abort,
    /// Ein Runtime-Fault (5.3, 7.3, 12.6).
    Runtime(RuntimeKind),
    /// Die Aenderung eines Tunables (8.4).
    Tune,
}

/// Liefert der Rand von aussen an `c`? Nur an einen Input mit `hw`-Bindung
/// (8.1, 8.3): Was von aussen kommt, erreicht das Produkt ueber den Treiber
/// einer Adresse (8.10). Einen Input mit `sim` speist das Programm selbst
/// (der Schreibstrom eines Ports, 12.10), einer ohne Bindung hat keine
/// Quelle, und einen Kanal des Geraets `sys`, den der Kern stellt, liefert
/// kein Treiber (13.3).
pub fn input(c: &Channel) -> Result<(), String> {
    if c.dir != Direction::Input {
        return Err(format!("`{}` ist kein Input", c.name));
    }
    match &c.binding {
        Binding::Hw(a) if crate::sys::channel(&a.text()).is_some_and(|s| s.core) => {
            Err(format!("`{}` stellt der Kern, kein Treiber (13.3)", a.text()))
        }
        Binding::Hw(_) => Ok(()),
        Binding::Sim(_) => Err(format!(
            "`{}` hat eine `sim`-Bindung; von aussen liefert nur der Treiber einer `hw`-Adresse (8.3)",
            c.name
        )),
        Binding::None => Err(format!("`{}` hat keine Bindung und darum keine Quelle von aussen (8.1, 8.3)", c.name)),
    }
}

/// Ist `c` der Schreibstrom eines Ports (12.10): ein Input `sim("mmio/ADR/w")`
/// zu einem `port … @ mmio(ADR)`? Nur dort steht `sim` an einem Input (8.1).
pub fn port_write_stream(p: &Program, c: &Channel) -> bool {
    let Binding::Sim(a) = &c.binding else { return false };
    c.dir == Direction::Input
        && p.ports.iter().any(|port| *a == Address::simple(&format!("mmio/{:#x}/w", port.address)))
}

/// Hat ein Runtime-Fault der Art `kind` im Produkt eine Quelle (5.3)?
/// `output` ist der Ausgang eines `Driver`-Faults. Ein Ueberlauf faultet nur
/// unter `overrun = fault` (7.3), `Driver` meldet nur der Treiber eines
/// Ausgangs mit `hw`-Bindung (12.6 Zeile 6), und `Node` kommt nur von einem
/// Knoten (12.9).
pub fn runtime(p: &Program, kind: RuntimeKind, output: Option<&Channel>) -> Result<(), String> {
    match (kind, output) {
        (RuntimeKind::Overrun, _) if p.config.overrun == OverrunPolicy::Alert => {
            Err("unter `overrun = alert` faultet ein Ueberlauf nicht (7.3)".into())
        }
        (RuntimeKind::Driver, None) => Err("`runtime Driver` nennt den Ausgang".into()),
        (RuntimeKind::Driver, Some(o)) if o.dir != Direction::Output || !matches!(o.binding, Binding::Hw(_)) => {
            Err(format!("`{}` hat keinen Treiber, der einen Schreibvorgang meldet (12.6 Zeile 6)", o.name))
        }
        (RuntimeKind::Node, _) if p.nodes.is_empty() => {
            Err("ein Programm ohne Knoten kennt `Runtime(Node)` nicht (12.9)".into())
        }
        _ => Ok(()),
    }
}

/// Der erste Tick, in dem `event` wirken kann. Tick 0 ist der
/// Anfangszustand (1.5, 9.4): Ein Abort und ein Runtime-Fault wirken vor dem
/// Schritt einer Grenze (12.11), ein Tunable ab einer Grenze nach Tick 0
/// (8.4), und ein Ueberlauf meldet den Tick davor, der ein Schritt sein muss
/// (7.3).
pub fn first_tick(event: Event) -> u64 {
    match event {
        Event::Runtime(RuntimeKind::Overrun) => 2,
        Event::Abort | Event::Runtime(_) | Event::Tune => 1,
    }
}

/// Kann `event` im Tick `tick` kommen ([`first_tick`])?
pub fn at(event: Event, tick: u64) -> Result<(), String> {
    let first = first_tick(event);
    if tick >= first {
        return Ok(());
    }
    let what = match event {
        Event::Abort => "ein Operator-Abort wirkt vor dem Schritt einer Grenze (5.4, 12.11)".to_string(),
        Event::Runtime(RuntimeKind::Overrun) => "ein Ueberlauf meldet den Schritt des Ticks davor (7.3)".to_string(),
        Event::Runtime(kind) => format!("`Runtime({kind:?})` wirkt vor dem Schritt einer Grenze (5.4, 12.11)"),
        Event::Tune => "ein Tunable gilt ab einer Grenze nach Tick 0 (8.4)".to_string(),
    };
    Err(format!("erst ab Tick {first}: {what}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(name: &str, dir: Direction, binding: Binding) -> Channel {
        let mut c = crate::sample::full_program().channels.remove(0);
        c.name = name.into();
        c.dir = dir;
        c.binding = binding;
        c
    }

    /// **Von aussen nur ueber `hw`** (8.1, 8.3, 12.5; FB-688): ein Input mit
    /// `hw`-Bindung ja; mit `sim`, ohne Bindung, ein Ausgang und der Kanal,
    /// den der Kern stellt, nicht.
    #[test]
    fn only_a_hw_input_receives_from_outside() {
        assert!(input(&channel("p", Direction::Input, Binding::Hw(Address::simple("daq1/ai0")))).is_ok());
        let sim = input(&channel("a", Direction::Input, Binding::Sim(Address::simple("i/a")))).expect_err("sim");
        assert!(sim.contains("`sim`-Bindung"), "{sim}");
        assert!(input(&channel("n", Direction::Input, Binding::None)).is_err());
        assert!(input(&channel("o", Direction::Output, Binding::Hw(Address::simple("o/o")))).is_err());
        let core = Binding::Hw(Address::simple(crate::sys::OUTER_FAULT));
        assert!(input(&channel("f", Direction::Input, core)).is_err_and(|e| e.contains("Kern")));
    }

    /// **Ein Runtime-Fault braucht eine Quelle** (5.3, 7.3, 12.6, 12.9;
    /// FB-689): `Node` gibt es ohne Knoten nicht, `Overrun` nicht unter
    /// `alert`, `Driver` nur fuer einen Ausgang mit `hw`-Bindung; und keiner
    /// in Tick 0, ein Ueberlauf erst ab Tick 2.
    #[test]
    fn a_runtime_fault_needs_a_source() {
        let mut p = crate::sample::full_program();
        p.nodes.clear();
        assert!(runtime(&p, RuntimeKind::Node, None).is_err_and(|e| e.contains("ohne Knoten")));
        assert!(runtime(&p, RuntimeKind::Hardware, None).is_ok());
        assert!(runtime(&p, RuntimeKind::Overrun, None).is_ok());
        p.config.overrun = OverrunPolicy::Alert;
        assert!(runtime(&p, RuntimeKind::Overrun, None).is_err());
        let hw = channel("o", Direction::Output, Binding::Hw(Address::simple("o/o")));
        let sim = channel("s", Direction::Output, Binding::Sim(Address::simple("o/s")));
        assert!(runtime(&p, RuntimeKind::Driver, Some(&hw)).is_ok());
        assert!(runtime(&p, RuntimeKind::Driver, Some(&sim)).is_err());
        assert!(runtime(&p, RuntimeKind::Driver, None).is_err());
        assert!(at(Event::Abort, 0).is_err() && at(Event::Abort, 1).is_ok());
        assert!(at(Event::Tune, 0).is_err() && at(Event::Runtime(RuntimeKind::Hardware), 1).is_ok());
        assert!(at(Event::Runtime(RuntimeKind::Overrun), 1).is_err());
        assert!(at(Event::Runtime(RuntimeKind::Overrun), 2).is_ok());
    }
}
