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

    let state_ty = format!("%{}_state", crate::fns::sanitized(&m.name));
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
        ctx.leaf = Some(*id);
        // 5.2: Aktiv ist ein *Pfad*, nicht ein Zustand. Die `loop:`-Bloecke
        // laufen von der Maschine abwaerts bis zum Blatt — ein `check` auf
        // einer Zwischenebene ist die Invariante *aller* Zustaende darunter,
        // und wer nur das Blatt ausfuehrt, laesst sie fallen.
        block(&m.loop_block.clone(), &mut ctx, module)?;
        let pfad = machine::path_to(m, *id);
        for anc in &pfad {
            block(&m.states[anc.index()].loop_block.clone(), &mut ctx, module)?;
        }
        // 8.7: Die Handler verarbeiten das Fenster ihres Stroms. Sie
        // laufen nach den `loop:`-Bloecken, weil ein `check` dort die
        // Invariante des Zustands ist — sie gilt, bevor ein Ereignis sie
        // stoeren kann.
        let mut handler: Vec<takt_mir::machine::Handler> = m.handlers.clone();
        for anc in &pfad {
            handler.extend(m.states[anc.index()].handlers.iter().cloned());
        }
        dispatch(&handler, &mut ctx, module, &end)?;
        // Dann die Uebergaenge, vom Blatt aufwaerts: Der innerste Zustand
        // entscheidet zuerst (5.2), und innerhalb einer Ebene gewinnt der
        // erste passende in Quelltextreihenfolge.
        for anc in pfad.iter().rev() {
            let list = m.states[anc.index()].transitions.clone();
            transitions(&list, i, leaves, &mut ctx, module, &end, &slot)?;
        }
        module.void_inst(&format!("br label %{end}"));
        // 5.2 Regel 5: Ein Fault fuehrt sofort zum Fault-Ziel des
        // innersten Zustands, der eines deklariert (Fault-Wald, 5.3). Das
        // Ziel wird betreten und im Entry-Modus ausgefuehrt — damit
        // stehen die Outputs am Commit des Ticks auf seinen Werten.
        //
        // Der Trampolin steht je Blatt, weil das Ziel am Blatt haengt.
        fault_path(st, *id, &Ziel { leaves, end: &end, conf: &slot }, &mut ctx, module)?;
    }

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
    for t in list {
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
        // Die Marke muss je *erzeugter* Verzweigung eindeutig sein, nicht
        // je Zustand: Ein Blatt fuehrt auch die Uebergaenge seiner
        // Vorfahren aus (5.2), und zwei Ebenen haetten sonst dieselbe.
        let id = ctx.next_label();
        let name = &ctx.machine.name;
        let (take, skip) = (format!("uebergang{id}_{name}"), format!("bleibt{id}_{name}"));
        m.void_inst(&format!("br i1 {}, label %{take}, label %{skip}", c.value));
        m.label(&take);
        // 5.3 und 6.2: Ein Uebergang zeigt nicht immer auf einen Zustand.
        // Die beiden anderen Ziele gehen verschiedene Wege, und der
        // Unterschied ist der Fault selbst.
        let to = match t.target {
            Target::State(to) => to,
            // Der Timeout einer Sequenz (6.2) *ist* ein Fault: Er wird
            // vorgemerkt und nimmt dann den Fault-Pfad des Blatts —
            // denselben, den ein gescheiterter `check` nimmt. Der Pfad
            // endet in `end`, hier kommt nichts nach.
            Target::Fault(kind) => {
                pending(ctx, m, kind);
                m.void_inst(&format!("br label %fault_{}_{}", ctx.machine.name, leaves[from].index()));
                m.label(&skip);
                continue;
            }
            // `-> FAULTED` (5.3) ist kein Fault, sondern ein Ziel: Die
            // Konfiguration wird leer, kein Nutzercode laeuft mehr, und
            // die Outputs stehen auf `safe`. Der Interpreter setzt hier
            // keinen `last_fault`, also tut es der Codegen auch nicht.
            Target::Faulted => {
                leave_configuration(ctx, m, leaves.len());
                safe_outputs(ctx, m)?;
                m.void_inst(&format!("br label %{end}"));
                m.label(&skip);
                continue;
            }
        };
        // 5.2: Ein Uebergang auf einen zusammengesetzten Zustand betritt
        // dessen `initial`-Kind, und das rekursiv bis zu einem Blatt.
        let leaf = machine::initial_leaf(ctx.machine, to).ok_or(NotYet { what: "Zielzustand ohne `initial`" })?;
        let Some(index) = leaves.iter().position(|l| *l == leaf) else {
            return Err(NotYet { what: "Zielblatt" });
        };
        // 5.2 gibt die Reihenfolge vor: `exit:` des verlassenen Zustands,
        // dann der Aktionsblock des Uebergangs (Modus ENTRY), dann
        // `enter:` des betretenen. Wer sie vertauscht, laesst `enter:` auf
        // einem Zustand laufen, den `exit:` noch aufraeumt.
        // 5.2: `exit:` laeuft vom verlassenen Blatt aufwaerts bis unter
        // den gemeinsamen Vorfahren, `enter:` von dort abwaerts bis zum
        // neuen Blatt. Wer nur Blatt und Ziel nimmt, laesst die
        // Zwischenebenen aus — und ein `enter:` auf einer Zwischenebene
        // ist genau die Stelle, an der ein Ablauf seine Vorbedingung
        // herstellt.
        for id in machine::exiting(ctx.machine, leaves[from], leaf) {
            block(&ctx.machine.states[id.index()].exit.clone(), ctx, m)?;
        }
        block(&t.actions, ctx, m)?;
        for id in machine::entering(ctx.machine, leaves[from], leaf) {
            block(&ctx.machine.states[id.index()].enter.clone(), ctx, m)?;
        }
        m.void_inst(&format!("store i8 {index}, ptr {conf_slot}"));
        reset_time(ctx, m);
        // 5.2 Regel 4 (Entry-Tick): Die `loop:`-Bloecke der neu betretenen
        // Zustaende laufen noch in diesem Tick — die darueberliegenden
        // liefen bereits. `check`s wirken, `-> ZIEL` ist wirkungslos, und
        // `on`-Handler laufen nicht (das Fenster ist leer).
        //
        // Ohne sie erreichte ein Zustand seine Invarianten einen Tick zu
        // spaet, und die Outputs des Ticks stuenden auf den Werten des
        // alten Zustands.
        for id in machine::entering(ctx.machine, leaves[from], leaf) {
            block(&ctx.machine.states[id.index()].loop_block.clone(), ctx, m)?;
        }
        m.void_inst(&format!("br label %{end}"));
        m.label(&skip);
    }
    Ok(())
}

