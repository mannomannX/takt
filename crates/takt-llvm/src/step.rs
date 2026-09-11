//! Der Tickschritt einer Maschine (11.2, 9.4, 5.2).
//!
//! 11.2: „Konfiguration als Pfad-Array fester Tiefe; Blattzustand =
//! Enum-Diskriminante; `switch` ueber die Blaetter."
//!
//! Der Schritt liest `conf`, springt in den Block des aktiven Blatts,
//! fuehrt dessen `loop:` aus, prueft die Uebergaenge und kehrt zurueck.
//! Was er *nicht* tut, ist ebenso wichtig: Er committet keine Outputs (das
//! macht die Runtime, 12.1) und er fuehrt keinen Fault aus (das macht die
//! Abort-Phase, 5.4).
//!
//! **Die Reihenfolge im Zustand folgt 5.2.** Erst `loop:`, dann die
//! Uebergaenge — ein `check` im `loop:` wirkt also, bevor ein `when` den
//! Zustand verlassen kann. Umgekehrt waere der Zustand schon gewechselt,
//! wenn die Invariante bricht, und die Meldung nennte den falschen.

use takt_mir::StateId;
use takt_mir::machine::{Guard, Machine, Target, TransTrigger, Transition};
use takt_mir::program::Program;

use crate::emit::Module;
use crate::expr::{NotYet, lower as lower_expr};
use crate::machine::{self, Role, StateStruct};
use crate::stmt::{Ctx, block};

/// Schreibt die vollstaendige Schrittfunktion einer Maschine.
///
/// Die Struktur folgt 11.2:
///
/// ```text
/// define void @m_step(ptr %0, ptr %1, ptr %2, ptr %3) {
///   %4 = load i8 aus conf[0]
///   switch i8 %4, label %ende [ i8 0, label %m_ZUSTAND … ]
/// m_ZUSTAND:
///   … loop-Koerper …
///   … Uebergaenge: Guard pruefen, conf setzen, t_in_state = 0 …
///   br label %ende
/// fault_m:
///   pending setzen, ret
/// ende:
///   t_in_state += 1, ret void
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
    leaves: &[StateId],
) -> Result<(), NotYet> {
    machine::begin_step(m, module);

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
    for (i, id) in leaves.iter().enumerate() {
        module.label(&machine::label_of(m, *id));
        let state = &m.states[id.index()];
        // 5.2: erst der `loop:`-Koerper …
        block(&state.loop_block, &mut ctx, module)?;
        // … dann die Uebergaenge, in Quelltextreihenfolge (5.2: der erste
        // passende gewinnt).
        transitions(&state.transitions, i, leaves, &mut ctx, module, &end, &slot)?;
        module.void_inst(&format!("br label %{end}"));
    }

    machine::fault_trampoline(m, st, module);
    module.label(&end);
    // `t_in_state` zaehlt die Ticks im aktiven Zustand (5.2, 11.2). Ein
    // Uebergang hat ihn auf 0 gesetzt; hier waechst er um einen Tick.
    if let Some(t_i) = st.index_of(Role::TimeInState, 0) {
        let base = module.inst(&format!("getelementptr inbounds {state_ty}, ptr %0, i32 0, i32 {t_i}"));
        let cell = module.inst(&format!("getelementptr inbounds [{} x i64], ptr {base}, i32 0, i32 0", st.depth));
        let old = module.inst(&format!("load i64, ptr {cell}"));
        let new = module.inst(&format!("add i64 {old}, 1"));
        module.void_inst(&format!("store i64 {new}, ptr {cell}"));
    }
    module.end(None);
    Ok(())
}

