//! Der Konfigurationswechsel (9.3, `switch`) im erzeugten Code.
//!
//! Jeder Test fuehrt ein kleines Programm in beiden Implementierungen und
//! verlangt dieselben Outputs (Satz 9.4.4); dazu haelt er den Wert fest,
//! den 9.3 vorgibt. Die Faelle stammen aus einem Messkern, dessen Digest
//! auf dem Board vom Interpreter abwich (FB-287): Zustandslokale
//! Variablen wurden beim Wiedereintritt nicht neu gesetzt, und dahinter
//! lagen weitere Stellen, an denen der Codegen einen Wechsel anders
//! ordnete als der Interpreter.

mod common;

use takt_interp::{RunOptions, Trace};
use takt_llvm::toolchain::Clang;

const HEAD: &str = "\
system:
    language = 1
    tick     = 1 ms

output probe : int in 0..10000 @ hw(\"probe\") with safe = 0

";

/// Beide Traces eines Programms; `None` ohne clang.
fn both(body: &str, name: &str, ticks: u64) -> Option<(String, String)> {
    let path = common::clang_path()?;
    let src = format!("{HEAD}{body}");
    let options = takt_sema::Options {
        policy: takt_diag::Policy::default(),
        build: takt_sema::Build::Sim,
        profile: None,
        ..Default::default()
    };
    let out = takt_sema::compile(&src, &options);
    assert!(!out.has_errors(), "{name}: {:?}", out.diagnostics);
    let p = out.program.expect("Programm");
    let interpreted =
        takt_interp::run(&p, &Trace::default(), &RunOptions { ticks, ..Default::default() }).expect("Lauf");
    let native = common::run_native_all(&Clang::At(path), &p, name, ticks).expect("nativ");
    Some((interpreted.trace.render(), native))
}

/// Beide Implementierungen liefern dieselben Outputs; zurueck kommt der
/// Trace des Interpreters.
fn agree(body: &str, name: &str, ticks: u64) -> Option<String> {
    let (interpreted, native) = both(body, name, ticks)?;
    let diffs = takt_conformance::compare(&interpreted, &native);
    let list: Vec<String> = diffs.iter().take(6).map(|d| format!("  {d}")).collect();
    assert!(
        diffs.is_empty(),
        "{name}:\n{}\n--- Interpreter ---\n{interpreted}\n--- nativ ---\n{native}",
        list.join("\n")
    );
    Some(interpreted)
}

/// `probe` nach Tick `t`; der Trace schreibt nur Aenderungen (9.3).
fn probe_at(trace: &str, t: u64) -> String {
    let mut value = String::new();
    for line in trace.lines() {
        let mut w = line.split_whitespace();
        let tick = w.next().and_then(|s| s.strip_prefix("t=")).and_then(|s| s.parse::<u64>().ok());
        if let (Some(tick), Some("out"), Some("probe"), Some(v)) = (tick, w.next(), w.next(), w.next())
            && tick <= t
        {
            value = v.to_string();
        }
    }
    value
}

/// Die Ticks der `log`-Zeilen; die Texte schreibt nur der Interpreter aus.
fn log_ticks(trace: &str) -> Vec<String> {
    trace
        .lines()
        .filter(|l| l.contains(" log "))
        .filter_map(|l| l.split_whitespace().next())
        .map(String::from)
        .collect()
}

/// 9.3 Schritt 3: Eine zustandslokale Variable beginnt bei jedem Eintritt
/// neu, auch beim Selbstuebergang.
#[test]
fn a_state_local_starts_fresh_on_every_entry() {
    let body = "\
machine m:
    var trips : int in 0..1000 = 0
    initial A

    state A:
        var count : int in 0..10 = 0
        loop:
            count = count + 1
            probe = count * 1000 + trips
        when count == 3:
            trips = (trips + 1) % 1000
            -> A
";
    let Some(trace) = agree(body, "switch_reinit", 12) else { return };
    assert_eq!(probe_at(&trace, 2), "1001", "`count` beginnt nach dem Wiedereintritt bei null:\n{trace}");
}

/// Geschwister teilen ihr Overlay (11.2); jedes setzt seine Variablen beim
/// Eintritt selbst, auch der Anfangszustand.
#[test]
fn siblings_sharing_the_overlay_keep_their_own_initial_values() {
    let body = "\
machine m:
    initial A

    state A:
        var a : int in 0..100 = 5
        loop:
            probe = a
        after 3 ms: -> B

    state B:
        var b : int in 0..100 = 7
        loop:
            probe = b
        after 3 ms: -> A
";
    let Some(trace) = agree(body, "switch_overlay", 12) else { return };
    assert_eq!(probe_at(&trace, 0), "5", "{trace}");
    assert_eq!(probe_at(&trace, 3), "7", "{trace}");
}

/// Der Messkern, an dem es auffiel: Die Pruefsequenz betritt ihren
/// Zustand nach jedem Fault neu, und ihr `expect` gilt wieder.
#[test]
fn the_sequence_kernel_re_arms_its_expectation() {
    let src = include_str!("../bench/sequence.takt");
    let body = src.split_once("machine seq:").map(|(_, rest)| format!("machine seq:{rest}")).expect("Maschine");
    let body = body.replace("digest", "probe");
    let Some(trace) = agree(&body, "switch_sequence", 60) else { return };
    assert!(trace.lines().filter(|l| l.contains(" fault ")).count() > 1, "nur ein Fault:\n{trace}");
}

/// 9.3: Die Aktionen eines Uebergangs laufen vor den `exit:`-Bloecken, in
/// der alten Konfiguration.
#[test]
fn transition_actions_run_before_the_exit_blocks() {
    let body = "\
machine m:
    var x : int in 0..100000 = 1
    initial A

    loop:
        probe = x

    state A:
        when x < 1000:
            x = x + 1
            -> B
        exit:
            x = (x * 2) % 100000

    state B:
        after 1 ms: -> A
";
    let Some(trace) = agree(body, "switch_order", 12) else { return };
    assert_eq!(probe_at(&trace, 2), "4", "erst `x + 1`, dann `x * 2`:\n{trace}");
}

/// 9.3 Schritt 4: Scheitert ein `enter:`, wird der Fault mit der neuen
/// Konfiguration behandelt — das Fault-Ziel des betretenen Zustands gilt.
#[test]
fn a_fault_in_an_enter_block_takes_the_fault_path_of_the_new_state() {
    let body = "\
machine m:
    fault -> SAFE_M
    var x : int in 0..5 = 0
    initial A

    state A:
        enter:
            probe = 1
        after 2 ms: -> B

    state B:
        fault -> SAFE_B
        enter:
            x = x + 3
            probe = 2
        after 1 ms: -> A

    state SAFE_B:
        enter:
            probe = 30
        after 3 ms: -> A

    state SAFE_M:
        enter:
            probe = 40
        after 3 ms: -> A
";
    let Some(trace) = agree(body, "switch_enter_fault", 14) else { return };
    assert_eq!(probe_at(&trace, 5), "30", "{trace}");
}

/// Dasselbe fuer ein `->` im Block: Auch dort gilt nach dem Wechsel das
/// Fault-Ziel des neuen Zustands.
#[test]
fn a_fault_after_a_goto_takes_the_fault_path_of_the_new_state() {
    let body = "\
machine m:
    fault -> SAFE_M
    var x : int in 0..5 = 0
    var n : int in 0..100 = 0
    initial A

    state A:
        loop:
            n = n + 1
            probe = n
            if n % 3 == 0:
                -> B

    state B:
        fault -> SAFE_B
        enter:
            x = x + 2
            probe = 50
        after 1 ms: -> A

    state SAFE_B:
        enter:
            probe = 60
        after 2 ms: -> A

    state SAFE_M:
        enter:
            probe = 70
        after 2 ms: -> A
";
    let Some(trace) = agree(body, "switch_goto_fault", 14) else { return };
    assert_eq!(probe_at(&trace, 8), "60", "{trace}");
}

/// Scheitert ein `exit:`, entfallen die restlichen Bloecke, und der Fault
/// wird ab dem kleinsten gemeinsamen Vorfahren behandelt (9.3, FB-289):
/// B ist noch nicht betreten, sein Fault-Ziel SAFE_B gilt nicht, sondern
/// das der Maschine, SAFE.
#[test]
fn a_fault_in_an_exit_block_is_handled_from_the_common_ancestor() {
    let body = "\
machine m:
    fault -> SAFE
    var x : int in 0..10 = 8
    initial A

    state A:
        enter:
            probe = 1
        after 1 ms: -> B
        exit:
            x = x + 5

    state B:
        fault -> SAFE_B
        enter:
            probe = 2

    state SAFE_B:
        enter:
            probe = 30

    state SAFE:
        enter:
            probe = 40
";
    let Some(trace) = agree(body, "switch_exit_fault", 6) else { return };
    assert_eq!(probe_at(&trace, 1), "40", "{trace}");
}

/// Auch der Weg nach `FAULTED` verlaesst die Konfiguration: Die
/// `exit:`-Bloecke laufen, nach einem Fault ebenso wie nach `-> FAULTED`
/// mit seinen Aktionen davor (5.3, 9.3).
#[test]
fn the_way_into_faulted_runs_the_exit_blocks() {
    let by_fault = "\
machine m:
    var n : int in 0..100 = 0
    initial A

    state A:
        loop:
            n = n + 1
            probe = n
            check n < 4, \"zu viel\"
        exit:
            log \"verlassen {n}\"
";
    let by_goto = "\
machine m:
    var n : int in 0..100 = 0
    initial A

    state A:
        loop:
            n = n + 1
            probe = n
        when n == 3:
            log \"aktion {n}\"
            -> FAULTED
        exit:
            log \"verlassen {n}\"
";
    for (body, name, want) in [(by_fault, "switch_faulted_exit", 1), (by_goto, "switch_goto_faulted", 2)] {
        let Some((interpreted, native)) = both(body, name, 8) else { return };
        assert!(takt_conformance::compare(&interpreted, &native).is_empty(), "{name}:\n{interpreted}\n{native}");
        assert_eq!(log_ticks(&interpreted).len(), want, "{name}:\n{interpreted}");
        assert_eq!(log_ticks(&interpreted), log_ticks(&native), "{name}:\n{interpreted}\n--- nativ ---\n{native}");
    }
}

/// `FAULTED` ist die Senke des Fault-Walds (5.3): Scheitert ein `exit:`
/// auf dem Weg hinein oder eine Aktion auf dem Weg hinaus, bleibt die
/// Maschine dort, mit sicheren Outputs — nicht beim Fault-Ziel der
/// Maschine.
#[test]
fn a_fault_while_in_faulted_stays_in_faulted() {
    let exit_on_the_way_in = "\
machine m:
    fault -> SAFE
    var x : int in 0..10 = 8
    initial A

    state A:
        enter:
            probe = 1
        after 1 ms: -> FAULTED
        exit:
            probe = 5
            x = x + 5
            probe = 6

    state SAFE:
        enter:
            probe = 40
";
    let action_on_the_way_out = "\
machine m:
    fault -> SAFE
    var x : int in 0..10 = 8
    var n : int in 0..100 = 0
    initial A

    loop:
        n = n + 1

    state A:
        loop:
            probe = n
        after 2 ms: -> FAULTED

    state SAFE:
        enter:
            probe = 40

    state FAULTED:
        when n > 2:
            x = x + 5
            -> A
";
    for (body, name) in [(exit_on_the_way_in, "switch_faulted_in"), (action_on_the_way_out, "switch_faulted_out")] {
        let Some(trace) = agree(body, name, 8) else { return };
        assert_eq!(probe_at(&trace, 7), "0", "{name}: nicht in FAULTED:\n{trace}");
        assert!(!trace.contains("out probe 40"), "{name}: zum Fault-Ziel der Maschine:\n{trace}");
    }
}

/// Aus `FAULTED` fuehren die Uebergaenge, die der Nutzer dort deklariert
/// (5.3); betreten wird wie bei jedem Wechsel, mit Entry-Tick.
#[test]
fn a_transition_out_of_faulted_enters_its_target() {
    let body = "\
machine m:
    var n : int in 0..100 = 0
    initial A

    state A:
        loop:
            n = n + 1
            probe = n
            check n % 4 != 3, \"drei\"

    state FAULTED:
        when n > 0:
            n = n + 1
            -> A
";
    let Some(trace) = agree(body, "switch_faulted_back", 12) else { return };
    assert_eq!(probe_at(&trace, 3), "5", "Aktion und Entry-Tick im selben Tick:\n{trace}");
}

/// 9.3: Der kleinste gemeinsame Vorfahr liegt echt oberhalb des
/// Zielzustands. Ein Blatt, das auf sich selbst wechselt, laesst seinen
/// Elternzustand stehen; ein Wechsel auf den Elternzustand verlaesst und
/// betritt ihn neu.
#[test]
fn the_common_ancestor_lies_strictly_above_the_target_state() {
    let to_itself = "\
machine m:
    var n : int in 0..10000 = 0
    initial P

    loop:
        probe = n

    state P:
        initial A
        enter:
            n = (n + 100) % 10000

        state A:
            enter:
                n = (n + 1) % 10000
            after 2 ms: -> A
";
    let to_the_parent = "\
machine m:
    var n : int in 0..10000 = 0
    initial P

    loop:
        probe = n

    state P:
        initial A1
        enter:
            n = (n + 100) % 10000

        state A1:
            enter:
                n = (n + 1) % 10000
            after 1 ms: -> A2

        state A2:
            enter:
                n = (n + 10) % 10000
            after 1 ms: -> P
";
    let Some(trace) = agree(to_itself, "switch_nested_self", 10) else { return };
    assert_eq!(probe_at(&trace, 3), "102", "nur `A` neu betreten:\n{trace}");
    let Some(trace) = agree(to_the_parent, "switch_to_parent", 10) else { return };
    assert_eq!(probe_at(&trace, 3), "212", "`P` neu betreten:\n{trace}");
}

/// 5.12: Auch ein `->` im Block betritt einen `resume`-Zustand ueber den
/// gespeicherten Pfad.
#[test]
fn a_goto_into_a_resume_state_takes_the_saved_path() {
    let body = "\
machine m:
    var n : int in 0..1000 = 0
    initial WORK

    loop:
        n = n + 1

    state WORK resume:
        initial FIRST
        loop:
            if n % 7 == 0:
                -> AWAY

        state FIRST:
            enter:
                probe = 1
            after 2 ms: -> SECOND

        state SECOND:
            enter:
                probe = 2
            after 20 ms: -> FIRST

    state AWAY:
        enter:
            probe = 9
        loop:
            if n % 7 == 2:
                -> WORK
";
    let Some(trace) = agree(body, "switch_goto_resume", 12) else { return };
    assert_eq!(probe_at(&trace, 6), "9", "{trace}");
    assert_eq!(probe_at(&trace, 8), "2", "zurueck nach SECOND, nicht nach FIRST:\n{trace}");
}

/// Die `exit:`-Bloecke sehen die Verweildauer des Zustands, den sie
/// verlassen (9.3), nicht die eines Ziels, das noch nicht betreten ist.
#[test]
fn exit_blocks_see_the_dwell_time_of_the_state_they_leave() {
    let body = "\
machine m:
    var d : Duration = 0 ms
    initial A

    loop:
        if d == 3 ms:
            probe = 3
        elif d == 5 ms:
            probe = 5
        else:
            probe = 1

    state A:
        after 3 ms: -> B
        exit:
            d = time_in_state

    state B:
        after 5 ms: -> A
        exit:
            d = time_in_state
";
    let Some(trace) = agree(body, "switch_exit_time", 12) else { return };
    assert_eq!(probe_at(&trace, 4), "3", "{trace}");
    assert_eq!(probe_at(&trace, 9), "5", "{trace}");
}

/// 5.11: Scheitert ein `exit:`-Block einer gescopten Instanz, entfallen
/// die restlichen; der Lauf geht weiter, die Instanz wird verworfen.
#[test]
fn a_fault_in_the_exit_of_a_scoped_instance_ends_only_its_exit() {
    let body = "\
output a : int in 0..100 @ hw(\"o/a\") with safe = 0

machine blip(o: output int) every 1 ms:
    var x : int in 0..10 = 8
    initial LOW

    state LOW:
        enter:
            o = 7

        after 30 ms: -> LOW

        exit:
            log \"vor\"
            x = x + 5
            log \"nach\"

machine ctrl:
    var n : int in 0..100 = 0
    initial FIRST

    loop:
        n = n + 1
        probe = n

    state FIRST:
        instance p = blip(o = a)

        after 4 ms: -> SECOND

    state SECOND:
        after 3 ms: -> FIRST
";
    let Some((interpreted, native)) = both(body, "switch_scoped_exit", 16) else { return };
    assert!(takt_conformance::compare(&interpreted, &native).is_empty(), "{interpreted}\n{native}");
    assert!(!interpreted.contains("\"nach\""), "der Rest des `exit:` lief:\n{interpreted}");
    assert_eq!(log_ticks(&interpreted), ["t=4", "t=11"], "{interpreted}");
    assert_eq!(log_ticks(&interpreted), log_ticks(&native), "{interpreted}\n--- nativ ---\n{native}");
}

#[test]
fn a_check_with_its_own_target_goes_there_from_a_leaf() {
    // 5.3: `check e -> X` ueberschreibt das Fault-Ziel fuer genau diesen
    // Check; der Codegen nahm immer das des Zustands (FB-412).
    let Some(trace) = agree(
        "
machine m:
    fault -> SAFE
    var k : int in 0..100 = 0
    initial RUN
    state RUN:
        loop:
            k = k + 1
            probe = k
            check k < 3, \"zu gross\" -> OTHER
    state SAFE:
        enter:
            probe = 900
        after 1 s: -> RUN
    state OTHER:
        enter:
            probe = 500
        after 1 s: -> RUN
",
        "check_target_leaf",
        4,
    ) else {
        return;
    };
    assert_eq!(probe_at(&trace, 2), "500", "nach OTHER, nicht nach SAFE:\n{trace}");
}

#[test]
fn a_check_with_its_own_target_goes_there_from_a_shared_level() {
    // Der Check steht im `loop:` des Elternzustands, den zwei Blaetter
    // teilen: Der Codegen kennt das Blatt erst zur Laufzeit.
    let Some(trace) = agree(
        "
command flip
machine m:
    fault -> SAFE
    var k : int in 0..100 = 0
    initial P
    state P:
        initial A
        loop:
            k = k + 1
            check k < 3, \"zu gross\" -> OTHER
        state A:
            loop:
                probe = k
            when flip: -> B
        state B:
            loop:
                probe = k + 10
            when flip: -> A
    state SAFE:
        enter:
            probe = 900
        after 1 s: -> P
    state OTHER:
        enter:
            probe = 500
        after 1 s: -> P
",
        "check_target_shared",
        4,
    ) else {
        return;
    };
    assert_eq!(probe_at(&trace, 2), "500", "nach OTHER, nicht nach SAFE:\n{trace}");
}

#[test]
fn a_chain_of_check_targets_runs_to_its_end() {
    // Lemma 9.3.1: Die Schranke von `resolve_m` ist der laengste Pfad im
    // Fault-Wald samt `check ... -> X`; mit den Fault-Zielen allein war sie
    // 2, und der Interpreter brach die Kette ab (FB-419).
    let Some(trace) = agree(
        "
command ok
machine m:
    initial A
    state A:
        loop:
            probe = 1
            check ok, \"a\" -> B
    state B:
        loop:
            probe = 2
            check ok, \"b\" -> C
    state C:
        loop:
            probe = 3
            check ok, \"c\" -> D
    state D:
        loop:
            probe = 4
            check ok, \"d\" -> E
    state E:
        loop:
            probe = 5
",
        "check_chain",
        2,
    ) else {
        return;
    };
    assert_eq!(probe_at(&trace, 0), "5", "die Kette endet in E:\n{trace}");
}

#[test]
fn a_child_of_the_machine_fault_target_inherits_faulted() {
    // 5.3: φ(s) ist explizit, sonst φ des Elternzustands. Der Fault-Ziel-
    // zustand der Maschine hat φ = FAULTED, sein Kind erbt das. Vorher
    // zeigte das Kind zurueck auf SAFE, und ein Fault dort kreiste.
    let Some(trace) = agree(
        "
command ok
machine m:
    fault -> SAFE
    initial RUN
    state RUN:
        loop:
            probe = 1
            check ok, \"run\"
    state SAFE:
        initial HOLD
        state HOLD:
            loop:
                probe = 2
                check ok, \"hold\"
",
        "phi_child_of_safe",
        2,
    ) else {
        return;
    };
    assert!(trace.contains("t=0 state m FAULTED"), "HOLD faultet nach FAULTED:\n{trace}");
}

/// **Ein Fault im Guard nimmt den Fault-Pfad** (9.3, 5.3; KON2-031): Der
/// `when`-Ausdruck teilt durch null; statt des Uebergangs kommt das
/// Fault-Ziel, in beiden Implementierungen.
#[test]
fn a_fault_in_a_guard_takes_the_fault_path() {
    let body = "\
machine m:
    fault -> SAFE
    var k : int in 0..9 = 0
    initial RUN

    state RUN:
        loop:
            probe = 1
        when 100 / k > 1: -> NEXT

    state NEXT:
        enter:
            probe = 2

    state SAFE:
        enter:
            probe = 9
";
    let Some(trace) = agree(body, "switch_guard_fault", 3) else { return };
    assert!(trace.contains("fault m Arithmetic(DivZero)"), "{trace}");
    assert_eq!(probe_at(&trace, 1), "9", "der Guard faultet in Tick 1, dem ersten mit Uebergaengen:\n{trace}");
}

/// Ein Elternzustand mit einem Guard ueber eine Variable, die von Anfang an
/// wahr ist, und ein Kind mit zwei immer wahren Guards.
const GUARDS: &str = "\
machine m:
    var armed : bool = true
    initial P

    state P:
        initial C
        when armed: -> OUTER

        state C:
            enter:
                probe = 1
            when true: -> FIRST
            when true: -> SECOND

    state OUTER:
        enter:
            probe = 10

    state FIRST:
        enter:
            probe = 20

    state SECOND:
        enter:
            probe = 30
";

/// **Mehrere wahre Guards: aussen zuerst** (9.3 Schritt 2; KON2-031): Der
/// Elternzustand und sein Kind haben je einen wahren Guard; es gewinnt der
/// des Elternzustands.
#[test]
fn the_outer_guard_wins() {
    let Some(trace) = agree(GUARDS, "switch_outer_first", 3) else { return };
    assert_eq!(probe_at(&trace, 1), "10", "der Guard des Elternzustands gewinnt:\n{trace}");
}

/// **Mehrere wahre Guards eines Zustands: der erste der Quelle** (9.3 Schritt
/// 2; KON2-031). Ist der Guard des Elternzustands falsch, gewinnt von den
/// beiden wahren Guards des Kindes der erste.
#[test]
fn the_first_true_guard_in_source_wins() {
    let inner = GUARDS.replace("when armed: -> OUTER", "when not armed: -> OUTER");
    let Some(trace) = agree(&inner, "switch_source_order", 3) else { return };
    assert_eq!(probe_at(&trace, 1), "20", "der erste Guard des Kindes gewinnt:\n{trace}");
}

/// **Ein Wechsel zwischen Vettern verschiedener Teilbaeume verlaesst innen
/// nach aussen und betritt aussen nach innen, ueber je zwei Ebenen** (9.3,
/// Lemma 9.3.1; KON2-031). Jeder Block haengt seine Ziffer an `trail`; die
/// Folge der Ziffern ist die Reihenfolge der Bloecke.
#[test]
fn a_switch_between_cousins_exits_inside_out_and_enters_outside_in() {
    let body = "\
output trail : int in 0..99999999 @ hw(\"trail\") with safe = 0

machine m:
    var t : int in 0..9999999 = 0
    initial A

    state A:
        initial A1
        exit:
            t = t * 10 + 3
            trail = t

        state A1:
            initial A11
            exit:
                t = t * 10 + 2
                trail = t

            state A11:
                when true: -> B11
                exit:
                    t = t * 10 + 1
                    trail = t

    state B:
        initial B1
        enter:
            t = t * 10 + 4
            trail = t

        state B1:
            initial B11
            enter:
                t = t * 10 + 5
                trail = t

            state B11:
                enter:
                    t = t * 10 + 6
                    trail = t
                    probe = 1
";
    let Some(trace) = agree(body, "switch_cousins", 3) else { return };
    let last = trace.lines().filter_map(|l| l.strip_prefix("t=1 out trail ")).next_back().map(str::to_string);
    assert_eq!(last.as_deref(), Some("123456"), "exit 1, 2, 3, dann enter 4, 5, 6:\n{trace}");
}

/// **Der Eintritt setzt `every` und die Dauer eines `check … for` zurueck**
/// (5.8, 9.3; KON2-031). Ein Zustand, der jede Millisekunde wieder betreten
/// wird, zaehlt mit `every 3 ms` nie, und ein `check` mit `for 3 ms` faultet
/// nie, obwohl seine Bedingung immer falsch ist; ohne Wiedereintritt zaehlt
/// und faultet beides.
#[test]
fn an_entry_resets_every_and_the_duration_of_a_check() {
    let body = "\
output fired : int in 0..1000 @ hw(\"fired\") with safe = 0

machine m:
    fault -> HALT
    var n : int in 0..1000 = 0
    initial R

    state R:
        loop:
            every 3 ms:
                n = n + 1
                fired = n
            check probe > 5, \"never\" for 3 ms
        after 1 ms: -> R

    state HALT:
        enter:
            probe = 9
";
    let Some(trace) = agree(body, "switch_reset_every", 12) else { return };
    assert!(!trace.contains(" fault "), "der Wiedereintritt setzt die Dauer zurueck:\n{trace}");
    let fired = trace.lines().filter_map(|l| l.split_once(" out fired ")).map(|(_, v)| v.to_string()).next_back();
    assert_eq!(fired.as_deref(), Some("0"), "`every` beginnt mit jedem Eintritt neu:\n{trace}");
    let steady = body.replace("        after 1 ms: -> R\n", "        after 100 ms: -> R\n");
    let Some(trace) = agree(&steady, "switch_no_reset_every", 12) else { return };
    assert!(trace.contains("fault m CheckFailed"), "ohne Wiedereintritt laeuft die Dauer ab:\n{trace}");
}