/// Setzt die Outputs der Maschine auf ihren `safe`-Wert (5.3).
///
/// In `FAULTED` laeuft kein Nutzercode mehr, und niemand schreibt die
/// Outputs — ohne diesen Schritt behielten sie den letzten Wert des
/// verlassenen Zustands. Genau das soll `safe` verhindern: Ein
/// Ventil, das offen stand, bliebe offen.
///
/// Der Interpreter tut dasselbe (`safe_outputs`), und nur die Outputs
/// *dieser* Maschine: Ein Fault einer Maschine stellt nicht die Anlage
/// still, sondern ihren eigenen Wirkungsbereich (5.4).
fn safe_outputs(ctx: &mut Ctx<'_>, m: &mut Module) -> Result<(), NotYet> {
    let eigene: Vec<(takt_mir::ChannelId, takt_mir::expr::Expr)> = ctx
        .program
        .channels
        .iter()
        .enumerate()
        .filter(|(_, c)| c.owner == Some(takt_mir::MachineId(ctx.machine_index)))
        .filter_map(|(i, c)| c.attrs.safe.clone().map(|e| (takt_mir::ChannelId(i as u32), e)))
        .collect();
    for (c, safe) in eigene {
        let vars = ctx.vars();
        let value = crate::expr::lower(&safe, ctx.program, m, &vars)?;
        let off = crate::image::latch_offset(c, ctx.program).ok_or(NotYet { what: "Versatz im Latch" })?;
        let ptr = m.inst(&format!("getelementptr inbounds i8, ptr %3, i64 {off}"));
        m.void_inst(&format!("store {} {}, ptr {ptr}", value.ty, value.value));
    }
    Ok(())
}

/// Merkt einen Fault vor, bevor der Fault-Pfad ihn aufnimmt (5.4).
///
/// `pending` ist `{ i1 gueltig, i32 Art, i32 Ursprung }`. Der Fault-Pfad
/// setzt das Flag selbst; hier kommt die *Art* dazu, weil nur der
/// Uebergang sie kennt — ein Timeout ist ein anderer Fault als ein
/// gescheiterter `check`, und die Abort-Phase (5.4) reicht ihn weiter.
fn pending(ctx: &Ctx<'_>, m: &mut Module, kind: takt_mir::machine::FaultKind) {
    let Some(i) = ctx.state.index_of(Role::Pending, 0) else { return };
    let state_ty = format!("%{}_state", crate::fns::sanitized(&ctx.machine.name));
    let field = m.inst(&format!("getelementptr inbounds {state_ty}, ptr %0, i32 0, i32 {i}"));
    let art = m.inst(&format!("getelementptr inbounds {{ i1, i32, i32 }}, ptr {field}, i32 0, i32 1"));
    m.void_inst(&format!("store i32 {}, ptr {art}", fault_code(kind)));
}

/// Die Fault-Art als Zahl, in der Reihenfolge von `FaultKind` (5.3).
///
/// Die Runtime liest sie aus `pending`; die Zahlen sind darum Teil der
/// ABI und stehen neben `abi.rs`, nicht im Code verstreut.
fn fault_code(kind: takt_mir::machine::FaultKind) -> u32 {
    use takt_mir::machine::FaultKind as F;
    match kind {
        F::CheckFailed => 0,
        F::Expect => 1,
        F::Timeout => 2,
        F::SensorFault => 3,
        F::MissingValue => 4,
        F::Arithmetic(_) => 5,
        F::Range => 6,
        F::StreamOverflow => 7,
        F::Timing => 8,
        F::ScheduleOverflow => 9,
        F::JobOverflow => 10,
        F::Abort => 11,
        F::Runtime(_) => 12,
    }
}

