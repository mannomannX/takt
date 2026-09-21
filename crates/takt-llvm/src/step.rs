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
use takt_mir::expr::{Expr, ExprKind};
use takt_mir::machine::{Guard, Machine, Target, TransTrigger, Transition};
use takt_mir::program::Program;

use crate::emit::Module;
use crate::expr::{NotYet, Vars, lower as lower_expr};
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

/// 5.7: `step` hoechstens einmal je Aktivierung — die Flags aller
/// Instanzen gehen zu Beginn zurueck, wie `clear_stepped` im Interpreter.
fn clear_stepped(m: &Machine, st: &StateStruct, p: &Program, module: &mut Module) -> Result<(), NotYet> {
    let state_ty = format!("%{}_state", crate::fns::sanitized(&m.name));
    for bi in &m.layout.block_instances {
        let def = p.blocks.get(bi.block.index()).ok_or(NotYet { what: "Block" })?;
        let inst = crate::block::instance_of(def, p).ok_or(NotYet { what: "Blockinstanz" })?;
        let i = st.index_of(Role::Var, bi.var.index()).ok_or(NotYet { what: "Instanz im Zustand" })?;
        let field = module.inst(&format!("getelementptr inbounds {state_ty}, ptr %0, i32 0, i32 {i}"));
        let flag =
            module.inst(&format!("getelementptr inbounds {}, ptr {field}, i32 0, i32 {}", inst.llvm(), inst.stepped()));
        module.void_inst(&format!("store i1 false, ptr {flag}"));
    }
    Ok(())
}

