//! Der Tickschritt einer Maschine (11.2, 9.4).
//!
//! 11.2: „Konfiguration als Pfad-Array fester Tiefe; Blattzustand =
//! Enum-Diskriminante; `switch` ueber die Blaetter."
//!
//! Der Schritt liest `conf`, springt in den Block des aktiven Blatts,
//! fuehrt dessen `loop:` aus und kehrt zurueck. Was er *nicht* tut, ist
//! ebenso wichtig: Er committet keine Outputs (das macht die Runtime,
//! 12.1) und er fuehrt keinen Fault aus (das macht die Abort-Phase, 5.4).

use takt_mir::machine::Machine;
use takt_mir::program::Program;

use crate::emit::Module;
use crate::expr::NotYet;
use crate::machine::{self, Role, StateStruct};
use crate::stmt::{Ctx, block};

/// Schreibt die vollstaendige Schrittfunktion einer Maschine.
///
/// Die Struktur folgt 11.2:
///
/// ```text
/// define void @m_step(ptr %0, ptr %1, ptr %2, ptr %3) {
///   %4 = load i8 aus conf[0]
///   switch i8 %4, label %unbekannt [ i8 0, label %m_ZUSTAND … ]
/// m_ZUSTAND:
///   … loop-Koerper …
///   br label %ende
/// fault_m:
///   pending setzen, ret
/// ende:
///   ret void
/// }
/// ```
///
/// `Err` heisst: Ein Koerper enthaelt etwas, das der Codegen noch nicht
/// senkt. Die Funktion entsteht dann nicht halb — eine halbe
/// Schrittfunktion waere schlimmer als keine, weil sie uebersetzt.
pub fn step_function(m: &Machine, st: &StateStruct, p: &Program, module: &mut Module) -> Result<(), NotYet> {
    let leaves = machine::leaves(m);
    if leaves.is_empty() {
        return Err(NotYet { what: "Maschine ohne Blattzustand" });
    }
    let mark = module.mark();
    match write_step(m, st, p, module, &leaves) {
        Ok(()) => Ok(()),
        Err(e) => {
            module.abort(mark);
            Err(e)
        }
    }
}

/// Der eigentliche Rumpf; `step_function` raeumt bei `Err` auf.
fn write_step(
    m: &Machine,
    st: &StateStruct,
    p: &Program,
    module: &mut Module,
    leaves: &[takt_mir::StateId],
) -> Result<(), NotYet> {
    machine::begin_step(m, module);

    // 11.2: `conf[0]` ist der aktive Zustand der obersten Ebene. Tiefere
    // Ebenen kommen mit den geschachtelten Zustaenden; der `switch` ueber
    // die Blaetter ist die Form, die 11.2 nennt.
    let state_ty = format!("%{}_state", m.name);
    let conf_i = st.index_of(Role::Conf, 0).ok_or(NotYet { what: "conf im Zustand" })?;
    let conf = module.inst(&format!("getelementptr inbounds {state_ty}, ptr %0, i32 0, i32 {conf_i}"));
    let slot = module.inst(&format!("getelementptr inbounds [{} x i8], ptr {conf}, i32 0, i32 0", st.depth));
    let cur = module.inst(&format!("load i8, ptr {slot}"));

    let end = format!("ende_{}", m.name);
    let arms: Vec<String> =
        leaves.iter().enumerate().map(|(i, id)| format!("i8 {i}, label %{}", machine::label_of(m, *id))).collect();
    // Der Default-Zweig geht ans Ende: Eine Konfiguration ausserhalb der
    // Blaetter kann nicht entstehen (der Zustandsraum ist statisch), und
    // `unreachable` waere hier die schaerfere, aber unbelegte Aussage —
    // 4.1 verlangt Totalitaet, nicht undefiniertes Verhalten.
    module.void_inst(&format!("switch i8 {cur}, label %{end} [ {} ]", arms.join(" ")));

    let mut ctx = Ctx::new(m, st, p);
    for id in leaves {
        module.label(&machine::label_of(m, *id));
        block(&m.states[id.index()].loop_block, &mut ctx, module)?;
        module.void_inst(&format!("br label %{end}"));
    }

    machine::fault_trampoline(m, st, module);
    module.label(&end);
    module.end(None);
    Ok(())
}