/// `-> FAULTED`: die Konfiguration wird leer (5.3, 9.3).
///
/// Es gibt keinen Zustand mehr, in dem Code laeuft. Der Interpreter
/// setzt `conf` auf die leere Folge; im erzeugten Code steht dafuer der
/// Index hinter dem letzten Blatt — der `switch` der Schrittfunktion
/// trifft ihn nicht, und damit laeuft nichts mehr.
fn leave_configuration(ctx: &Ctx<'_>, m: &mut Module, leaves: usize) {
    let Some(conf_i) = ctx.state.index_of(Role::Conf, 0) else { return };
    let state_ty = format!("%{}_state", crate::fns::sanitized(&ctx.machine.name));
    let base = m.inst(&format!("getelementptr inbounds {state_ty}, ptr %0, i32 0, i32 {conf_i}"));
    let cell = m.inst(&format!("getelementptr inbounds [{} x i8], ptr {base}, i32 0, i32 0", ctx.state.depth));
    // Der Index hinter dem letzten Blatt: Der `switch` der
    // Schrittfunktion kennt nur 0..leaves und trifft ihn nicht.
    m.void_inst(&format!("store i8 {leaves}, ptr {cell}"));
    reset_time(ctx, m);
}

/// `t_in_state = 0` beim Eintritt in einen Zustand (5.2).
///
/// Ohne das Zuruecksetzen misst `after d` die Zeit seit dem Start der
/// Maschine statt seit dem Eintritt — der haeufigste Fehler, den eine
/// handgeschriebene Zustandsmaschine macht.
fn reset_time(ctx: &Ctx<'_>, m: &mut Module) {
    let Some(t_i) = ctx.state.index_of(Role::TimeInState, 0) else { return };
    let state_ty = format!("%{}_state", crate::fns::sanitized(&ctx.machine.name));
    let base = m.inst(&format!("getelementptr inbounds {state_ty}, ptr %0, i32 0, i32 {t_i}"));
    let cell = m.inst(&format!("getelementptr inbounds [{} x i64], ptr {base}, i32 0, i32 0", ctx.state.depth));
    // 0, wie im Interpreter (`enter_state`): Ein Zustand, der im Tick k
    // betreten wird, liest dort `t_in_state == 0` — der Entry-Modus
    // (5.2 Regel 4) laeuft noch in diesem Tick und sieht die Null.
    //
    // Die Erhoehung am Ende des Schritts macht daraus 1 fuer den
    // naechsten Tick. Ein `-1` hier haette den Entry-Modus -1 lesen
    // lassen und jede `after`-Frist um einen Tick verschoben.
    m.void_inst(&format!("store i64 0, ptr {cell}"));
}

/// Schreibt die Eintrittsfunktion einer Maschine (9.4).
///
/// 9.4: Der Anfangszustand wird betreten, *bevor* der erste Tick laeuft —
/// sein `enter:` gehoert darum nicht in den Tickschritt, sondern in eine
/// eigene Funktion, die die Runtime einmal ruft. Stuende es im Schritt,
/// liefe es in jedem Tick.
pub fn init_function(m: &Machine, st: &StateStruct, p: &Program, module: &mut Module) -> Result<(), NotYet> {
    let leaves = machine::leaves(m);
    // 5.2: Auch der Anfangszustand kann zusammengesetzt sein; betreten
    // wird sein `initial`-Pfad bis zum Blatt.
    let leaf = machine::initial_leaf(m, m.initial).ok_or(NotYet { what: "Anfangszustand ohne `initial`" })?;
    let Some(index) = leaves.iter().position(|l| *l == leaf) else {
        return Err(NotYet { what: "Anfangsblatt" });
    };
    let mark = module.mark();
    let ptr = crate::ty::LlvmType::Ptr;
    module.begin(
        &format!("{}_init", m.name),
        &crate::ty::LlvmType::Void,
        &[ptr.clone(), ptr.clone(), ptr.clone(), ptr],
    );
    let state_ty = format!("%{}_state", crate::fns::sanitized(&m.name));
    let Some(conf_i) = st.index_of(Role::Conf, 0) else {
        module.abort(mark);
        return Err(NotYet { what: "conf im Zustand" });
    };
    let conf = module.inst(&format!("getelementptr inbounds {state_ty}, ptr %0, i32 0, i32 {conf_i}"));
    let slot = module.inst(&format!("getelementptr inbounds [{} x i8], ptr {conf}, i32 0, i32 0", st.depth));
    module.void_inst(&format!("store i8 {index}, ptr {slot}"));

    let mut ctx = Ctx::new(m, st, p);
    ctx.leaf = Some(leaf);
    // 9.4: `s0` sind die Anfangswerte der Variablen. Sie stehen *vor*
    // jedem `enter:`, weil ein `enter:`-Block sie schon lesen darf (1.4)
    // — und ohne sie stuende dort die Null, die der Speicher mitbringt.
    for (i, v) in m.vars.iter().enumerate() {
        let Some(init) = v.init.clone() else { continue };
        let id = takt_mir::VarId(i as u32);
        // Eine Blockinstanz wird nicht zugewiesen; ihr Zustand entsteht
        // aus den Initialwerten des Blocks (5.7).
        if machine::instance_block(m, id).is_some() {
            continue;
        }
        let vars = ctx.vars();
        let value = match crate::expr::lower(&init, p, module, &vars) {
            Ok(v) => v,
            Err(e) => {
                module.abort(mark);
                return Err(e);
            }
        };
        let Some(ptr) = ctx.field(Role::Var, i, module) else {
            module.abort(mark);
            return Err(NotYet { what: "Variable im Zustand" });
        };
        module.void_inst(&format!("store {} {}, ptr {ptr}", value.ty, value.value));
    }
    // Die ganze Kette von der Wurzel bis zum Blatt wird betreten (5.2).
    for id in machine::path_to(m, leaf) {
        let enter = m.states[id.index()].enter.clone();
        if let Err(e) = block(&enter, &mut ctx, module) {
            module.abort(mark);
            return Err(e);
        }
    }
    // 5.2 Regel 4: Der Anfangszustand laeuft im Tick 0 im Entry-Modus —
    // „wie eine Maschine bei Tick 0" (1012). Seine `check`s wirken also
    // schon dort, und ein Fault fuehrt vor dem ersten Commit zum
    // Fault-Ziel. Ohne das stuenden die Outputs des Ticks 0 auf den
    // Werten eines Zustands, den die Maschine bereits verlassen hat.
    let kette: Vec<takt_mir::StateId> = machine::path_to(m, leaf);
    let mut koerper = vec![m.loop_block.clone()];
    koerper.extend(kette.iter().map(|id| m.states[id.index()].loop_block.clone()));
    let ende = format!("init_ende_{}", m.name);
    for b in &koerper {
        if let Err(e) = block(b, &mut ctx, module) {
            module.abort(mark);
            return Err(e);
        }
    }
    module.void_inst(&format!("br label %{ende}"));
    // Der Fault-Pfad des Anfangszustands: Ein `check`, der schon im
    // Tick 0 scheitert, fuehrt zum Fault-Ziel (5.2 Regel 5). Ohne ihn
    // spraenge der Zweig ins Leere — die Marke steht nur im Schritt.
    if let Err(e) = fault_path(st, leaf, &Ziel { leaves: &leaves, end: &ende, conf: &slot }, &mut ctx, module) {
        module.abort(mark);
        return Err(e);
    }
    module.label(&ende);
    // 9.4: Tick 0 schreibt den Zaehler fort „wie am Ende jedes Ticks"
    // (`System::init` ruft `advance_counters`). Ohne das misst der
    // erzeugte Code eine Frist um einen Tick zu lang: Der Interpreter
    // steht zu Beginn von Tick 1 bei `t_in_state == 1`, der Code bei 0,
    // und `after 30 ms` feuert bei 10 ms Tick erst in Tick 4 statt 3.
    advance_time(&ctx, module);
    module.end(None);
    Ok(())
}

