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
        let Target::State(to) = t.target else { return Err(NotYet { what: "Uebergangsziel" }) };
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
    let state_ty = format!("%{}_state", m.name);
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
    // Die Frist darf ein Literal oder ein Parameter sein: Beide stehen
    // fuer den Lauf fest, und nur dann kann die Schedulability (7.2) sie
    // beschraenken. Ein Ausdruck ueber Variablen haette einen Wert je
    // Tick — `after` waere dann keine Frist mehr, sondern eine Bedingung.
    let vars = ctx.vars();
    let frist = match d.kind {
        takt_mir::expr::ExprKind::Duration(ns) => {
            crate::expr::Lowered { value: ns.to_string(), ty: crate::ty::LlvmType::Int(64) }
        }
        takt_mir::expr::ExprKind::Param(_) => crate::expr::lower(d, ctx.program, m, &vars)?,
        _ => return Err(NotYet { what: "`after` mit berechneter Dauer" }),
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
        let state_ty = format!("%{}_state", ctx.machine.name);
        let cur_ptr = m.inst(&format!("getelementptr inbounds {state_ty}, ptr %0, i32 0, i32 {cursor}"));
        let cur = m.inst(&format!("load i64, ptr {cur_ptr}"));
        let n = m.inst(&format!("call i32 @{}(i32 {sid}, i64 {cur})", crate::stream::Streams::COUNT));
        // Der Zaehler laeuft ueber das Fenster; seine Schranke ist `n`.
        let i_ptr = m.inst("alloca i32");
        m.void_inst(&format!("store i32 0, ptr {i_ptr}"));
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
        // 8.7: der erste passende Handler gewinnt. Solange nur
        // Catch-alls gesenkt werden, ist das immer der erste — weitere
        // kaemen nie zum Zug, und sie zu erzeugen waere toter Code.
        // Sobald Muster dazukommen, wird daraus eine Kette von Zweigen.
        let Some(first) = hs.first() else { continue };
        if hs.iter().any(|h| h.pattern.is_some()) {
            return Err(NotYet { what: "Handler mit Muster" });
        }
        block(&first.body.clone(), ctx, m)?;
        let cur_i = m.inst(&format!("load i32, ptr {i_ptr}"));
        let next = m.inst(&format!("add i32 {cur_i}, 1"));
        m.void_inst(&format!("store i32 {next}, ptr {i_ptr}"));
        m.void_inst(&format!("br label %{kopf}"));
        m.label(&ende);
        // Der Cursor steht danach hinter dem letzten untersuchten
        // Element (9.6); die Runtime fuehrt ihn mit `examined` nach.
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
        let state_ty = format!("%{}_state", machine_def.name);
        let field = m.inst(&format!("getelementptr inbounds {state_ty}, ptr %0, i32 0, i32 {pending}"));
        let flag = m.inst(&format!("getelementptr inbounds {{ i1, i32, i32 }}, ptr {field}, i32 0, i32 0"));
        m.void_inst(&format!("store i1 true, ptr {flag}"));
    }
    let target = machine_def.fault_target_of(from);
    let takt_mir::machine::FaultTarget::State(to) = target else {
        // `FAULTED` fuehrt keinen Nutzercode aus (5.2 Regel 5); die
        // Outputs stehen bereits auf `safe`, was die Runtime stellt.
        m.void_inst("ret void");
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