/// Die Uebergaenge eines Zustands (5.2).
///
/// Sie werden in Quelltextreihenfolge geprueft; der erste, dessen Guard
/// haelt, gewinnt und verlaesst den Zustand. 8.7 verlangt dieselbe
/// Reihenfolge fuer Handler — der Quelltext ist die Prioritaet, damit sie
/// dasteht, statt hergeleitet werden zu muessen.
fn transitions(
    list: &[Transition],
    from: usize,
    leaves: &[StateId],
    ctx: &mut Ctx<'_>,
    m: &mut Module,
    end: &str,
    conf_slot: &crate::emit::Reg,
) -> Result<(), NotYet> {
    for (n, t) in list.iter().enumerate() {
        let c = match &t.trigger {
            TransTrigger::When(Guard::Expr(cond)) => {
                let vars = ctx.vars();
                lower_expr(cond, ctx.program, m, &vars)?
            }
            TransTrigger::After(d) => after(d, ctx, m)?,
            // Muster-Guards brauchen den Fensterzugriff (8.7); er kommt mit
            // den Stroemen.
            TransTrigger::When(_) => return Err(NotYet { what: "Uebergang mit Muster-Guard" }),
        };
        let name = &ctx.machine.name;
        let (take, skip) = (format!("uebergang{from}_{n}_{name}"), format!("bleibt{from}_{n}_{name}"));
        m.void_inst(&format!("br i1 {}, label %{take}, label %{skip}", c.value));
        m.label(&take);
        let Target::State(to) = t.target else { return Err(NotYet { what: "Uebergangsziel" }) };
        let Some(index) = leaves.iter().position(|l| *l == to) else {
            // Ein Ziel, das kein Blatt ist, hat einen `initial`-Pfad
            // hinunter (5.2); der entsteht mit der Verschachtelung.
            return Err(NotYet { what: "Uebergang in einen zusammengesetzten Zustand" });
        };
        // 5.2 gibt die Reihenfolge vor: `exit:` des verlassenen Zustands,
        // dann der Aktionsblock des Uebergangs (Modus ENTRY), dann
        // `enter:` des betretenen. Wer sie vertauscht, laesst `enter:` auf
        // einem Zustand laufen, den `exit:` noch aufraeumt.
        let from_state = &ctx.machine.states[leaves[from].index()];
        block(&from_state.exit.clone(), ctx, m)?;
        block(&t.actions, ctx, m)?;
        let to_state = &ctx.machine.states[to.index()];
        block(&to_state.enter.clone(), ctx, m)?;
        m.void_inst(&format!("store i8 {index}, ptr {conf_slot}"));
        reset_time(ctx, m);
        m.void_inst(&format!("br label %{end}"));
        m.label(&skip);
    }
    Ok(())
}

/// `t_in_state = 0` beim Eintritt in einen Zustand (5.2).
///
/// Ohne das Zuruecksetzen misst `after d` die Zeit seit dem Start der
/// Maschine statt seit dem Eintritt — der haeufigste Fehler, den eine
/// handgeschriebene Zustandsmaschine macht.
fn reset_time(ctx: &Ctx<'_>, m: &mut Module) {
    let Some(t_i) = ctx.state.index_of(Role::TimeInState, 0) else { return };
    let state_ty = format!("%{}_state", ctx.machine.name);
    let base = m.inst(&format!("getelementptr inbounds {state_ty}, ptr %0, i32 0, i32 {t_i}"));
    let cell = m.inst(&format!("getelementptr inbounds [{} x i64], ptr {base}, i32 0, i32 0", ctx.state.depth));
    // -1, weil das Ende des Schritts gleich um 1 erhoeht: Der erste Tick
    // im neuen Zustand hat `t_in_state == 0`.
    m.void_inst(&format!("store i64 -1, ptr {cell}"));
}