/// Schreibt `t_in_state` um einen Tick fort (9.4).
///
/// Der Zaehler misst die Ticks *seit* dem Eintritt; er waechst am Ende
/// jedes Ticks, in dem die Maschine aktiv war — und am Ende der
/// Initialisierung, weil Tick 0 dazugehoert.
fn advance_time(ctx: &Ctx<'_>, m: &mut Module) {
    let Some(t_i) = ctx.state.index_of(Role::TimeInState, 0) else { return };
    let state_ty = format!("%{}_state", crate::fns::sanitized(&ctx.machine.name));
    let base = m.inst(&format!("getelementptr inbounds {state_ty}, ptr %0, i32 0, i32 {t_i}"));
    let cell = m.inst(&format!("getelementptr inbounds [{} x i64], ptr {base}, i32 0, i32 0", ctx.state.depth));
    let now = m.inst(&format!("load i64, ptr {cell}"));
    let next = m.inst(&format!("add i64 {now}, 1"));
    m.void_inst(&format!("store i64 {next}, ptr {cell}"));
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
/// Die Dauer ist jeder Ausdruck vom Typ `Duration`; die Grammatik sagt
/// es so (`duration_expr := expr`), und der Interpreter wertet ihn aus.
fn after(d: &takt_mir::expr::Expr, ctx: &Ctx<'_>, m: &mut Module) -> Result<crate::expr::Lowered, NotYet> {
    // Ein Literal steht schon zur Uebersetzungszeit fest und braucht
    // keine Anweisung; alles andere wird gesenkt wie jeder Ausdruck —
    // ein Maschinenparameter (5.8) ebenso wie eine Variable.
    //
    // **Warum keine Beschraenkung auf Konstanten.** Der Codegen verlangte
    // frueher ein Literal oder einen `param`, mit Verweis auf die
    // Schedulability (7.2). Die begrenzt aber die *Kosten je
    // Aktivierung*, nicht die Fristen: Eine `after`-Dauer geht in die
    // Budgetrechnung gar nicht ein. Der Interpreter wertet jeden
    // `Duration`-Ausdruck aus, und die Grammatik erlaubt ihn — eine
    // engere Regel im Codegen hiesse, dass dasselbe Programm auf zwei
    // Wegen verschieden ausfaellt.
    let vars = ctx.vars();
    let frist = match d.kind {
        takt_mir::expr::ExprKind::Duration(ns) => {
            crate::expr::Lowered { value: ns.to_string(), ty: crate::ty::LlvmType::Int(64) }
        }
        _ => crate::expr::lower(d, ctx.program, m, &vars)?,
    };
    let Some(t_i) = ctx.state.index_of(Role::TimeInState, 0) else {
        return Err(NotYet { what: "t_in_state im Zustand" });
    };
    let state_ty = format!("%{}_state", crate::fns::sanitized(&ctx.machine.name));
    let base = m.inst(&format!("getelementptr inbounds {state_ty}, ptr %0, i32 0, i32 {t_i}"));
    let cell = m.inst(&format!("getelementptr inbounds [{} x i64], ptr {base}, i32 0, i32 0", ctx.state.depth));
    let ticks = m.inst(&format!("load i64, ptr {cell}"));
    // Die Periode der Maschine in Nanosekunden steht fest (7.2): `period`
    // Basis-Ticks mal T0.
    let period_ns = i64::from(ctx.machine.period.max(1)).saturating_mul(ctx.program.config.tick);
    let elapsed = m.inst(&format!("mul i64 {ticks}, {period_ns}"));
    let positive = m.inst(&format!("icmp sgt i64 {elapsed}, 0"));
    let reached = m.inst(&format!("icmp sge i64 {elapsed}, {}", frist.value));
    let both = m.inst(&format!("and i1 {positive}, {reached}"));
    Ok(crate::expr::Lowered { value: both.to_string(), ty: crate::ty::LlvmType::Int(1) })
}

/// Der Handler-Durchlauf eines Zustands (8.7, 9.6).
///
/// Die Form folgt dem Interpreter (`dispatch`): Stroeme in
/// Deklarationsreihenfolge, je Strom das Fenster in `seq`-Reihenfolge,
/// und je Element der erste passende Handler. Auch ein Element, auf das
/// kein Handler passt, gilt als untersucht — sonst saehe die Maschine es
/// im naechsten Tick wieder.
///
/// **Warum eine Schleife und keine abgerollte Folge.** Die Fenstergroesse
/// ist zur Uebersetzungszeit nicht bekannt (sie haengt an der Lieferung),
/// nur ihre Schranke: `CAP`. 4.1 verlangt eine Schranke, nicht eine feste
/// Zahl — und `CAP` Durchlaeufe abzurollen waere bei einem Ring von 256
/// Elementen unbrauchbar.
fn dispatch(
    handlers: &[takt_mir::machine::Handler],
    ctx: &mut Ctx<'_>,
    m: &mut Module,
    end: &str,
) -> Result<(), NotYet> {
    if handlers.is_empty() {
        return Ok(());
    }
    // Stroeme in Deklarationsreihenfolge; jede Ebene sieht dasselbe
    // Fenster (8.7).
    let mut streams: Vec<takt_mir::expr::StreamRef> = Vec::new();
    for h in handlers {
        if !streams.contains(&h.stream) {
            streams.push(h.stream);
        }
    }
    for stream in streams {
        let cursor = cursor_index(ctx, stream).ok_or(NotYet { what: "Cursor eines Stroms" })?;
        let sid = stream_id(stream).ok_or(NotYet { what: "Strom ohne feste Nummer" })?;
        let k = ctx.next_label();
        let state_ty = format!("%{}_state", crate::fns::sanitized(&ctx.machine.name));
        let cur_ptr = m.inst(&format!("getelementptr inbounds {state_ty}, ptr %0, i32 0, i32 {cursor}"));
        let cur = m.inst(&format!("load i64, ptr {cur_ptr}"));
        let n = m.inst(&format!("call i32 @{}(i32 {sid}, i64 {cur})", crate::stream::Streams::COUNT));
        // Der Zaehler laeuft ueber das Fenster; seine Schranke ist `n`.
        let i_ptr = m.inst("alloca i32");
        m.void_inst(&format!("store i32 0, ptr {i_ptr}"));
        // 9.6: `examined` merkt sich die hoechste untersuchte Nummer;
        // daraus wird am Ende `cur[s, m] = examined + 1`. Der Anfangswert
        // `-1` heisst „nichts untersucht" — dann bleibt der Cursor stehen.
        let ex_ptr = m.inst("alloca i64");
        m.void_inst(&format!("store i64 -1, ptr {ex_ptr}"));
        let (kopf, rumpf, ende) = (format!("strom{k}"), format!("strom{k}_rumpf"), format!("strom{k}_ende"));
        m.void_inst(&format!("br label %{kopf}"));
        m.label(&kopf);
        let i = m.inst(&format!("load i32, ptr {i_ptr}"));
        let weiter = m.inst(&format!("icmp slt i32 {i}, {n}"));
        m.void_inst(&format!("br i1 {weiter}, label %{rumpf}, label %{ende}"));
        m.label(&rumpf);
        // Das Element wird in die Bindung geschrieben; ohne Bindung in
        // einen Scratch, weil `takt_stream_at` einen Platz braucht.
        let hs: Vec<&takt_mir::machine::Handler> = handlers.iter().filter(|h| h.stream == stream).collect();
        let slot = element_slot(&hs, ctx, m)?;
        let seq =
            m.inst(&format!("call i64 @{}(i32 {sid}, i64 {cur}, i32 {i}, ptr {slot})", crate::stream::Streams::AT));
        // 9.6: Auch ein Element ohne passenden Handler gilt als
        // untersucht — sonst saehe die Maschine es im naechsten Tick
        // wieder.
        m.void_inst(&format!("call void @{}(i32 {sid}, i64 {seq})", crate::stream::Streams::EXAMINED));
        m.void_inst(&format!("store i64 {seq}, ptr {ex_ptr}"));
        // 8.7: Der erste passende Handler gewinnt. Ohne Muster ist das
        // immer der erste — weitere kaemen nie zum Zug. Mit Muster wird
        // daraus eine Kette: Je Handler prueft der Automat, und wer
        // trifft, laeuft; die uebrigen springen ans Ende.
        if hs.is_empty() {
            continue;
        }
        handler_chain(&hs, slot, ctx, m)?;
        let cur_i = m.inst(&format!("load i32, ptr {i_ptr}"));
        let next = m.inst(&format!("add i32 {cur_i}, 1"));
        m.void_inst(&format!("store i32 {next}, ptr {i_ptr}"));
        m.void_inst(&format!("br label %{kopf}"));
        m.label(&ende);
        // 9.6, `advance_cursors()`: `cur[s, m] = examined + 1`. Der
        // Cursor steht im Zustand der Maschine, nicht im Strom — nur der
        // erzeugte Code kann ihn schreiben. `takt_stream_examined` meldet
        // dasselbe an die Runtime, die daraus das Minimum ueber *alle*
        // Konsumenten bildet und den Puffer freigibt; beides ist noetig,
        // und 9.6 fuehrt es als zwei Schritte.
        let ex = m.inst(&format!("load i64, ptr {ex_ptr}"));
        let etwas = m.inst(&format!("icmp sge i64 {ex}, 0"));
        let weiter_cur = m.inst(&format!("add i64 {ex}, 1"));
        // Ohne untersuchtes Element bleibt der Cursor, wo er stand.
        let neu = m.inst(&format!("select i1 {etwas}, i64 {weiter_cur}, i64 {cur}"));
        m.void_inst(&format!("store i64 {neu}, ptr {cur_ptr}"));
        let _ = end;
    }
    Ok(())
}

/// Der Index des Cursors eines Stroms im Zustands-Struct (9.6).
fn cursor_index(ctx: &Ctx<'_>, stream: takt_mir::expr::StreamRef) -> Option<u32> {
    let nth = ctx.machine.layout.cursors.iter().position(|c| *c == stream)?;
    ctx.state.index_of(Role::Cursor, nth)
}

/// Die Nummer eines Stroms fuer die Runtime.
///
/// Channel und interner Strom haben je eigene Nummern; die Runtime
/// unterscheidet sie am Vorzeichen, damit ein Aufruf genuegt.
fn stream_id(stream: takt_mir::expr::StreamRef) -> Option<i64> {
    match stream {
        takt_mir::expr::StreamRef::Channel(c) => Some(i64::from(c.0)),
        takt_mir::expr::StreamRef::Internal(s) => Some(-1 - i64::from(s.0)),
        // `t.fired` ist v1.2, ein Strom in einer Variablen v1.1; beide
        // haben zur Uebersetzungszeit keine feste Nummer.
        _ => None,
    }
}

/// Der Platz, an den `takt_stream_at` das Element schreibt.
///
/// Mit Bindung ist das die gehobene Variable (8.7); ohne Bindung ein
/// Scratch, weil der Aufruf einen Platz braucht und der Wert nicht
/// gelesen wird.
fn element_slot(
    hs: &[&takt_mir::machine::Handler],
    ctx: &mut Ctx<'_>,
    m: &mut Module,
) -> Result<crate::emit::Reg, NotYet> {
    for h in hs {
        if let Some(var) = h.binding {
            return ctx.field(Role::Var, var.index(), m).ok_or(NotYet { what: "Bindung im Zustand" });
        }
    }
    Ok(m.inst("alloca i64"))
}

/// Die Handler eines Stroms als Kette (8.7).
///
/// „Der erste passende Handler gewinnt": Je Handler entsteht eine
/// Pruefung und ein Rumpf, und wer trifft, springt ans Ende der Kette.
/// Ein Catch-all beendet sie — was danach kaeme, liefe nie.
fn handler_chain(
    hs: &[&takt_mir::machine::Handler],
    slot: crate::emit::Reg,
    ctx: &mut Ctx<'_>,
    m: &mut Module,
) -> Result<(), NotYet> {
    let k = ctx.next_label();
    let name = &ctx.machine.name;
    let ende = format!("handler{k}_{name}_ende");
    for (n, h) in hs.iter().enumerate() {
        let Some((kind, pattern)) = &h.pattern else {
            // Catch-all: Er laeuft immer, und die Kette endet hier.
            block(&h.body.clone(), ctx, m)?;
            m.void_inst(&format!("br label %{ende}"));
            m.label(&ende);
            return Ok(());
        };
        let takt_mir::pattern::Pattern::Text { pieces, dfa } = pattern else {
            return Err(NotYet { what: "Record-Muster im Handler" });
        };
        // Die Bindung ist ein Record; der Inhalt steht unter `.data`
        // beziehungsweise `.text` (8.7). Der Vergleich laeuft darauf.
        let text = m.inst(&format!("getelementptr inbounds i8, ptr {slot}, i64 0"));
        let hat_capture = pieces.iter().any(|p| matches!(p, takt_mir::pattern::PatternPiece::Capture { .. }));
        // 8.7: `matches` verlangt den ganzen Text, `has` ein Vorkommen.
        //
        // Ohne Platzhalter genuegt der Automat: Er liest jedes Byte
        // einmal und sagt, ob das Muster traegt (11.2). Mit Platzhaltern
        // braucht es den Durchlauf, denn ein Automat ueber Zeichenklassen
        // kennt die Grenzen, aber nicht die Werte — und ihn zusaetzlich
        // laufen zu lassen hiesse, denselben Text zweimal zu lesen.
        let ist_matches = *kind == takt_mir::expr::MatchKind::Matches;
        let hit = match (hat_capture, ist_matches, dfa) {
            (false, true, Some(dfa)) => {
                let id = m.next_label();
                crate::dfa::declare(id, dfa, m);
                crate::dfa::run(id, dfa, text, m)?
            }
            (_, true, _) => pattern_matches(pieces, text, h, ctx, m)?,
            (_, false, _) => pattern_has(pieces, text, h, ctx, m)?,
        };
        let (dann, sonst) = (format!("handler{k}_{n}_{name}"), format!("handler{k}_{n}_{name}_sonst"));
        m.void_inst(&format!("br i1 {hit}, label %{dann}, label %{sonst}"));
        m.label(&dann);
        block(&h.body.clone(), ctx, m)?;
        m.void_inst(&format!("br label %{ende}"));
        m.label(&sonst);
    }
    m.void_inst(&format!("br label %{ende}"));
    m.label(&ende);
    Ok(())
}

/// Wohin die Werte eines Musters gehen (8.7, Wrapper-Regel).
///
/// Die Bindung ist ein Record, dessen erste Felder die Platzhalter sind;
/// dahinter stehen `t`, `seq` und der Inhalt.
struct Bindung {
    /// Die Variable im Zustand der Maschine.
    var: takt_mir::VarId,
    /// Ihr Typ, fuer die Adressrechnung.
    record: crate::ty::LlvmType,
    /// Index und Typ je Capture, in Musterreihenfolge.
    felder: Vec<(u32, crate::ty::LlvmType)>,
}

impl Bindung {
    /// `None` heisst: keine Bindung, also nichts abzulegen — der
    /// Vergleich laeuft trotzdem.
    fn of(h: &takt_mir::machine::Handler, ctx: &Ctx<'_>) -> Option<Bindung> {
        let var = h.binding?;
        let ty = ctx.machine.vars.get(var.index())?.ty;
        let record = crate::ty::lower(ty, ctx.program)?;
        let crate::ty::LlvmType::Struct(fields) = &record else { return None };
        let takt_mir::types::Type::Record(r) = ctx.program.types.list.get(ty.index())? else { return None };
        let defs = &ctx.program.records.get(r.index())?.fields;
        // Die Captures stehen vorn; `t`, `seq` und `text`/`data`
        // schliessen an. Die Grenze ist der erste dieser Namen.
        let ende =
            defs.iter().position(|d| matches!(d.name.as_str(), "t" | "seq" | "text" | "data")).unwrap_or(defs.len());
        let felder = (0..ende).filter_map(|i| fields.get(i).map(|f| (i as u32, f.clone()))).collect();
        Some(Bindung { var, record, felder })
    }

    /// Der Platz im Zustand, an den die Werte gehen.
    fn slot(&self, ctx: &mut Ctx<'_>, m: &mut Module) -> Result<crate::emit::Reg, NotYet> {
        ctx.field(Role::Var, self.var.index(), m).ok_or(NotYet { what: "Bindung im Zustand" })
    }
}

/// Was `captures::walk` braucht, oder nichts.
fn ziel<'a>(
    b: &'a Option<Bindung>,
    ctx: &mut Ctx<'_>,
    m: &mut Module,
) -> Result<Option<crate::captures::Ziel<'a>>, NotYet> {
    match b {
        Some(b) => {
            let slot = b.slot(ctx, m)?;
            Ok(Some(crate::captures::Ziel { slot, record: &b.record, felder: &b.felder }))
        }
        None => Ok(None),
    }
}