/// Eine Blockinstanz im Zustand der Maschine: erst die Parameter aus den
/// Argumenten der Instanziierung, dann der Zustand (5.7).
fn init_instance(
    block: takt_mir::BlockId,
    init: &Expr,
    var: usize,
    ctx: &mut Ctx<'_>,
    module: &mut Module,
) -> Result<(), NotYet> {
    let ExprKind::BlockInit { args, count: None, .. } = &init.kind else {
        return Err(NotYet { what: "Instanzfeld" });
    };
    let p = ctx.program;
    let def = p.blocks.get(block.index()).ok_or(NotYet { what: "Block" })?;
    let inst = crate::block::instance_of(def, p).ok_or(NotYet { what: "Blockinstanz" })?;
    let ptr = ctx.field(Role::Var, var, module).ok_or(NotYet { what: "Instanz im Zustand" })?;
    let struct_ty = inst.llvm();
    let vars = ctx.vars();
    for (k, a) in args.iter().enumerate() {
        let v = lower_expr(a, p, module, &vars)?;
        let at = module.inst(&format!("getelementptr inbounds {struct_ty}, ptr {ptr}, i32 0, i32 {k}"));
        module.void_inst(&format!("store {} {}, ptr {at}", v.ty, v.value));
    }
    let exit = vars.fault_label().ok_or(NotYet { what: "Fault-Marke" })?;
    crate::block::init_state(ptr, def, &inst, exit, p, module)
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
    clear_stepped(m, st, p, module)?;

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
    // 11.2: Ein `->` im Block springt ans Kettenende. Nur hier gesetzt —
    // die Init-Funktion laeuft im Entry-Modus, und dort ist es
    // wirkungslos (5.2 Regel 4).
    ctx.end = Some(end.clone());
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
        dispatch(&handler, &mut ctx, module)?;
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
        fault_path(st, *id, &Jump { leaves, end: &end, conf: &slot }, &mut ctx, module)?;
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
    // 9.6, `advance_cursors()`: `cur[s, m] = examined + 1`. Der Cursor
    // steht im Zustand der Maschine, nicht im Strom — nur der erzeugte
    // Code kann ihn schreiben; `takt_stream_examined` meldet dasselbe
    // an die Runtime, die daraus das Minimum ueber alle Konsumenten
    // bildet. Ohne untersuchtes Element bleibt der Cursor, wo er stand.
    for i in 0..m.layout.cursors.len() {
        let (Some(c), Some(e)) = (st.index_of(Role::Cursor, i), st.index_of(Role::Examined, i)) else { continue };
        let cur_ptr = module.inst(&format!("getelementptr inbounds {state_ty}, ptr %0, i32 0, i32 {c}"));
        let ex_ptr = module.inst(&format!("getelementptr inbounds {state_ty}, ptr %0, i32 0, i32 {e}"));
        let cur = module.inst(&format!("load i64, ptr {cur_ptr}"));
        let next = module.inst(&format!("load i64, ptr {ex_ptr}"));
        let ahead = module.inst(&format!("icmp sgt i64 {next}, {cur}"));
        let new = module.inst(&format!("select i1 {ahead}, i64 {next}, i64 {cur}"));
        module.void_inst(&format!("store i64 {new}, ptr {cur_ptr}"));
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
            TransTrigger::When(Guard::Match { subject, kind, pattern, binding }) => {
                match_guard(subject, *kind, pattern, *binding, ctx, m)?
            }
            TransTrigger::When(Guard::Next { stream, binding }) => next_element(*stream, *binding, ctx, m)?,
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
        if let Some(slot) = ctx.machine.layout.saved_paths.iter().position(|s| *s == to) {
            resume_into(ctx, m, leaves[from], to, slot, conf_slot, &t.actions, leaves, end, &skip)?;
            continue;
        }
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
        enter_leaf(ctx, m, leaves[from], leaf, index, conf_slot, Some(&t.actions))?;
        // 5.2 Regel 4 (Entry-Tick): Die `loop:`-Bloecke der neu betretenen
        // Zustaende laufen noch in diesem Tick — die darueberliegenden
        // liefen bereits. `check`s wirken, `-> ZIEL` ist wirkungslos, und
        // `on`-Handler laufen nicht (das Fenster ist leer).
        //
        // Ohne sie erreichte ein Zustand seine Invarianten einen Tick zu
        // spaet, und die Outputs des Ticks stuenden auf den Werten des
        // alten Zustands.
        // 5.2 Regel 4: Im Entry-Tick ist `-> ZIEL` wirkungslos. Der
        // Interpreter erreicht das mit `Mode::Entry`; hier wird das
        // Sprungziel fuer die Dauer dieser Bloecke entfernt.
        let saved = ctx.end.take();
        for id in machine::entering(ctx.machine, leaves[from], leaf) {
            block(&ctx.machine.states[id.index()].loop_block.clone(), ctx, m)?;
        }
        ctx.end = saved;
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
/// Ventil, das remaining stand, bliebe remaining.
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

/// Der Wechsel von einem Blatt zu einem anderen (5.2).
///
/// **Eine Stelle fuer beide Ausloeser.** Ein Uebergang (`when`/`after`)
/// und ein `->` im Block tun dasselbe; 5.2 gibt die Reihenfolge vor, und
/// zwei Kopien waeren zwei Gelegenheiten, sie verschieden auszulegen.
///
/// Die Reihenfolge: `exit:` vom verlassenen Blatt aufwaerts bis unter den
/// gemeinsamen Vorfahren, dann der Aktionsblock (nur ein Uebergang hat
/// einen), dann `enter:` von dort abwaerts bis zum neuen Blatt. Wer nur
/// Blatt und Ziel nimmt, laesst die Zwischenebenen aus — und ein `enter:`
/// dort ist genau die Stelle, an der ein Ablauf seine Vorbedingung
/// herstellt.
/// 5.12: Was verlassen wird, merkt sich sein Blatt.
fn save_paths(ctx: &mut Ctx<'_>, m: &mut Module, from: StateId, leaf: StateId) {
    for id in machine::exiting(ctx.machine, from, leaf) {
        let Some(slot) = ctx.machine.layout.saved_paths.iter().position(|s| *s == id) else { continue };
        let Some(ptr) = ctx.field(Role::Saved, slot, m) else { continue };
        m.void_inst(&format!("store i32 {}, ptr {ptr}", from.index()));
    }
}

/// 5.12: Ein Uebergang auf einen `resume`-Zustand betritt den gespeicherten
/// Blattpfad; ohne gespeicherten Pfad das `initial`-Kind.
#[allow(clippy::too_many_arguments)]
fn resume_into(
    ctx: &mut Ctx<'_>,
    m: &mut Module,
    from: StateId,
    to: StateId,
    slot: usize,
    conf_slot: &crate::emit::Reg,
    actions: &takt_mir::stmt::Block,
    leaves: &[StateId],
    end: &str,
    skip: &str,
) -> Result<(), NotYet> {
    let initial = machine::initial_leaf(ctx.machine, to).ok_or(NotYet { what: "Zielzustand ohne `initial`" })?;
    let under: Vec<(usize, StateId)> = leaves
        .iter()
        .enumerate()
        .filter(|(_, l)| machine::path_to(ctx.machine, **l).contains(&to))
        .map(|(i, l)| (i, *l))
        .collect();
    let Some(ptr) = ctx.field(Role::Saved, slot, m) else { return Err(NotYet { what: "`saved`-Slot" }) };
    let saved = m.inst(&format!("load i32, ptr {ptr}"));
    let tail = format!("resume_{}_{}", ctx.machine.name, to.index());
    for (n, (index, leaf)) in under.iter().enumerate() {
        let hit = format!("{tail}_hit{n}");
        let next = format!("{tail}_next{n}");
        let cond = m.inst(&format!("icmp eq i32 {saved}, {}", leaf.index()));
        m.void_inst(&format!("br i1 {cond}, label %{hit}, label %{next}"));
        m.label(&hit);
        enter_leaf(ctx, m, from, *leaf, *index, conf_slot, Some(actions))?;
        entry_tick(ctx, m, from, *leaf)?;
        m.void_inst(&format!("br label %{end}"));
        m.label(&next);
    }
    let index = leaves.iter().position(|l| *l == initial).ok_or(NotYet { what: "Zielblatt" })?;
    enter_leaf(ctx, m, from, initial, index, conf_slot, Some(actions))?;
    entry_tick(ctx, m, from, initial)?;
    m.void_inst(&format!("br label %{end}"));
    m.label(skip);
    Ok(())
}

/// 5.2 Regel 4: die `loop:`-Bloecke der betretenen Zustaende, `-> ZIEL`
/// wirkungslos.
fn entry_tick(ctx: &mut Ctx<'_>, m: &mut Module, from: StateId, leaf: StateId) -> Result<(), NotYet> {
    let saved = ctx.end.take();
    for id in machine::entering(ctx.machine, from, leaf) {
        block(&ctx.machine.states[id.index()].loop_block.clone(), ctx, m)?;
    }
    ctx.end = saved;
    Ok(())
}

fn enter_leaf(
    ctx: &mut Ctx<'_>,
    m: &mut Module,
    from: StateId,
    leaf: StateId,
    index: usize,
    conf_slot: &crate::emit::Reg,
    actions: Option<&takt_mir::stmt::Block>,
) -> Result<(), NotYet> {
    save_paths(ctx, m, from, leaf);
    for id in machine::exiting(ctx.machine, from, leaf) {
        block(&ctx.machine.states[id.index()].exit.clone(), ctx, m)?;
    }
    if let Some(a) = actions {
        block(&a.clone(), ctx, m)?;
    }
    for id in machine::entering(ctx.machine, from, leaf) {
        block(&ctx.machine.states[id.index()].enter.clone(), ctx, m)?;
    }
    m.void_inst(&format!("store i8 {index}, ptr {conf_slot}"));
    if m.instrument != crate::target::Instrument::Off {
        crate::stmt::mark(leaf.0, ctx, m);
    }
    reset_time(ctx, m);
    // 5.8/5.6: Die `every`- und Bestaetigungszaehler der betretenen
    // Zustaende beginnen neu. Vor den `loop:`-Bloecken darunter, weil die
    // im selben Tick laufen (5.2 Regel 4) und das `every` dort steht —
    // ein Reset danach setzte zurueck, was gerade feuerte.
    for id in machine::entering(ctx.machine, from, leaf) {
        reset_counters(ctx, Some(id), m);
    }
    Ok(())
}

/// `-> ZIEL` als Anweisung im Block (11.2).
///
/// 11.2 gibt die Form vor: „`->` → Setzen der Goto-Vormerkung + Sprung
/// ans Kettenende." Der Sprung ist hier der an `end`: Was im Block
/// danach steht, laeuft nicht mehr, und die Uebergaenge des verlassenen
/// Zustands werden nicht mehr geprueft.
///
/// **Nur im Run-Modus.** Der Interpreter liefert `Out::Goto` nur dort
/// (`exec`); im Entry-Modus ist ein `->` wirkungslos (5.2 Regel 4), weil
/// der Zustand gerade erst betreten wurde. Der Codegen erzeugt die
/// `loop:`-Bloecke des Entry-Ticks aus demselben MIR-Block — ein `->`
/// darin duerfte also nicht wirken. Hier gilt darum dieselbe Regel wie
/// bei `dispatch`: Der Entry-Zweig senkt den Block ohne Goto.
pub fn goto(target: Target, ctx: &mut Ctx<'_>, m: &mut Module, end: &str) -> Result<(), NotYet> {
    let leaves = machine::leaves(ctx.machine);
    let from = ctx.leaf.ok_or(NotYet { what: "`->` ausserhalb eines Blattzweigs" })?;
    let Some(from_index) = leaves.iter().position(|l| *l == from) else {
        return Err(NotYet { what: "`->` aus einem unbekannten Blatt" });
    };
    let state_ty = format!("%{}_state", crate::fns::sanitized(&ctx.machine.name));
    let conf_i = ctx.state.index_of(Role::Conf, 0).ok_or(NotYet { what: "conf im Zustand" })?;
    let conf = m.inst(&format!("getelementptr inbounds {state_ty}, ptr %0, i32 0, i32 {conf_i}"));
    let slot = m.inst(&format!("getelementptr inbounds [{} x i8], ptr {conf}, i32 0, i32 0", ctx.state.depth));
    match target {
        Target::State(to) => {
            let leaf = machine::initial_leaf(ctx.machine, to).ok_or(NotYet { what: "Zielzustand ohne `initial`" })?;
            let Some(index) = leaves.iter().position(|l| *l == leaf) else {
                return Err(NotYet { what: "Zielblatt" });
            };
            enter_leaf(ctx, m, leaves[from_index], leaf, index, &slot, None)?;
            // 5.2 Regel 4: Die `loop:`-Bloecke der neu betretenen
            // Zustaende laufen noch in diesem Tick. Der Interpreter tut
            // es in `switch` (`exec_chain(… Mode::Entry)`), und zwar fuer
            // *jeden* Wechsel — auch fuer ein `->` im Block.
            //
            // Ohne das zaehlte ein Zaehler im Ziel einen Tick zu spaet;
            // der Strukturfuzzer fand es an `c = c + 1; -> S1` mit einem
            // zweiten `c = c + 10` im Ziel (FB-122).
            //
            // Im Entry-Modus ist ein weiteres `->` wirkungslos, darum
            // wird das Sprungziel fuer die Dauer entfernt.
            let saved = ctx.end.take();
            for id in machine::entering(ctx.machine, leaves[from_index], leaf) {
                block(&ctx.machine.states[id.index()].loop_block.clone(), ctx, m)?;
            }
            ctx.end = saved;
        }
        // `-> FAULTED` (5.3): die Konfiguration wird leer, die Outputs
        // gehen auf `safe`. Wie beim Uebergang.
        Target::Faulted => {
            leave_configuration(ctx, m, leaves.len());
            safe_outputs(ctx, m)?;
        }
        // Ein Fault-Ziel als Anweisung gibt es nicht: `Target::Fault`
        // entsteht nur aus dem Timeout einer Sequenz (6.2), und der ist
        // ein Uebergang, keine Anweisung.
        Target::Fault(_) => return Err(NotYet { what: "`->` auf ein Fault-Ziel" }),
    }
    m.void_inst(&format!("br label %{end}"));
    // Was nach dem Sprung kaeme, ist unerreichbar; LLVM verlangt fuer den
    // folgenden Code trotzdem einen Block.
    let k = ctx.next_label();
    m.label(&format!("nach_goto{k}_{}", ctx.machine.name));
    Ok(())
}

/// `t_in_state = 0` beim Eintritt in einen Zustand (5.2).
///
/// Ohne das Zuruecksetzen misst `after d` die Zeit elapsed dem Start der
/// Maschine statt elapsed dem Eintritt — der haeufigste Fehler, den eine
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

/// Setzt die `every`- und Bestaetigungszaehler eines Zustands beim
/// Eintritt zurueck (5.8, 5.6).
///
/// Der Interpreter loescht ihre Eintraege (`enter_state`), sodass der
/// naechste Zugriff wieder mit dem Startwert beginnt. Hier stehen sie als
/// Felder, also wird geschrieben: `every_next` auf null — der Block
/// laeuft dann, sobald `uhr >= 0` gilt, und `every` addiert `d` darauf.
///
/// **Warum null und nicht `d`.** 5.8 nennt `d` als Startwert, und der
/// Interpreter setzt ihn in `at_or`. Beide Wege ergeben dasselbe
/// Verhalten, weil die Uhr beim Eintritt ebenfalls auf null steht: Mit
/// Start `d` feuert der Block erstmals bei `uhr == d`, mit Start null
/// sofort. Der Unterschied ist beobachtbar — darum wird hier `d` beim
/// ersten Durchlauf gesetzt, nicht hier; siehe `every`.
fn reset_counters(ctx: &Ctx<'_>, s: Option<takt_mir::StateId>, m: &mut Module) {
    let state_ty = format!("%{}_state", crate::fns::sanitized(&ctx.machine.name));
    for (i, c) in ctx.machine.layout.every_counters.iter().enumerate() {
        if c.state != s {
            continue;
        }
        let Some(idx) = ctx.state.index_of(Role::EveryNext, i) else { continue };
        let p = m.inst(&format!("getelementptr inbounds {state_ty}, ptr %0, i32 0, i32 {idx}"));
        // `-1` heisst „noch nicht gesetzt": Der erste Durchlauf traegt
        // `d` ein (5.8). Eine Null waere ein gueltiger Zeitpunkt und
        // liesse den Block im Eintritts-Tick laufen.
        m.void_inst(&format!("store i64 -1, ptr {p}"));
    }
    for (i, c) in ctx.machine.layout.viol_sites.iter().enumerate() {
        if c.state != s {
            continue;
        }
        let Some(idx) = ctx.state.index_of(Role::Viol, i) else { continue };
        let p = m.inst(&format!("getelementptr inbounds {state_ty}, ptr %0, i32 0, i32 {idx}"));
        m.void_inst(&format!("store i32 0, ptr {p}"));
    }
}

/// `<maschine>_advance(st, n)`: `n` virtuelle Ticks nachtragen (9.9).
///
/// Ein uebersprungener Tick ruft kein `_step`; ohne diese Funktion bliebe
/// `t_in_state` stehen und die `after`-Frist feuerte um die geschlafenen
/// Ticks zu spaet. `n` kommt in Basis-Ticks und wird durch die Periode
/// geteilt, weil der Zaehler Aktivierungen zaehlt (7.2).
///
/// Die `every`-Zaehler bleiben unberuehrt — 9.9 sagt es ausdruecklich,
/// und in `idle` gibt es kein `loop:`, also auch kein `every`.
pub fn advance_function(m: &Machine, st: &StateStruct, module: &mut Module) -> Result<(), NotYet> {
    let period = i64::from(m.period.max(1));
    module.begin(
        &format!("{}_advance", m.name),
        &crate::ty::LlvmType::Void,
        &[crate::ty::LlvmType::Ptr, crate::ty::LlvmType::Int(64)],
    );
    let Some(t_i) = st.index_of(Role::TimeInState, 0) else {
        module.end(None);
        return Ok(());
    };
    let state_ty = format!("%{}_state", crate::fns::sanitized(&m.name));
    let base = module.inst(&format!("getelementptr inbounds {state_ty}, ptr %0, i32 0, i32 {t_i}"));
    let cell = module.inst(&format!("getelementptr inbounds [{} x i64], ptr {base}, i32 0, i32 0", st.depth));
    let old = module.inst(&format!("load i64, ptr {cell}"));
    let activations = module.inst(&format!("sdiv i64 %1, {period}"));
    let new = module.inst(&format!("add i64 {old}, {activations}"));
    module.void_inst(&format!("store i64 {new}, ptr {cell}"));
    module.end(None);
    Ok(())
}

/// `<maschine>_idle(st) -> i1`: Ist die Maschine bereit zu schlafen (9.9)?
///
/// Zwei der sechs Konjunkte stehen im Zustandsblock: Das aktive Blatt ist
/// `idle` (oder liegt unter einem `idle`-Zustand), und `pending` ist leer.
/// Die uebrigen vier kennt nur der Rahmen.
pub fn idle_function(m: &Machine, st: &StateStruct, module: &mut Module) -> Result<(), NotYet> {
    let leaves = machine::leaves(m);
    let sleeping: Vec<usize> = leaves
        .iter()
        .enumerate()
        .filter(|(_, l)| machine::path_to(m, **l).iter().any(|id| m.states[id.index()].idle))
        .map(|(i, _)| i)
        .collect();

    let mark = module.mark();
    module.begin(&format!("{}_idle", m.name), &crate::ty::LlvmType::Int(1), &[crate::ty::LlvmType::Ptr]);

    // Ohne `idle`-Zustand schlaeft die Maschine nie.
    if sleeping.is_empty() {
        module.end(Some((&crate::ty::LlvmType::Int(1), "0".into())));
        return Ok(());
    }

    let state_ty = format!("%{}_state", crate::fns::sanitized(&m.name));
    let Some(conf_i) = st.index_of(Role::Conf, 0) else {
        module.abort(mark);
        return Err(NotYet { what: "conf im Zustand" });
    };
    let conf = module.inst(&format!("getelementptr inbounds {state_ty}, ptr %0, i32 0, i32 {conf_i}"));
    let slot = module.inst(&format!("getelementptr inbounds [{} x i8], ptr {conf}, i32 0, i32 0", st.depth));
    let cur = module.inst(&format!("load i8, ptr {slot}"));

    let mut acc = None;
    for i in &sleeping {
        let eq = module.inst(&format!("icmp eq i8 {cur}, {i}"));
        acc = Some(match acc {
            None => eq,
            Some(a) => module.inst(&format!("or i1 {a}, {eq}")),
        });
    }
    let in_idle = acc.expect("mindestens ein Blatt");

    // `pending`: Feld 0 des Fault-Records ist das Flag.
    let Some(pending_i) = st.index_of(Role::Pending, 0) else {
        module.abort(mark);
        return Err(NotYet { what: "pending im Zustand" });
    };
    let pending = module.inst(&format!("getelementptr inbounds {state_ty}, ptr %0, i32 0, i32 {pending_i}"));
    let flag = module.inst(&format!("getelementptr inbounds {{ i1, i32, i32 }}, ptr {pending}, i32 0, i32 0"));
    let raised = module.inst(&format!("load i1, ptr {flag}"));
    let quiet = module.inst(&format!("xor i1 {raised}, true"));
    let out = module.inst(&format!("and i1 {in_idle}, {quiet}"));

    module.end(Some((&crate::ty::LlvmType::Int(1), out.to_string())));
    Ok(())
}

/// `<maschine>_scope_<n>(st) -> i1`: Steht der Scope-Zustand der n-ten
/// gescopten Instanz in der Konfiguration? (5.11)
///
/// Die Aktivitaet ist eine Frage an die Konfiguration des Besitzers, und
/// die steht in `conf[0]` als Blattnummer. Welche Blaetter unter dem
/// Scope liegen, weiss der Codegen statisch — die Funktion ist darum ein
/// `or` ueber eine feste Liste, wie `_idle`. Der Rahmen ruft sie vor dem
/// Schritt der Instanz und vergleicht mit dem Stand des vorigen Ticks.
pub fn scope_function(
    m: &Machine,
    st: &StateStruct,
    index: usize,
    scope: takt_mir::StateId,
    module: &mut Module,
) -> Result<(), NotYet> {
    let leaves = machine::leaves(m);
    let under: Vec<usize> =
        leaves.iter().enumerate().filter(|(_, l)| machine::path_to(m, **l).contains(&scope)).map(|(i, _)| i).collect();

    let mark = module.mark();
    module.begin(&format!("{}_scope_{index}", m.name), &crate::ty::LlvmType::Int(1), &[crate::ty::LlvmType::Ptr]);
    if under.is_empty() {
        module.end(Some((&crate::ty::LlvmType::Int(1), "0".into())));
        return Ok(());
    }
    let state_ty = format!("%{}_state", crate::fns::sanitized(&m.name));
    let Some(conf_i) = st.index_of(Role::Conf, 0) else {
        module.abort(mark);
        return Err(NotYet { what: "conf im Zustand" });
    };
    let conf = module.inst(&format!("getelementptr inbounds {state_ty}, ptr %0, i32 0, i32 {conf_i}"));
    let slot = module.inst(&format!("getelementptr inbounds [{} x i8], ptr {conf}, i32 0, i32 0", st.depth));
    let cur = module.inst(&format!("load i8, ptr {slot}"));
    let mut acc = None;
    for i in &under {
        let eq = module.inst(&format!("icmp eq i8 {cur}, {i}"));
        acc = Some(match acc {
            None => eq,
            Some(a) => module.inst(&format!("or i1 {a}, {eq}")),
        });
    }
    let out = acc.expect("mindestens ein Blatt");
    module.end(Some((&crate::ty::LlvmType::Int(1), out.to_string())));
    Ok(())
}

/// `<maschine>_deadline(st) -> i64`: Basis-Ticks bis zur naechsten
/// `after`-Frist.
///
/// `-1` heisst: keine Frist, es weckt nur ein Ereignis (9.9). `t_in_state`
/// zaehlt Aktivierungen, die Runtime springt Basis-Ticks — die Periode
/// rechnet zwischen beiden um (7.2).
pub fn deadline_function(m: &Machine, st: &StateStruct, p: &Program, module: &mut Module) -> Result<(), NotYet> {
    let leaves = machine::leaves(m);
    // Eine Aktivierung dauert `period` Basis-Ticks (7.2); `t_in_state`
    // zaehlt Aktivierungen, nicht Ticks.
    let period = u64::from(m.period.max(1));
    let activation_ns = period.saturating_mul(p.config.tick.max(1) as u64);
    // Je Blatt die kuerzeste `after`-Frist seiner Kette, in Aktivierungen.
    let deadlines: Vec<Option<u64>> = leaves
        .iter()
        .map(|l| {
            machine::path_to(m, *l)
                .iter()
                .flat_map(|id| &m.states[id.index()].transitions)
                .filter_map(|t| match &t.trigger {
                    // `after 0` feuert bei der ersten Aktivierung
                    // (`elapsed > 0`), ist also eine Frist von eins.
                    TransTrigger::After(e) => match e.kind {
                        takt_mir::expr::ExprKind::Duration(ns) if ns >= 0 => {
                            Some((ns as u64).div_ceil(activation_ns).max(1))
                        }
                        _ => None,
                    },
                    TransTrigger::When(_) => None,
                })
                .min()
        })
        .collect();

    let mark = module.mark();
    module.begin(&format!("{}_deadline", m.name), &crate::ty::LlvmType::Int(64), &[crate::ty::LlvmType::Ptr]);

    if deadlines.iter().all(Option::is_none) {
        module.end(Some((&crate::ty::LlvmType::Int(64), "-1".into())));
        return Ok(());
    }

    let state_ty = format!("%{}_state", crate::fns::sanitized(&m.name));
    let (Some(conf_i), Some(tis_i)) = (st.index_of(Role::Conf, 0), st.index_of(Role::TimeInState, 0)) else {
        module.abort(mark);
        return Err(NotYet { what: "conf oder t_in_state im Zustand" });
    };
    let conf = module.inst(&format!("getelementptr inbounds {state_ty}, ptr %0, i32 0, i32 {conf_i}"));
    let slot = module.inst(&format!("getelementptr inbounds [{} x i8], ptr {conf}, i32 0, i32 0", st.depth));
    let cur = module.inst(&format!("load i8, ptr {slot}"));
    let tis = module.inst(&format!("getelementptr inbounds {state_ty}, ptr %0, i32 0, i32 {tis_i}"));
    let tis0 = module.inst(&format!("getelementptr inbounds [{} x i64], ptr {tis}, i32 0, i32 0", st.depth));
    let elapsed = module.inst(&format!("load i64, ptr {tis0}"));

    // Kaskade statt Sprungtabelle: Ein Blatt ohne Frist liefert -1.
    let mut acc = module.inst("select i1 true, i64 -1, i64 -1");
    for (i, deadline) in deadlines.iter().enumerate() {
        let Some(ticks) = deadline else { continue };
        let rest = module.inst(&format!("sub i64 {ticks}, {elapsed}"));
        let positiv = module.inst(&format!("icmp sgt i64 {rest}, 0"));
        let remaining = module.inst(&format!("select i1 {positiv}, i64 {rest}, i64 0"));
        // Die Runtime springt Basis-Ticks, nicht Aktivierungen.
        let value = module.inst(&format!("mul i64 {remaining}, {period}"));
        let is_leaf = module.inst(&format!("icmp eq i8 {cur}, {i}"));
        acc = module.inst(&format!("select i1 {is_leaf}, i64 {value}, i64 {acc}"));
    }

    module.end(Some((&crate::ty::LlvmType::Int(64), acc.to_string())));
    Ok(())
}

/// Schreibt die Eintrittsfunktion einer Maschine (9.4).
///
/// 9.4: Der Anfangszustand wird betreten, *bevor* der erste Tick laeuft —
/// sein `enter:` gehoert darum nicht in den Tickschritt, sondern in eine
/// eigene Funktion, die die Runtime einmal ruft. Stuende es im Schritt,
/// liefe es in jedem Tick.
pub fn init_function(m: &Machine, st: &StateStruct, p: &Program, module: &mut Module) -> Result<(), NotYet> {
    emit_init(m, st, p, module, "_init", true, true)
}

/// `<maschine>_init_vars`: nur Anfangszustand und s0-Defaults (5.9).
///
/// Ein Rahmen mit Journal ruft danach `_persist_restore` und erst dann
/// `_enter` — dieselbe Reihenfolge, in der der Interpreter zwischen
/// `init_vars` und `machine::init` laedt. Ohne Journal genuegt `_init`.
pub fn init_vars_function(m: &Machine, st: &StateStruct, p: &Program, module: &mut Module) -> Result<(), NotYet> {
    emit_init(m, st, p, module, "_init_vars", true, false)
}

/// `<maschine>_enter`: die `enter:`-Kette und der Entry-Tick (5.2).
pub fn enter_function(m: &Machine, st: &StateStruct, p: &Program, module: &mut Module) -> Result<(), NotYet> {
    emit_init(m, st, p, module, "_enter", false, true)
}

/// `<maschine>_exit_all(st, in, par, out)`: die `exit:`-Bloecke der ganzen
/// Konfiguration, von innen nach aussen (5.11).
///
/// Eine gescopte Instanz verlaesst bei einem regulaeren Uebergang ihres
/// Besitzers jeden Zustand, in dem sie steht. Es gibt kein Ziel, darum
/// kein `switch`: Der Zweig haengt am aktuellen Blatt, und der Pfad
/// dorthin steht statisch fest. Wirksam ist das fuer `log` und Signale —
/// die Outputs der Instanz gehen danach ohnehin auf `safe` (5.11).
pub fn exit_all_function(m: &Machine, st: &StateStruct, p: &Program, module: &mut Module) -> Result<(), NotYet> {
    let leaves = machine::leaves(m);
    let has_exit = m.states.iter().any(|s| !s.exit.stmts.is_empty());
    let mark = module.mark();
    let ptr = crate::ty::LlvmType::Ptr;
    module.begin(
        &format!("{}_exit_all", m.name),
        &crate::ty::LlvmType::Void,
        &[ptr.clone(), ptr.clone(), ptr.clone(), ptr],
    );
    if !has_exit || leaves.is_empty() {
        module.end(None);
        return Ok(());
    }
    let state_ty = format!("%{}_state", crate::fns::sanitized(&m.name));
    let Some(conf_i) = st.index_of(Role::Conf, 0) else {
        module.abort(mark);
        return Err(NotYet { what: "conf im Zustand" });
    };
    let conf = module.inst(&format!("getelementptr inbounds {state_ty}, ptr %0, i32 0, i32 {conf_i}"));
    let slot = module.inst(&format!("getelementptr inbounds [{} x i8], ptr {conf}, i32 0, i32 0", st.depth));
    let cur = module.inst(&format!("load i8, ptr {slot}"));
    let end = format!("exit_all_{}_end", crate::fns::sanitized(&m.name));
    let mut ctx = Ctx::new(m, st, p);
    for (i, leaf) in leaves.iter().enumerate() {
        let hit = format!("exit_all_{}_{i}", crate::fns::sanitized(&m.name));
        let next = format!("exit_all_{}_n{i}", crate::fns::sanitized(&m.name));
        let eq = module.inst(&format!("icmp eq i8 {cur}, {i}"));
        module.void_inst(&format!("br i1 {eq}, label %{hit}, label %{next}"));
        module.label(&hit);
        ctx.leaf = Some(*leaf);
        for id in machine::path_to(m, *leaf).iter().rev() {
            let b = m.states[id.index()].exit.clone();
            if let Err(e) = block(&b, &mut ctx, module) {
                module.abort(mark);
                return Err(e);
            }
        }
        module.void_inst(&format!("br label %{end}"));
        module.label(&next);
    }
    module.void_inst(&format!("br label %{end}"));
    module.label(&end);
    module.end(None);
    Ok(())
}

fn emit_init(
    m: &Machine,
    st: &StateStruct,
    p: &Program,
    module: &mut Module,
    suffix: &str,
    vars: bool,
    enter: bool,
) -> Result<(), NotYet> {
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
        &format!("{}{suffix}", m.name),
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
    if vars {
        module.void_inst(&format!("store i8 {index}, ptr {slot}"));
    }

    let mut ctx = Ctx::new(m, st, p);
    ctx.leaf = Some(leaf);
    // 9.4: `s0` sind die Anfangswerte der Variablen. Sie stehen *vor*
    // jedem `enter:`, weil ein `enter:`-Block sie schon lesen darf (1.4)
    // — und ohne sie stuende dort die Null, die der Speicher mitbringt.
    if vars {
        for (i, v) in m.vars.iter().enumerate() {
            let Some(init) = v.init.clone() else { continue };
            let id = takt_mir::VarId(i as u32);
            // Eine Blockinstanz: die Parameter aus den Argumenten, der
            // Zustand aus den Initialwerten des Blocks (5.7).
            if let Some(b) = machine::instance_block(m, id) {
                if let Err(e) = init_instance(b, &init, i, &mut ctx, module) {
                    module.abort(mark);
                    return Err(e);
                }
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
    }
    if enter {
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
        // Die Zaehler der betretenen Zustaende und die der Maschinenebene
        // beginnen bei `-1` („noch nicht gesetzt", 5.8). Der Speicher kommt
        // genullt, und die Null waere ein gueltiger Zeitpunkt — das `every`
        // liefe dann schon im Tick 0 statt nach `d`.
        reset_counters(&ctx, None, module);
        for id in &kette {
            reset_counters(&ctx, Some(*id), module);
        }
        // 5.12: Kein gespeicherter Pfad vor dem ersten Austritt; die Null
        // des genullten Speichers waere ein gueltiges Blatt.
        for slot in 0..m.layout.saved_paths.len() {
            if let Some(ptr) = ctx.field(Role::Saved, slot, module) {
                module.void_inst(&format!("store i32 -1, ptr {ptr}"));
            }
        }
        let mut koerper = vec![m.loop_block.clone()];
        koerper.extend(kette.iter().map(|id| m.states[id.index()].loop_block.clone()));
        let end_at = format!("init_ende_{}", m.name);
        for b in &koerper {
            if let Err(e) = block(b, &mut ctx, module) {
                module.abort(mark);
                return Err(e);
            }
        }
        module.void_inst(&format!("br label %{end_at}"));
        // Der Fault-Pfad des Anfangszustands: Ein `check`, der schon im
        // Tick 0 scheitert, fuehrt zum Fault-Ziel (5.2 Regel 5). Ohne ihn
        // spraenge der Zweig ins Leere — die Marke steht nur im Schritt.
        if let Err(e) = fault_path(st, leaf, &Jump { leaves: &leaves, end: &end_at, conf: &slot }, &mut ctx, module) {
            module.abort(mark);
            return Err(e);
        }
        module.label(&end_at);
        // 9.4: Tick 0 schreibt den Zaehler fort „wie am Ende jedes Ticks"
        // (`System::init` ruft `advance_counters`). Ohne das misst der
        // erzeugte Code eine Frist um einen Tick zu lang: Der Interpreter
        // steht zu Beginn von Tick 1 bei `t_in_state == 1`, der Code bei 0,
        // und `after 30 ms` feuert bei 10 ms Tick erst in Tick 4 statt 3.
        advance_time(&ctx, module);
    }
    module.end(None);
    Ok(())
}

/// Schreibt `t_in_state` um einen Tick fort (9.4).
///
/// Der Zaehler misst die Ticks *elapsed* dem Eintritt; er waechst am Ende
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
    let deadline = match d.kind {
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
    let reached = m.inst(&format!("icmp sge i64 {elapsed}, {}", deadline.value));
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
fn dispatch(handlers: &[takt_mir::machine::Handler], ctx: &mut Ctx<'_>, m: &mut Module) -> Result<(), NotYet> {
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
        let (cur_ptr, ex_ptr) = ctx.vars().stream_slots(stream, m).ok_or(NotYet { what: "Cursor eines Stroms" })?;
        let sid = crate::stream::number(stream).ok_or(NotYet { what: "Strom ohne feste Nummer" })?;
        let elem = crate::stream::element(ctx.program, stream).ok_or(NotYet { what: "Elementtyp eines Stroms" })?;
        let k = ctx.next_label();
        let cur = m.inst(&format!("load i64, ptr {cur_ptr}"));
        let n = m.inst(&format!("call i32 @{}(i32 {sid}, i64 {cur})", crate::stream::Streams::COUNT));
        // Der Zaehler laeuft ueber das Fenster; seine Schranke ist `n`.
        let i_ptr = m.inst("alloca i32");
        m.void_inst(&format!("store i32 0, ptr {i_ptr}"));
        // Das Element kommt in einen Scratch; die Bindungen fuellt
        // `bind_element` je Handler (8.7).
        let buf = crate::stream::scratch(ctx.program, elem, m)?;
        let (head, body, end_at) = (format!("strom{k}"), format!("strom{k}_rumpf"), format!("strom{k}_ende"));
        m.void_inst(&format!("br label %{head}"));
        m.label(&head);
        let i = m.inst(&format!("load i32, ptr {i_ptr}"));
        let go_on = m.inst(&format!("icmp slt i32 {i}, {n}"));
        m.void_inst(&format!("br i1 {go_on}, label %{body}, label %{end_at}"));
        m.label(&body);
        let seq =
            m.inst(&format!("call i64 @{}(i32 {sid}, i64 {cur}, i32 {i}, ptr {buf})", crate::stream::Streams::AT));
        // 9.6: Auch ein Element ohne passenden Handler gilt als
        // untersucht — sonst saehe die Maschine es im naechsten Tick
        // wieder.
        let mi = ctx.machine_index;
        m.void_inst(&format!("call void @{}(i32 {sid}, i32 {mi}, i64 {seq})", crate::stream::Streams::EXAMINED));
        crate::stream::note_examined(ex_ptr, seq, m);
        // 8.7: Der erste passende Handler gewinnt. Ohne Muster ist das
        // immer der erste — weitere kaemen nie zum Zug. Mit Muster wird
        // daraus eine Kette: Je Handler prueft der Automat, und wer
        // trifft, laeuft; die uebrigen springen ans Ende.
        let hs: Vec<&takt_mir::machine::Handler> = handlers.iter().filter(|h| h.stream == stream).collect();
        handler_chain(&hs, buf, seq, elem, ctx, m)?;
        let cur_i = m.inst(&format!("load i32, ptr {i_ptr}"));
        let next = m.inst(&format!("add i32 {cur_i}, 1"));
        m.void_inst(&format!("store i32 {next}, ptr {i_ptr}"));
        m.void_inst(&format!("br label %{head}"));
        m.label(&end_at);
    }
    Ok(())
}

/// `s as e` als Guard (8.6): das naechste Element des Fensters, wenn es
/// eines gibt — wie `first_match` ohne Muster: binden, untersuchen, wahr.
fn next_element(
    stream: takt_mir::expr::StreamRef,
    binding: takt_mir::VarId,
    ctx: &mut Ctx<'_>,
    m: &mut Module,
) -> Result<crate::expr::Lowered, NotYet> {
    let (cur_ptr, ex_ptr) = ctx.vars().stream_slots(stream, m).ok_or(NotYet { what: "Cursor eines Stroms" })?;
    let sid = crate::stream::number(stream).ok_or(NotYet { what: "Strom ohne feste Nummer" })?;
    let elem = crate::stream::element(ctx.program, stream).ok_or(NotYet { what: "Elementtyp eines Stroms" })?;
    let mi = ctx.machine_index;
    let buf = crate::stream::scratch(ctx.program, elem, m)?;
    let k = ctx.next_label();
    let name = ctx.machine.name.clone();
    let cur = m.inst(&format!("load i64, ptr {cur_ptr}"));
    let n = m.inst(&format!("call i32 @{}(i32 {sid}, i64 {cur})", crate::stream::Streams::COUNT));
    let some = m.inst(&format!("icmp sgt i32 {n}, 0"));
    let (take, done) = (format!("naechstes{k}_{name}"), format!("naechstes{k}_{name}_fertig"));
    m.void_inst(&format!("br i1 {some}, label %{take}, label %{done}"));
    m.label(&take);
    let seq = m.inst(&format!("call i64 @{}(i32 {sid}, i64 {cur}, i32 0, ptr {buf})", crate::stream::Streams::AT));
    bind_element(binding, buf, seq, elem, ctx, m)?;
    m.void_inst(&format!("call void @{}(i32 {sid}, i32 {mi}, i64 {seq})", crate::stream::Streams::EXAMINED));
    crate::stream::note_examined(ex_ptr, seq, m);
    m.void_inst(&format!("br label %{done}"));
    m.label(&done);
    Ok(crate::expr::Lowered { value: some.to_string(), ty: crate::ty::LlvmType::Int(1) })
}

/// Legt das Element aus dem Scratch in die Bindung (8.7, Wrapper-Regel):
/// `t` und `seq` hinter den Captures, dann der Inhalt als `text`
/// beziehungsweise `data`. Die Captures schreibt `captures::walk`.
pub(crate) fn bind_element(
    var: takt_mir::VarId,
    buf: crate::emit::Reg,
    seq: crate::emit::Reg,
    elem: takt_mir::TypeId,
    ctx: &mut Ctx<'_>,
    m: &mut Module,
) -> Result<(), NotYet> {
    let p = ctx.program;
    let ty = ctx.machine.vars.get(var.index()).map(|v| v.ty).ok_or(NotYet { what: "Bindung ohne Typ" })?;
    let record = crate::ty::lower(ty, p).ok_or(NotYet { what: "Typ der Bindung" })?;
    let Some(takt_mir::types::Type::Record(r)) = p.types.list.get(ty.index()) else {
        return Err(NotYet { what: "Bindung ohne Record" });
    };
    let slot = ctx.field(Role::Var, var.index(), m).ok_or(NotYet { what: "Bindung im Zustand" })?;
    for (i, def) in p.records[r.index()].fields.iter().enumerate() {
        let at = |m: &mut Module| m.inst(&format!("getelementptr inbounds {record}, ptr {slot}, i32 0, i32 {i}"));
        match def.name.as_str() {
            "t" => {
                let t = m.inst(&format!("load i64, ptr {buf}"));
                let dst = at(m);
                m.void_inst(&format!("store i64 {t}, ptr {dst}"));
            }
            "seq" => {
                let dst = at(m);
                m.void_inst(&format!("store i64 {seq}, ptr {dst}"));
            }
            "text" | "data" => {
                let dst = at(m);
                crate::stream::copy_payload(buf, dst, elem, p, m)?;
            }
            _ => {}
        }
    }
    Ok(())
}

/// Ein Record-Muster (8.7): die Felder des dekodierten Elements gegen die
/// konstanten Werte des Musters.
fn record_hit(
    record: takt_mir::RecordId,
    fields: &[(u32, Expr)],
    buf: crate::emit::Reg,
    elem: takt_mir::TypeId,
    ctx: &mut Ctx<'_>,
    m: &mut Module,
) -> Result<crate::emit::Reg, NotYet> {
    let p = ctx.program;
    let rec = crate::ty::lower(elem, p).ok_or(NotYet { what: "Typ des Elements" })?;
    let tmp = m.inst(&format!("alloca {rec}"));
    let src = m.inst(&format!("getelementptr inbounds i8, ptr {buf}, i64 {}", crate::stream::Streams::BYTES_AT));
    crate::persist::decode_canonical(p, elem, src, tmp, m)?;
    let mut hit: Option<crate::emit::Reg> = None;
    for (i, e) in fields {
        let def = p.records.get(record.index()).and_then(|r| r.fields.get(*i as usize));
        let fty = crate::ty::lower(def.ok_or(NotYet { what: "Feld des Musters" })?.ty, p)
            .ok_or(NotYet { what: "Feldtyp des Musters" })?;
        let cmp = match fty {
            crate::ty::LlvmType::Int(_) => "icmp eq",
            crate::ty::LlvmType::F32 | crate::ty::LlvmType::F64 => "fcmp oeq",
            _ => return Err(NotYet { what: "Record-Muster auf einem zusammengesetzten Feld" }),
        };
        let at = m.inst(&format!("getelementptr inbounds {rec}, ptr {tmp}, i32 0, i32 {i}"));
        let have = m.inst(&format!("load {fty}, ptr {at}"));
        let want = lower_expr(e, p, m, &ctx.vars())?;
        let eq = m.inst(&format!("{cmp} {fty} {have}, {}", want.value));
        hit = Some(match hit {
            Some(h) => m.inst(&format!("and i1 {h}, {eq}")),
            None => eq,
        });
    }
    Ok(hit.unwrap_or_else(|| m.inst("and i1 true, true")))
}

/// Die Handler eines Stroms als Kette (8.7).
///
/// „Der erste passende Handler gewinnt": Je Handler entsteht eine
/// Pruefung und ein Rumpf, und wer trifft, springt ans Ende der Kette.
/// Ein Catch-all beendet sie — was danach kaeme, liefe nie.
fn handler_chain(
    hs: &[&takt_mir::machine::Handler],
    buf: crate::emit::Reg,
    seq: crate::emit::Reg,
    elem: takt_mir::TypeId,
    ctx: &mut Ctx<'_>,
    m: &mut Module,
) -> Result<(), NotYet> {
    let k = ctx.next_label();
    let name = &ctx.machine.name;
    let end_at = format!("handler{k}_{name}_ende");
    for (n, h) in hs.iter().enumerate() {
        if let Some(v) = h.binding {
            bind_element(v, buf, seq, elem, ctx, m)?;
        }
        let Some((kind, pattern)) = &h.pattern else {
            // Catch-all: Er laeuft immer, und die Kette endet hier — es
            // sei denn, ein Guard (FB-14) laesst das Element weiter.
            if let Some(g) = &h.guard {
                let ok = lower_expr(g, ctx.program, m, &ctx.vars())?;
                let (then_l, else_l) = (format!("handler{k}_{n}_{name}"), format!("handler{k}_{n}_{name}_sonst"));
                m.void_inst(&format!("br i1 {}, label %{then_l}, label %{else_l}", ok.value));
                m.label(&then_l);
                block(&h.body.clone(), ctx, m)?;
                m.void_inst(&format!("br label %{end_at}"));
                m.label(&else_l);
                continue;
            }
            block(&h.body.clone(), ctx, m)?;
            m.void_inst(&format!("br label %{end_at}"));
            m.label(&end_at);
            return Ok(());
        };
        let hit = match pattern {
            takt_mir::pattern::Pattern::Record { record, fields } => record_hit(*record, fields, buf, elem, ctx, m)?,
            takt_mir::pattern::Pattern::Text { pieces, dfa } => {
                // Der Text steht im Scratch als `{ i32 len, [N x i8] }`;
                // der Vergleich laeuft darauf, die Captures gehen in die
                // Bindung (8.7).
                let text =
                    m.inst(&format!("getelementptr inbounds i8, ptr {buf}, i64 {}", crate::stream::Streams::LEN_AT));
                let hat_capture = pieces.iter().any(|p| matches!(p, takt_mir::pattern::PatternPiece::Capture { .. }));
                // 8.7: `matches` verlangt den ganzen Text, `has` ein Vorkommen.
                //
                // Ohne Platzhalter genuegt der Automat: Er liest jedes Byte
                // einmal und sagt, ob das Muster traegt (11.2). Mit Platzhaltern
                // braucht es den Durchlauf, denn ein Automat ueber Zeichenklassen
                // kennt die Grenzen, aber nicht die Werte — und ihn zusaetzlich
                // laufen zu lassen hiesse, denselben Text zweimal zu lesen.
                let ist_matches = *kind == takt_mir::expr::MatchKind::Matches;
                match (hat_capture, ist_matches, dfa) {
                    (false, true, Some(dfa)) => {
                        let id = m.next_label();
                        crate::dfa::declare(id, dfa, m);
                        crate::dfa::run(id, dfa, text, m)?
                    }
                    (_, true, _) => {
                        let b = Binding::of(h, ctx);
                        pattern_matches(pieces, text, &b, ctx, m)?
                    }
                    (_, false, _) => {
                        let b = Binding::of(h, ctx);
                        pattern_has(pieces, text, &b, ctx, m)?
                    }
                }
            }
        };
        let (then_l, else_l) = (format!("handler{k}_{n}_{name}"), format!("handler{k}_{n}_{name}_sonst"));
        m.void_inst(&format!("br i1 {hit}, label %{then_l}, label %{else_l}"));
        m.label(&then_l);
        // FB-14: Der Guard sieht die Bindung; `false` reicht das Element
        // an den naechsten Handler weiter.
        if let Some(g) = &h.guard {
            let ok = lower_expr(g, ctx.program, m, &ctx.vars())?;
            let body_l = format!("handler{k}_{n}_{name}_rumpf");
            m.void_inst(&format!("br i1 {}, label %{body_l}, label %{else_l}", ok.value));
            m.label(&body_l);
        }
        block(&h.body.clone(), ctx, m)?;
        m.void_inst(&format!("br label %{end_at}"));
        m.label(&else_l);
    }
    m.void_inst(&format!("br label %{end_at}"));
    m.label(&end_at);
    Ok(())
}

/// Wohin die Werte eines Musters gehen (8.7, Wrapper-Regel).
///
/// Die Bindung ist ein Record, dessen erste Felder die Platzhalter sind;
/// dahinter stehen `t`, `seq` und der Inhalt.
struct Binding {
    /// Die Variable im Zustand der Maschine.
    var: takt_mir::VarId,
    /// Ihr Typ, fuer die Adressrechnung.
    record: crate::ty::LlvmType,
    /// Index und Typ je Capture, in Musterreihenfolge.
    fields: Vec<(u32, crate::ty::LlvmType)>,
}

impl Binding {
    /// `None` heisst: keine Bindung, also nichts abzulegen — der
    /// Vergleich laeuft trotzdem.
    fn of(h: &takt_mir::machine::Handler, ctx: &Ctx<'_>) -> Option<Binding> {
        Binding::of_var(h.binding?, ctx)
    }

    /// Dieselbe Rechnung fuer eine Bindung, die nicht an einem Handler
    /// haengt — ein Muster-Guard in einem Uebergang bindet ebenso (8.7).
    fn of_var(var: takt_mir::VarId, ctx: &Ctx<'_>) -> Option<Binding> {
        let ty = ctx.machine.vars.get(var.index())?.ty;
        let record = crate::ty::lower(ty, ctx.program)?;
        let crate::ty::LlvmType::Struct(fields) = &record else { return None };
        let takt_mir::types::Type::Record(r) = ctx.program.types.list.get(ty.index())? else { return None };
        let defs = &ctx.program.records.get(r.index())?.fields;
        // Die Captures stehen vorn; `t`, `seq` und `text`/`data`
        // schliessen an. Die Grenze ist der erste dieser Namen.
        let end_at =
            defs.iter().position(|d| matches!(d.name.as_str(), "t" | "seq" | "text" | "data")).unwrap_or(defs.len());
        let fields = (0..end_at).filter_map(|i| fields.get(i).map(|f| (i as u32, f.clone()))).collect();
        Some(Binding { var, record, fields })
    }

    /// Der Platz im Zustand, an den die Werte gehen.
    fn slot(&self, ctx: &mut Ctx<'_>, m: &mut Module) -> Result<crate::emit::Reg, NotYet> {
        ctx.field(Role::Var, self.var.index(), m).ok_or(NotYet { what: "Bindung im Zustand" })
    }
}

/// Was `captures::walk` braucht, oder nichts.
fn target<'a>(
    b: &'a Option<Binding>,
    ctx: &mut Ctx<'_>,
    m: &mut Module,
) -> Result<Option<crate::captures::Target<'a>>, NotYet> {
    match b {
        Some(b) => {
            let slot = b.slot(ctx, m)?;
            Ok(Some(crate::captures::Target { slot, record: &b.record, fields: &b.fields }))
        }
        None => Ok(None),
    }
}

/// Ein Muster-Guard in einem Uebergang (8.7, `until … matches`).
///
/// **Er sucht das erste passende Element und haelt dort an.** Der
/// Interpreter tut es in `first_match`: ueber das Fenster laufen, beim
/// ersten Treffer binden, `examined` auf dessen `seq` setzen und `true`
/// liefern. Was danach kommt, bleibt *unkonsumiert* — anders als beim
/// Handler-Dispatch, der jedes Element untersucht (9.7). Ein Guard ist
/// eine Frage an das Fenster, keine Verarbeitung.
///
/// `examined` rueckt darum nur bis zum Treffer; am Ende des Schritts
/// wird das Maximum ueber alle Konstrukte der Aktivierung zum Cursor
/// (9.7), und das Fenster bleibt fuer den ganzen Tick dasselbe.
fn match_guard(
    subject: &takt_mir::expr::Expr,
    kind: takt_mir::expr::MatchKind,
    pattern: &takt_mir::pattern::Pattern,
    binding: Option<takt_mir::VarId>,
    ctx: &mut Ctx<'_>,
    m: &mut Module,
) -> Result<crate::expr::Lowered, NotYet> {
    // Derselbe Schluss wie `stream_of` im Interpreter: Ein Channel mit
    // Stromtyp und ein interner Strom sind beide ein Fenster.
    let stream = match &subject.kind {
        takt_mir::expr::ExprKind::Input { channel, .. } => takt_mir::expr::StreamRef::Channel(*channel),
        takt_mir::expr::ExprKind::Stream(s) => takt_mir::expr::StreamRef::Internal(*s),
        // 8.7 laesst ein Muster auch auf einem gewoehnlichen Wert zu; der
        // Fall braucht keinen Fensterzugriff und kommt mit dem Bedarf.
        _ => return Err(NotYet { what: "Muster-Guard auf einem Nicht-Strom" }),
    };
    let (cur_ptr, ex_ptr) = ctx.vars().stream_slots(stream, m).ok_or(NotYet { what: "Cursor eines Stroms" })?;
    let sid = crate::stream::number(stream).ok_or(NotYet { what: "Strom ohne feste Nummer" })?;
    let elem = crate::stream::element(ctx.program, stream).ok_or(NotYet { what: "Elementtyp eines Stroms" })?;
    let b = binding.and_then(|v| Binding::of_var(v, ctx));
    let buf = crate::stream::scratch(ctx.program, elem, m)?;

    let k = ctx.next_label();
    let name = &ctx.machine.name;
    let cur = m.inst(&format!("load i64, ptr {cur_ptr}"));
    let n = m.inst(&format!("call i32 @{}(i32 {sid}, i64 {cur})", crate::stream::Streams::COUNT));
    let i_ptr = m.inst("alloca i32");
    m.void_inst(&format!("store i32 0, ptr {i_ptr}"));
    let hit_ptr = m.inst("alloca i1");
    m.void_inst(&format!("store i1 false, ptr {hit_ptr}"));

    let (head, body, done) =
        (format!("guard{k}_{name}"), format!("guard{k}_{name}_rumpf"), format!("guard{k}_{name}_fertig"));
    m.void_inst(&format!("br label %{head}"));
    m.label(&head);
    let i = m.inst(&format!("load i32, ptr {i_ptr}"));
    let in_window = m.inst(&format!("icmp slt i32 {i}, {n}"));
    let found = m.inst(&format!("load i1, ptr {hit_ptr}"));
    let still = m.inst(&format!("xor i1 {found}, true"));
    let go_on = m.inst(&format!("and i1 {in_window}, {still}"));
    m.void_inst(&format!("br i1 {go_on}, label %{body}, label %{done}"));

    m.label(&body);
    let seq = m.inst(&format!("call i64 @{}(i32 {sid}, i64 {cur}, i32 {i}, ptr {buf})", crate::stream::Streams::AT));
    if let Some(v) = binding {
        bind_element(v, buf, seq, elem, ctx, m)?;
    }
    let text = m.inst(&format!("getelementptr inbounds i8, ptr {buf}, i64 {}", crate::stream::Streams::LEN_AT));
    let ok = match (pattern, kind) {
        (takt_mir::pattern::Pattern::Record { record, fields }, _) => record_hit(*record, fields, buf, elem, ctx, m)?,
        (takt_mir::pattern::Pattern::Text { pieces, .. }, takt_mir::expr::MatchKind::Matches) => {
            pattern_matches(pieces, text, &b, ctx, m)?
        }
        (takt_mir::pattern::Pattern::Text { pieces, .. }, takt_mir::expr::MatchKind::Has) => {
            pattern_has(pieces, text, &b, ctx, m)?
        }
    };
    // 8.7: Nur das *passende* Element gilt als untersucht — der Guard
    // haelt dort an, und die uebrigen bleiben im Fenster.
    let (mark, next) = (format!("guard{k}_{name}_treffer"), format!("guard{k}_{name}_weiter"));
    m.void_inst(&format!("br i1 {ok}, label %{mark}, label %{next}"));
    m.label(&mark);
    let mi = ctx.machine_index;
    m.void_inst(&format!("call void @{}(i32 {sid}, i32 {mi}, i64 {seq})", crate::stream::Streams::EXAMINED));
    crate::stream::note_examined(ex_ptr, seq, m);
    m.void_inst(&format!("store i1 true, ptr {hit_ptr}"));
    m.void_inst(&format!("br label %{next}"));

    m.label(&next);
    let cur_i = m.inst(&format!("load i32, ptr {i_ptr}"));
    let inc = m.inst(&format!("add i32 {cur_i}, 1"));
    m.void_inst(&format!("store i32 {inc}, ptr {i_ptr}"));
    m.void_inst(&format!("br label %{head}"));

    m.label(&done);
    let result = m.inst(&format!("load i1, ptr {hit_ptr}"));
    Ok(crate::expr::Lowered { value: result.to_string(), ty: crate::ty::LlvmType::Int(1) })
}

/// `matches P`: Das Muster muss den ganzen Text verbrauchen (8.7).
fn pattern_matches(
    pieces: &[takt_mir::pattern::PatternPiece],
    text: crate::emit::Reg,
    b: &Option<Binding>,
    ctx: &mut Ctx<'_>,
    m: &mut Module,
) -> Result<crate::emit::Reg, NotYet> {
    let into = target(b, ctx, m)?;
    text_matches(pieces, text, &into, ctx, m)
}

/// Wie [`pattern_matches`], mit fertigem Ziel fuer die Captures.
fn text_matches(
    pieces: &[takt_mir::pattern::PatternPiece],
    text: crate::emit::Reg,
    into: &Option<crate::captures::Target<'_>>,
    ctx: &mut Ctx<'_>,
    m: &mut Module,
) -> Result<crate::emit::Reg, NotYet> {
    let _ = ctx;
    let zero = m.inst("add i32 0, 0");
    let (ok, at) = crate::captures::walk(pieces, text, into.as_ref(), zero, m)?;
    // Der ganze Text: Was hinter dem Durchlauf steht, darf nicht sein.
    let len_ptr = m.inst(&format!("getelementptr inbounds i8, ptr {text}, i64 0"));
    let len = m.inst(&format!("load i32, ptr {len_ptr}"));
    let whole = m.inst(&format!("icmp eq i32 {at}, {len}"));
    Ok(m.inst(&format!("and i1 {ok}, {whole}")))
}

/// `has P`: Das Muster darf an jeder Stelle beginnen (8.7).
///
/// Gesucht wird das linkeste Vorkommen. Die Schleife ist durch die
/// Textlaenge beschraenkt, die ihrerseits durch `N` beschraenkt ist
/// (3.9) — 4.1 verlangt genau das.
fn pattern_has(
    pieces: &[takt_mir::pattern::PatternPiece],
    text: crate::emit::Reg,
    b: &Option<Binding>,
    ctx: &mut Ctx<'_>,
    m: &mut Module,
) -> Result<crate::emit::Reg, NotYet> {
    let into = target(b, ctx, m)?;
    text_has(pieces, text, &into, ctx, m)
}

/// Wie [`pattern_has`], mit fertigem Ziel fuer die Captures.
fn text_has(
    pieces: &[takt_mir::pattern::PatternPiece],
    text: crate::emit::Reg,
    into: &Option<crate::captures::Target<'_>>,
    ctx: &mut Ctx<'_>,
    m: &mut Module,
) -> Result<crate::emit::Reg, NotYet> {
    let _ = ctx;
    let k = m.next_label();
    let (head, body, done) = (format!("has{k}"), format!("has{k}_rumpf"), format!("has{k}_fertig"));
    let len_ptr = m.inst(&format!("getelementptr inbounds i8, ptr {text}, i64 0"));
    let len = m.inst(&format!("load i32, ptr {len_ptr}"));
    let start_ptr = m.inst("alloca i32");
    m.void_inst(&format!("store i32 0, ptr {start_ptr}"));
    let hit_ptr = m.inst("alloca i1");
    m.void_inst(&format!("store i1 false, ptr {hit_ptr}"));
    m.void_inst(&format!("br label %{head}"));

    m.label(&head);
    let start = m.inst(&format!("load i32, ptr {start_ptr}"));
    // Auch hinter dem letzten Zeichen wird geprueft: Ein leeres Muster
    // passt am Ende (8.7).
    let in_text = m.inst(&format!("icmp sle i32 {start}, {len}"));
    let bisher = m.inst(&format!("load i1, ptr {hit_ptr}"));
    let open_still = m.inst(&format!("xor i1 {bisher}, true"));
    let searching = m.inst(&format!("and i1 {in_text}, {open_still}"));
    m.void_inst(&format!("br i1 {searching}, label %{body}, label %{done}"));

    m.label(&body);
    let (ok, _) = crate::captures::walk(pieces, text, into.as_ref(), start, m)?;
    m.void_inst(&format!("store i1 {ok}, ptr {hit_ptr}"));
    let next_i = m.inst(&format!("add i32 {start}, 1"));
    m.void_inst(&format!("store i32 {next_i}, ptr {start_ptr}"));
    m.void_inst(&format!("br label %{head}"));

    m.label(&done);
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
    target: &Jump<'_>,
    ctx: &mut Ctx<'_>,
    m: &mut Module,
) -> Result<(), NotYet> {
    let machine_def = ctx.machine;
    let (leaves, end, conf_slot) = (target.leaves, target.end, target.conf);
    m.label(&format!("fault_{}_{}", machine_def.name, from.index()));
    m.void_inst(&format!("call void @{}(i32 {}, i32 {})", crate::abi::Abi::FAULT, ctx.machine_index, from.index()));
    // Der Fault wird vorgemerkt; `pending` traegt ihn fuer die
    // Abort-Phase (5.4), die die Runtime fuehrt.
    if let Some(pending) = st.index_of(Role::Pending, 0) {
        let state_ty = format!("%{}_state", crate::fns::sanitized(&machine_def.name));
        let field = m.inst(&format!("getelementptr inbounds {state_ty}, ptr %0, i32 0, i32 {pending}"));
        let flag = m.inst(&format!("getelementptr inbounds {{ i1, i32, i32 }}, ptr {field}, i32 0, i32 0"));
        m.void_inst(&format!("store i1 true, ptr {flag}"));
    }
    // 5.3: Ein Fault-Uebergang bricht die laufenden Jobs der Maschine ab.
    for slot in 0..machine_def.layout.job_slots.len() {
        m.void_inst(&format!("call void @{}(i32 {}, i32 {slot})", crate::abi::Abi::JOB_CANCEL, ctx.machine_index));
    }
    let target = machine_def.fault_target_of(from);
    let takt_mir::machine::FaultTarget::State(to) = target else {
        // `FAULTED` fuehrt keinen Nutzercode aus (5.2 Regel 5), und die
        // Outputs gehen auf `safe` — noch in diesem Tick, nicht erst im
        // naechsten: Der Interpreter tut es an derselben Stelle, und ein
        // Ventil, das remaining stand, bliebe sonst einen Tick laenger remaining.
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
    save_paths(ctx, m, from, leaf);
    for id in machine::exiting(machine_def, from, leaf) {
        block(&machine_def.states[id.index()].exit.clone(), ctx, m)?;
    }
    for id in machine::entering(machine_def, from, leaf) {
        block(&machine_def.states[id.index()].enter.clone(), ctx, m)?;
    }
    m.void_inst(&format!("store i8 {index}, ptr {conf_slot}"));
    reset_time(ctx, m);
    // Auch der Fault-Pfad betritt einen Zustand: seine Zaehler beginnen
    // neu (5.8, 5.6).
    for id in machine::entering(machine_def, from, leaf) {
        reset_counters(ctx, Some(id), m);
    }
    // Entry-Modus: Die `loop:`-Bloecke des Fault-Ziels laufen noch in
    // diesem Tick (5.2 Regel 4 und 5) — und ein `->` darin ist dort
    // wirkungslos, wie in jedem Entry-Tick.
    let saved = ctx.end.take();
    for id in machine::entering(machine_def, from, leaf) {
        block(&machine_def.states[id.index()].loop_block.clone(), ctx, m)?;
    }
    ctx.end = saved;
    m.void_inst(&format!("br label %{end}"));
    Ok(())
}

/// Wohin ein Fault-Pfad fuehrt und wo er endet.
///
/// Die drei gehoeren zusammen: Sie beschreiben denselben Zweig, und
/// einzeln durchgereicht waeren sie drei Gelegenheiten, den falschen zu
/// nehmen.
struct Jump<'a> {
    /// Die Blattzustaende der Maschine, fuer die Nummer des Ziels.
    leaves: &'a [StateId],
    /// Die Marke am Ende des Schritts.
    end: &'a str,
    /// Der Zeiger auf `conf[0]`.
    conf: &'a crate::emit::Reg,
}

/// `<besitzer>_triggers(st, in, par, out)`: die Trigger-Phase (7.5).
///
/// Sie steht als eigene Funktion, nicht im Schritt: Der Rahmen ruft sie
/// vor den Maschinenschritten, wie der Interpreter seine Phase zwischen
/// Zustellung und Schritt legt. Ihr Zustand liegt trotzdem im Struct des
/// Besitzers — `armed` gehoert laut 7.5 ihm, und der Cursor daneben
/// spart ein zweites Schema.
///
/// Je armiertem Trigger laeuft das Fenster seines Quellstroms; das erste
/// passende Element plant die Ausgaben, loescht `armed` und legt sich als
/// Element in `fired`.
pub fn trigger_function(m: &Machine, st: &StateStruct, p: &Program, module: &mut Module) -> Result<(), NotYet> {
    let mine: Vec<(usize, &takt_mir::program::Trigger)> = p
        .triggers
        .iter()
        .enumerate()
        .filter(|(i, _)| m.layout.trigger_flags.contains(&takt_mir::TriggerId(*i as u32)))
        .collect();
    let mark = module.mark();
    let ptr = crate::ty::LlvmType::Ptr;
    module.begin(
        &format!("{}_triggers", m.name),
        &crate::ty::LlvmType::Void,
        &[ptr.clone(), ptr.clone(), ptr.clone(), ptr],
    );
    if mine.is_empty() {
        module.end(None);
        return Ok(());
    }
    let mut ctx = Ctx::new(m, st, p);
    for (i, t) in mine {
        if let Err(e) = one_trigger(takt_mir::TriggerId(i as u32), t, &mut ctx, module) {
            module.abort(mark);
            return Err(e);
        }
    }
    module.end(None);
    Ok(())
}

/// Ein Trigger: Fenster durchlaufen, beim ersten Treffer planen (7.5).
fn one_trigger(
    id: takt_mir::TriggerId,
    t: &takt_mir::program::Trigger,
    ctx: &mut Ctx<'_>,
    m: &mut Module,
) -> Result<(), NotYet> {
    let takt_mir::machine::Guard::Match { subject, kind, pattern, .. } = &t.guard else {
        return Err(NotYet { what: "Trigger ohne Mustern-Guard" });
    };
    let stream = match &subject.kind {
        takt_mir::expr::ExprKind::Input { channel, .. } => takt_mir::expr::StreamRef::Channel(*channel),
        takt_mir::expr::ExprKind::Stream(s) => takt_mir::expr::StreamRef::Internal(*s),
        _ => return Err(NotYet { what: "Trigger ohne Quellstrom" }),
    };
    let (armed_ptr, cur_ptr) = ctx.vars().trigger_slots(id, m).ok_or(NotYet { what: "Trigger im Zustand" })?;
    let sid = crate::stream::number(stream).ok_or(NotYet { what: "Strom ohne feste Nummer" })?;
    let elem = crate::stream::element(ctx.program, stream).ok_or(NotYet { what: "Elementtyp eines Stroms" })?;
    let k = ctx.next_label();
    let name = crate::fns::sanitized(&t.name);
    let (skip, head, body, end_at) =
        (format!("t{k}_{name}_aus"), format!("t{k}_{name}"), format!("t{k}_{name}_rumpf"), format!("t{k}_{name}_ende"));

    let armed = m.inst(&format!("load i1, ptr {armed_ptr}"));
    m.void_inst(&format!("br i1 {armed}, label %{head}, label %{skip}"));
    m.label(&head);
    let cur = m.inst(&format!("load i64, ptr {cur_ptr}"));
    let n = m.inst(&format!("call i32 @{}(i32 {sid}, i64 {cur})", crate::stream::Streams::COUNT));
    let i_ptr = m.inst("alloca i32");
    m.void_inst(&format!("store i32 0, ptr {i_ptr}"));
    let buf = crate::stream::scratch(ctx.program, elem, m)?;
    let loop_head = format!("{head}_schleife");
    m.void_inst(&format!("br label %{loop_head}"));
    m.label(&loop_head);
    let i = m.inst(&format!("load i32, ptr {i_ptr}"));
    let go_on = m.inst(&format!("icmp slt i32 {i}, {n}"));
    m.void_inst(&format!("br i1 {go_on}, label %{body}, label %{end_at}"));
    m.label(&body);
    let seq = m.inst(&format!("call i64 @{}(i32 {sid}, i64 {cur}, i32 {i}, ptr {buf})", crate::stream::Streams::AT));
    // 7.5: Der Trigger fuehrt seinen eigenen Cursor; was er gesehen hat,
    // sieht er nicht wieder.
    let next_seq = m.inst(&format!("add i64 {seq}, 1"));
    m.void_inst(&format!("store i64 {next_seq}, ptr {cur_ptr}"));

    let event = event_record(t, ctx, m)?;
    let hit = trigger_hit(pattern, *kind, buf, seq, elem, &event, ctx, m)?;
    let (fire, step_on) = (format!("{body}_treffer"), format!("{body}_weiter"));
    m.void_inst(&format!("br i1 {hit}, label %{fire}, label %{step_on}"));
    m.label(&fire);
    m.void_inst(&format!("store i1 false, ptr {armed_ptr}"));
    plan_outputs(t, &event, ctx, m)?;
    emit_fired(t, &event, ctx, m)?;
    m.void_inst(&format!("br label %{end_at}"));
    m.label(&step_on);
    let cur_i = m.inst(&format!("load i32, ptr {i_ptr}"));
    let nx = m.inst(&format!("add i32 {cur_i}, 1"));
    m.void_inst(&format!("store i32 {nx}, ptr {i_ptr}"));
    m.void_inst(&format!("br label %{loop_head}"));
    m.label(&end_at);
    m.void_inst(&format!("br label %{skip}"));
    m.label(&skip);
    Ok(())
}

/// Der Scratch fuer `event`: Captures, `.t`, `.seq`, Inhalt (7.5).
struct Event {
    slot: crate::emit::Reg,
    record: crate::ty::LlvmType,
    fields: Vec<(u32, crate::ty::LlvmType)>,
    ty: takt_mir::TypeId,
}

/// Legt den `event`-Record an: sein Typ ist der Elementtyp von `fired`.
fn event_record(t: &takt_mir::program::Trigger, ctx: &Ctx<'_>, m: &mut Module) -> Result<Event, NotYet> {
    let ty = ctx.program.streams[t.fired.index()].elem;
    let record = crate::ty::lower(ty, ctx.program).ok_or(NotYet { what: "`event`-Record" })?;
    let crate::ty::LlvmType::Struct(parts) = &record else { return Err(NotYet { what: "`event` ohne Record" }) };
    let takt_mir::types::Type::Record(r) = ctx.program.types.list.get(ty.index()).ok_or(NotYet { what: "`event`" })?
    else {
        return Err(NotYet { what: "`event` ohne Record" });
    };
    // Die Captures stehen vor `t`, `seq` und dem Inhalt (8.7).
    let fields = ctx.program.records[r.index()]
        .fields
        .iter()
        .enumerate()
        .take_while(|(_, f)| !matches!(f.name.as_str(), "t" | "seq" | "text" | "data"))
        .filter_map(|(i, _)| Some((i as u32, parts.get(i)?.clone())))
        .collect();
    let slot = m.inst(&format!("alloca {record}, align 8"));
    Ok(Event { slot, record, fields, ty })
}

/// Trifft das Muster? Die Captures gehen dabei in den `event`-Record.
#[allow(clippy::too_many_arguments)]
fn trigger_hit(
    pattern: &takt_mir::pattern::Pattern,
    kind: takt_mir::expr::MatchKind,
    buf: crate::emit::Reg,
    seq: crate::emit::Reg,
    elem: takt_mir::TypeId,
    event: &Event,
    ctx: &mut Ctx<'_>,
    m: &mut Module,
) -> Result<crate::emit::Reg, NotYet> {
    fill_event(event, buf, seq, elem, ctx, m)?;
    match pattern {
        takt_mir::pattern::Pattern::Record { record, fields } => record_hit(*record, fields, buf, elem, ctx, m),
        takt_mir::pattern::Pattern::Text { pieces, .. } => {
            let text = m.inst(&format!("getelementptr inbounds i8, ptr {buf}, i64 {}", crate::stream::Streams::LEN_AT));
            let into = Some(crate::captures::Target { slot: event.slot, record: &event.record, fields: &event.fields });
            if kind == takt_mir::expr::MatchKind::Matches {
                text_matches(pieces, text, &into, ctx, m)
            } else {
                text_has(pieces, text, &into, ctx, m)
            }
        }
    }
}

/// `.t`, `.seq` und der Inhalt des Elements in den `event`-Record (7.5).
fn fill_event(
    event: &Event,
    buf: crate::emit::Reg,
    seq: crate::emit::Reg,
    elem: takt_mir::TypeId,
    ctx: &Ctx<'_>,
    m: &mut Module,
) -> Result<(), NotYet> {
    let p = ctx.program;
    let takt_mir::types::Type::Record(r) = p.types.list.get(event.ty.index()).ok_or(NotYet { what: "`event`" })? else {
        return Err(NotYet { what: "`event` ohne Record" });
    };
    let record = &event.record;
    for (i, def) in p.records[r.index()].fields.iter().enumerate() {
        let at =
            |m: &mut Module| m.inst(&format!("getelementptr inbounds {record}, ptr {}, i32 0, i32 {i}", event.slot));
        match def.name.as_str() {
            "t" => {
                let t = m.inst(&format!("load i64, ptr {buf}"));
                let dst = at(m);
                m.void_inst(&format!("store i64 {t}, ptr {dst}"));
            }
            "seq" => {
                let dst = at(m);
                m.void_inst(&format!("store i64 {seq}, ptr {dst}"));
            }
            "text" | "data" => {
                let dst = at(m);
                crate::stream::copy_payload(buf, dst, elem, p, m)?;
            }
            _ => {}
        }
    }
    Ok(())
}

/// Die Ausgaben des `then` fuer `event.t + d` planen (7.5, 9.8).
///
/// Ein Ueberlauf von `sched` ist hier kein Fault-Zweig wie im `at` einer
/// Maschine: Der Trigger hat keinen Zustand und kein Fault-Ziel (7.5).
/// Der Rueckgabewert wird darum verworfen — die Runtime zaehlt ihn.
fn plan_outputs(
    t: &takt_mir::program::Trigger,
    event: &Event,
    ctx: &mut Ctx<'_>,
    m: &mut Module,
) -> Result<(), NotYet> {
    let at = lower_with_event(&t.time, event, ctx, m)?;
    for s in &t.then.stmts {
        let takt_mir::stmt::StmtKind::Assign { target: takt_mir::stmt::Place::Output(c), value } = &s.kind else {
            return Err(NotYet { what: "`then` mit mehr als Output-Zuweisungen" });
        };
        let v = lower_with_event(value, event, ctx, m)?;
        let word = match &v.ty {
            crate::ty::LlvmType::Int(1) => m.inst(&format!("zext i1 {} to i64", v.value)).to_string(),
            crate::ty::LlvmType::Int(64) => v.value.clone(),
            crate::ty::LlvmType::Int(n) => m.inst(&format!("sext i{n} {} to i64", v.value)).to_string(),
            _ => return Err(NotYet { what: "Trigger-Ausgabe mit zusammengesetztem Wert" }),
        };
        let _ = m.inst(&format!("call i1 @{}(i32 {}, i64 {}, i64 {word})", crate::abi::Abi::SCHEDULE, c.0, at.value));
    }
    Ok(())
}

/// Das Element von `fired` in den Ring (7.5, 8.6).
///
/// Es ist ein Record fester Groesse; `send` nimmt Zeiger und Laenge.
fn emit_fired(t: &takt_mir::program::Trigger, event: &Event, ctx: &mut Ctx<'_>, m: &mut Module) -> Result<(), NotYet> {
    let sid = crate::stream::number(takt_mir::expr::StreamRef::Internal(t.fired))
        .ok_or(NotYet { what: "Strom ohne feste Nummer" })?;
    let bytes = crate::stream::payload_cap(ctx.program, event.ty)?;
    let _ = m.inst(&format!("call i1 @{}(i32 {sid}, ptr {}, i32 {bytes})", crate::stream::Streams::SEND, event.slot));
    Ok(())
}

/// Senkt einen Ausdruck, in dem `event` vorkommen darf (7.5).
fn lower_with_event(
    e: &takt_mir::expr::Expr,
    event: &Event,
    ctx: &mut Ctx<'_>,
    m: &mut Module,
) -> Result<crate::expr::Lowered, NotYet> {
    let vars = EventVars { inner: ctx.vars(), event: event.slot, record: event.record.clone() };
    crate::expr::lower(e, ctx.program, m, &vars)
}

/// `Vars` im `then` eines Triggers: alles wie in der Maschine, dazu
/// `event` aus dem Scratch (7.5).
struct EventVars<'a> {
    inner: crate::stmt::StateVars<'a>,
    event: crate::emit::Reg,
    record: crate::ty::LlvmType,
}

impl crate::expr::Vars for EventVars<'_> {
    fn var(&self, id: takt_mir::VarId, m: &mut Module) -> Option<crate::expr::Lowered> {
        self.inner.var(id, m)
    }

    fn input(&self, c: takt_mir::ChannelId, m: &mut Module) -> Option<crate::expr::Lowered> {
        self.inner.input(c, m)
    }

    fn param(&self, id: takt_mir::ParamId, m: &mut Module) -> Option<crate::expr::Lowered> {
        self.inner.param(id, m)
    }

    fn output(&self, c: takt_mir::ChannelId, m: &mut Module) -> Option<crate::expr::Lowered> {
        self.inner.output(c, m)
    }

    fn machine_index(&self) -> Option<u32> {
        self.inner.machine_index()
    }

    fn builtin(&self, b: takt_mir::expr::Builtin, p: &Program, m: &mut Module) -> Option<crate::expr::Lowered> {
        match b {
            // `event` liegt im Scratch; `field_of` arbeitet auf Werten.
            takt_mir::expr::Builtin::Event => {
                let v = m.inst(&format!("load {}, ptr {}", self.record, self.event));
                Some(crate::expr::Lowered { value: v.to_string(), ty: self.record.clone() })
            }
            other => self.inner.builtin(other, p, m),
        }
    }
}