/// Schreibt die Eintrittsfunktion einer Maschine (9.4).
///
/// 9.4: Der Anfangszustand wird betreten, *bevor* der erste Tick laeuft —
/// sein `enter:` gehoert darum nicht in den Tickschritt, sondern in eine
/// eigene Funktion, die die Runtime einmal ruft. Stuende es im Schritt,
/// liefe es in jedem Tick.
pub fn init_function(m: &Machine, st: &StateStruct, p: &Program, module: &mut Module) -> Result<(), NotYet> {
    let leaves = machine::leaves(m);
    let Some(index) = leaves.iter().position(|l| *l == m.initial) else {
        return Err(NotYet { what: "Anfangszustand ist kein Blatt" });
    };
    let mark = module.mark();
    let ptr = crate::ty::LlvmType::Ptr;
    module.begin(
        &format!("{}_init", m.name),
        &crate::ty::LlvmType::Void,
        &[ptr.clone(), ptr.clone(), ptr.clone(), ptr],
    );
    let state_ty = format!("%{}_state", m.name);
    let Some(conf_i) = st.index_of(Role::Conf, 0) else {
        module.abort(mark);
        return Err(NotYet { what: "conf im Zustand" });
    };
    let conf = module.inst(&format!("getelementptr inbounds {state_ty}, ptr %0, i32 0, i32 {conf_i}"));
    let slot = module.inst(&format!("getelementptr inbounds [{} x i8], ptr {conf}, i32 0, i32 0", st.depth));
    module.void_inst(&format!("store i8 {index}, ptr {slot}"));

    let mut ctx = Ctx::new(m, st, p);
    let enter = m.states[m.initial.index()].enter.clone();
    if let Err(e) = block(&enter, &mut ctx, module) {
        module.abort(mark);
        return Err(e);
    }
    module.end(None);
    Ok(())
}

/// `after d` als Ausloeser (5.2, 7.1).
///
/// Die Bedingung ist die des Interpreters, Zeichen fuer Zeichen:
///
/// ```text
/// elapsed = t_in_state * periode
/// feuert  = elapsed > 0 and elapsed >= d
/// ```
///
/// `elapsed > 0` ist nicht ueberfluessig: 7.1 sagt, `after` feuert „im
/// ersten Aktivierungs-Tick mit `time_in_state >= d`, nie im Entry-Tick".
/// Ohne den Vergleich feuerte `after 0 ms` schon beim Betreten, und eine
/// Sequenz liefe in einem Tick durch alle Schritte.
///
/// Die Dauer muss ein Literal sein: Ein berechneter Ausdruck haette einen
/// Wert je Tick, und die Schedulability (7.2) koennte ihn nicht
/// beschraenken.
fn after(d: &takt_mir::expr::Expr, ctx: &Ctx<'_>, m: &mut Module) -> Result<crate::expr::Lowered, NotYet> {
    let takt_mir::expr::ExprKind::Duration(ns) = d.kind else {
        return Err(NotYet { what: "`after` mit berechneter Dauer" });
    };
    let Some(t_i) = ctx.state.index_of(Role::TimeInState, 0) else {
        return Err(NotYet { what: "t_in_state im Zustand" });
    };
    let state_ty = format!("%{}_state", ctx.machine.name);
    let base = m.inst(&format!("getelementptr inbounds {state_ty}, ptr %0, i32 0, i32 {t_i}"));
    let cell = m.inst(&format!("getelementptr inbounds [{} x i64], ptr {base}, i32 0, i32 0", ctx.state.depth));
    let ticks = m.inst(&format!("load i64, ptr {cell}"));
    // Die Periode der Maschine in Nanosekunden steht fest (7.2): `period`
    // Basis-Ticks mal T0.
    let period_ns = i64::from(ctx.machine.period.max(1)).saturating_mul(ctx.program.config.tick);
    let elapsed = m.inst(&format!("mul i64 {ticks}, {period_ns}"));
    let positive = m.inst(&format!("icmp sgt i64 {elapsed}, 0"));
    let reached = m.inst(&format!("icmp sge i64 {elapsed}, {ns}"));
    let both = m.inst(&format!("and i1 {positive}, {reached}"));
    Ok(crate::expr::Lowered { value: both.to_string(), ty: crate::ty::LlvmType::Int(1) })
}