/// `matches P`: Das Muster muss den ganzen Text verbrauchen (8.7).
fn pattern_matches(
    pieces: &[takt_mir::pattern::PatternPiece],
    text: crate::emit::Reg,
    h: &takt_mir::machine::Handler,
    ctx: &mut Ctx<'_>,
    m: &mut Module,
) -> Result<crate::emit::Reg, NotYet> {
    let b = Bindung::of(h, ctx);
    let wohin = ziel(&b, ctx, m)?;
    let null = m.inst("add i32 0, 0");
    let (ok, at) = crate::captures::walk(pieces, text, wohin.as_ref(), null, m)?;
    // Der ganze Text: Was hinter dem Durchlauf steht, darf nicht sein.
    let len_ptr = m.inst(&format!("getelementptr inbounds i8, ptr {text}, i64 0"));
    let len = m.inst(&format!("load i32, ptr {len_ptr}"));
    let ganz = m.inst(&format!("icmp eq i32 {at}, {len}"));
    Ok(m.inst(&format!("and i1 {ok}, {ganz}")))
}

/// `has P`: Das Muster darf an jeder Stelle beginnen (8.7).
///
/// Gesucht wird das linkeste Vorkommen. Die Schleife ist durch die
/// Textlaenge beschraenkt, die ihrerseits durch `N` beschraenkt ist
/// (3.9) — 4.1 verlangt genau das.
fn pattern_has(
    pieces: &[takt_mir::pattern::PatternPiece],
    text: crate::emit::Reg,
    h: &takt_mir::machine::Handler,
    ctx: &mut Ctx<'_>,
    m: &mut Module,
) -> Result<crate::emit::Reg, NotYet> {
    let b = Bindung::of(h, ctx);
    let wohin = ziel(&b, ctx, m)?;
    let k = m.next_label();
    let (kopf, rumpf, fertig) = (format!("has{k}"), format!("has{k}_rumpf"), format!("has{k}_fertig"));
    let len_ptr = m.inst(&format!("getelementptr inbounds i8, ptr {text}, i64 0"));
    let len = m.inst(&format!("load i32, ptr {len_ptr}"));
    let start_ptr = m.inst("alloca i32");
    m.void_inst(&format!("store i32 0, ptr {start_ptr}"));
    let hit_ptr = m.inst("alloca i1");
    m.void_inst(&format!("store i1 false, ptr {hit_ptr}"));
    m.void_inst(&format!("br label %{kopf}"));

    m.label(&kopf);
    let start = m.inst(&format!("load i32, ptr {start_ptr}"));
    // Auch hinter dem letzten Zeichen wird geprueft: Ein leeres Muster
    // passt am Ende (8.7).
    let im_text = m.inst(&format!("icmp sle i32 {start}, {len}"));
    let bisher = m.inst(&format!("load i1, ptr {hit_ptr}"));
    let offen = m.inst(&format!("xor i1 {bisher}, true"));
    let suchen = m.inst(&format!("and i1 {im_text}, {offen}"));
    m.void_inst(&format!("br i1 {suchen}, label %{rumpf}, label %{fertig}"));

    m.label(&rumpf);
    let (ok, _) = crate::captures::walk(pieces, text, wohin.as_ref(), start, m)?;
    m.void_inst(&format!("store i1 {ok}, ptr {hit_ptr}"));
    let ni = m.inst(&format!("add i32 {start}, 1"));
    m.void_inst(&format!("store i32 {ni}, ptr {start_ptr}"));
    m.void_inst(&format!("br label %{kopf}"));

    m.label(&fertig);
    Ok(m.inst(&format!("load i1, ptr {hit_ptr}")))
}

/// Der Fault-Pfad eines Blattzustands (5.2 Regel 5, 5.3).
///
/// Ein `check`, der scheitert, springt hierher. Der Pfad tut, was 5.2
/// verlangt: Er merkt den Fault vor (`last_fault`, fuer `m.last_fault`),
/// betritt das Fault-Ziel und fuehrt dessen `enter:` und `loop:` im
/// Entry-Modus aus.
///
/// **Warum je Blatt und nicht einmal je Maschine.** Das Fault-Ziel haengt
/// am innersten Zustand, der eines deklariert (Fault-Wald, 5.3); zwei
/// Blaetter koennen verschiedene haben. Ein gemeinsamer Trampolin
/// muesste die Konfiguration erneut auswerten — er haette den `switch`
/// ein zweites Mal.
fn fault_path(
    st: &StateStruct,
    from: takt_mir::StateId,
    ziel: &Ziel<'_>,
    ctx: &mut Ctx<'_>,
    m: &mut Module,
) -> Result<(), NotYet> {
    let machine_def = ctx.machine;
    let (leaves, end, conf_slot) = (ziel.leaves, ziel.end, ziel.conf);
    m.label(&format!("fault_{}_{}", machine_def.name, from.index()));
    // Der Fault wird vorgemerkt; `pending` traegt ihn fuer die
    // Abort-Phase (5.4), die die Runtime fuehrt.
    if let Some(pending) = st.index_of(Role::Pending, 0) {
        let state_ty = format!("%{}_state", crate::fns::sanitized(&machine_def.name));
        let field = m.inst(&format!("getelementptr inbounds {state_ty}, ptr %0, i32 0, i32 {pending}"));
        let flag = m.inst(&format!("getelementptr inbounds {{ i1, i32, i32 }}, ptr {field}, i32 0, i32 0"));
        m.void_inst(&format!("store i1 true, ptr {flag}"));
    }
    let target = machine_def.fault_target_of(from);
    let takt_mir::machine::FaultTarget::State(to) = target else {
        // `FAULTED` fuehrt keinen Nutzercode aus (5.2 Regel 5), und die
        // Outputs gehen auf `safe` — noch in diesem Tick, nicht erst im
        // naechsten: Der Interpreter tut es an derselben Stelle, und ein
        // Ventil, das offen stand, bliebe sonst einen Tick laenger offen.
        leave_configuration(ctx, m, leaves.len());
        safe_outputs(ctx, m)?;
        m.void_inst(&format!("br label %{end}"));
        return Ok(());
    };
    let Some(leaf) = machine::initial_leaf(machine_def, to) else {
        m.void_inst("ret void");
        return Ok(());
    };
    let Some(index) = leaves.iter().position(|l| *l == leaf) else {
        m.void_inst("ret void");
        return Ok(());
    };
    // 5.2 Regel 3: `exit:` des verlassenen, `enter:` des betretenen
    // Zustands. Ein Fault-Uebergang laeuft sonst wie jeder andere.
    for id in machine::exiting(machine_def, from, leaf) {
        block(&machine_def.states[id.index()].exit.clone(), ctx, m)?;
    }
    for id in machine::entering(machine_def, from, leaf) {
        block(&machine_def.states[id.index()].enter.clone(), ctx, m)?;
    }
    m.void_inst(&format!("store i8 {index}, ptr {conf_slot}"));
    reset_time(ctx, m);
    // Entry-Modus: Die `loop:`-Bloecke des Fault-Ziels laufen noch in
    // diesem Tick (5.2 Regel 4 und 5).
    for id in machine::entering(machine_def, from, leaf) {
        block(&machine_def.states[id.index()].loop_block.clone(), ctx, m)?;
    }
    m.void_inst(&format!("br label %{end}"));
    Ok(())
}

/// Wohin ein Fault-Pfad fuehrt und wo er endet.
///
/// Die drei gehoeren zusammen: Sie beschreiben denselben Zweig, und
/// einzeln durchgereicht waeren sie drei Gelegenheiten, den falschen zu
/// nehmen.
struct Ziel<'a> {
    /// Die Blattzustaende der Maschine, fuer die Nummer des Ziels.
    leaves: &'a [StateId],
    /// Die Marke am Ende des Schritts.
    end: &'a str,
    /// Der Zeiger auf `conf[0]`.
    conf: &'a crate::emit::Reg,
}
