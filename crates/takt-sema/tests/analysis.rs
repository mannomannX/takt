//! Das statische Gate (Referenz 3.4, 9.4.3, 11.5; plan/m3.md Abschnitt 6).
//!
//! Geprueft wird an der MIR, nicht am Trace: Eine bewiesene Zuweisung laesst
//! keine Pruefung uebrig, eine unbewiesene schon, und gewarnt wird nur, wo
//! 3.4 es verlangt. Lemma 3.4 prueft `theorems.rs`.

use takt_diag::Policy;
use takt_mir::Program;
use takt_mir::analysis::Report;
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 1 ms\n\n";
const OUT: &str = "output n : int in 0..99 @ hw(\"o/n\") with safe = 0\n\n";

/// Uebersetzt und liefert Programm samt Kennzahlen.
fn compile(body: &str) -> (Program, Report, Vec<String>) {
    let src = format!("{HEAD}{OUT}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "unerwartete Fehler:\n{}", errors.join("\n"));
    let warnings: Vec<String> = out.diagnostics.iter().filter(|d| !d.is_error()).map(|d| format!("{d}")).collect();
    (out.program.expect("Programm"), out.report, warnings)
}

/// Zahl der impliziten Pruefungen einer Ursache.
fn count(r: &Report, cause: &str) -> u32 {
    r.checks.get(cause).copied().unwrap_or(0)
}

#[test]
fn a_provable_assignment_needs_no_check() {
    // 3.4: „Ist das Intervall des Ausdrucks enthalten → keine Pruefung."
    let (_, r, _) = compile(
        "\
machine m:
    initial RUN
    state RUN:
        loop:
            var a : int in 0..9 = 5
            var b : int in 0..99 = a + 1
            n = b
",
    );
    assert_eq!(count(&r, "Declared"), 0, "0..9 plus 1 liegt in 0..99: {:?}", r.checks);
}

#[test]
fn an_unprovable_assignment_keeps_its_check() {
    let (_, r, _) = compile(
        "\
machine m:
    var big : int in 0..999 = 500
    initial RUN
    state RUN:
        loop:
            n = big
",
    );
    assert_eq!(count(&r, "Declared"), 1, "0..999 passt nicht in 0..99: {:?}", r.checks);
}

#[test]
fn a_comparison_refines_the_branch() {
    // 3.4: „Vergleiche verfeinern Intervalle in Zweigen."
    let (_, r, _) = compile(
        "\
machine m:
    var big : int in 0..999 = 5
    initial RUN
    state RUN:
        loop:
            if big < 100:
                n = big
",
    );
    assert_eq!(count(&r, "Declared"), 0, "im Zweig ist `big` unter 100: {:?}", r.checks);
}

#[test]
fn a_passed_check_refines_what_follows() {
    // 5.6: ein bestandener `check` gilt fuer das Folgende.
    let (_, r, _) = compile(
        "\
machine m:
    var big : int in 0..999 = 5
    initial RUN
    state RUN:
        loop:
            check big < 100
            n = big
",
    );
    assert_eq!(count(&r, "Declared"), 0, "nach dem `check` ist `big` unter 100: {:?}", r.checks);
}

#[test]
fn only_a_check_in_a_loop_warns() {
    // 3.4, Warnpolitik: „Warnungen im engeren Sinn entstehen nur bei
    // impliziten Pruefungen in `for`-Schleifen und in Aktionsbloecken."
    let (_, outside, w_outside) = compile(
        "\
machine m:
    var big : int in 0..999 = 500
    initial RUN
    state RUN:
        loop:
            n = big
",
    );
    assert_eq!(outside.warned, 0, "ausserhalb einer Schleife wird nicht gewarnt");
    assert!(!w_outside.iter().any(|w| w.contains("SC-24")), "{w_outside:?}");

    let (_, inside, w_inside) = compile(
        "\
machine m:
    var big : int in 0..999 = 500
    initial RUN
    state RUN:
        loop:
            for i in range(3):
                n = big
",
    );
    assert_eq!(inside.warned, 1, "in der Schleife wird gewarnt");
    assert!(w_inside.iter().any(|w| w.contains("SC-24")), "{w_inside:?}");
}

#[test]
fn the_metric_names_the_cause() {
    // 3.4: die Kennzahl ist nach Ursache aufgeschluesselt (FB-19).
    let (_, r, _) = compile(
        "\
machine m:
    var big : int in 0..999 = 500
    initial RUN
    state RUN:
        loop:
            n = big
",
    );
    for cause in ["Declared", "Index", "Convert", "Arith"] {
        assert!(r.checks.contains_key(cause), "`{cause}` fehlt in der Kennzahl: {:?}", r.checks);
    }
    let text = r.lines().join("\n");
    assert!(text.contains("Declared 1"), "die Ursache steht im Report: {text}");
}

#[test]
fn a_proven_expression_is_narrowed_to_i32() {
    // 3.4, Lemma 3.4: „32 Bit, wo moeglich".
    let (_, r, _) = compile(
        "\
machine m:
    initial RUN
    state RUN:
        loop:
            var a : int in 0..9 = 5
            n = a
",
    );
    assert!(r.narrowed > 0, "etwas wurde verengt: {} von {}", r.narrowed, r.integer_exprs);
    assert_eq!(r.narrowed, r.integer_exprs, "hier ist alles beweisbar schmal");
}

#[test]
fn every_machine_gets_a_cost_budget() {
    // 9.4.3: `B_m` und `F_m` als Vektoren ueber sieben Klassen.
    let (p, _, _) = compile(
        "\
machine m:
    initial RUN
    state RUN:
        loop:
            var a : int in 0..9 = 5
            n = a + 1
",
    );
    let m = &p.machines[0];
    let b = m.budget.expect("Budget aus M3");
    assert!(b.activation.i32 > 0 || b.activation.i64 > 0, "die Addition zaehlt: {:?}", b.activation);
}

#[test]
fn the_memory_report_separates_reliable_from_open() {
    // 11.5: jeder Posten traegt seine Herkunft, summiert wird nur
    // Belastbares.
    let (p, _, _) = compile(
        "\
machine m:
    var x : int in 0..99 = 0
    initial RUN
    state RUN:
        loop:
            n = x
",
    );
    let s = takt_mir::analysis::size::size(&p);
    assert!(s.total() > 0, "der Maschinenzustand zaehlt");
    assert!(s.has_open(), "Profilreserven fehlen noch (8.10, 13.8)");
    let text = s.lines().join("\n");
    assert!(text.contains("exakt") && text.contains("offen"), "beide Herkuenfte stehen da: {text}");
}

#[test]
fn the_cost_budget_separates_activation_from_fault_path() {
    // Pruefung 12 (9.4.3): `B_m` je Aktivierung und `F_m` je Fault-Pfad sind
    // getrennte Vektoren — die Abort-Phase (5.4) summiert nur die zweiten.
    let (p, _, _) = compile(
        "\
machine m:
    fault -> SAFE
    var a : int in 0..9 = 1
    initial RUN
    state RUN:
        loop:
            a = a
            check a < 9, \"zu gross\"
            n = a
    state SAFE:
        loop:
            n = 0
",
    );
    let b = p.machines[0].budget.expect("Budget aus M3");
    assert!(b.activation.i32 + b.activation.i64 > 0, "die Aktivierung kostet: {:?}", b.activation);
    // Ohne Uebergaenge aus `FAULTED` ist der Fault-Pfad leer; entscheidend
    // ist, dass er *getrennt* gefuehrt wird.
    assert_eq!(b.fault_path.call, 0, "der Fault-Pfad ist eigenstaendig: {:?}", b.fault_path);
}

#[test]
fn a_loop_multiplies_the_budget() {
    // 9.4.3: die statische Schranke einer `for`-Schleife geht in das Budget
    // ein — sonst waere es keine obere Schranke.
    let (one, _, _) = compile(
        "\
machine m:
    var a : int in 0..9 = 1
    initial RUN
    state RUN:
        loop:
            a = a + 1
            n = a
",
    );
    let (many, _, _) = compile(
        "\
machine m:
    var a : int in 0..9 = 1
    initial RUN
    state RUN:
        loop:
            for i in range(4):
                a = a + 1
            n = a
",
    );
    let (b1, b4) = (one.machines[0].budget.expect("B"), many.machines[0].budget.expect("B"));
    let sum = |c: takt_mir::fns::CostVec| c.i32 + c.i64;
    assert!(sum(b4.activation) > sum(b1.activation), "vier Durchlaeufe kosten mehr: {:?} vs {:?}", b4, b1);
}

#[test]
fn a_sequence_var_always_carries_an_initialiser() {
    // Pruefung 25 (Definite Assignment je Eintritt) hat heute keinen eigenen
    // Fall: Die Grammatik verlangt an jedem `var` ein `=` (2.3, `var_decl`),
    // also ist eine gehobene Variable nie uninitialisiert. Faellt diese
    // Schranke, muss 25 einen Korpus bekommen — dann schlaegt dieser Test
    // fehl und erinnert daran.
    let src = format!(
        "{HEAD}{OUT}{}",
        "machine m:
    initial RUN
    state RUN:
        sequence:
            var k : int in 0..9
            step \"setzen\":
                k = 3
            step \"lesen\":
                n = k
            -> RUN
"
    );
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    let codes: Vec<&str> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| d.code).collect();
    assert!(codes.contains(&"P"), "ein `var` ohne Initialisierer ist ein Syntaxfehler, kein SC-25: {codes:?}");
}

#[test]
fn an_error_inside_a_format_string_points_at_the_placeholder() {
    // FB-31: Die Spans eines Platzhalter-Ausdrucks zaehlen ab dem Platzhalter,
    // nicht ab dem Dateianfang. Ohne Verschiebung meldete jeder Namensfehler
    // in `log "… {x}"` die Stelle 1:1 — in einer langen Datei unbrauchbar.
    let src = format!(
        "{HEAD}{OUT}{}",
        "machine m:
    var zaehler : int in 0..99 = 0
    initial RUN
    state RUN:
        enter:
            log \"start {zaehlr}\"
        loop:
            n = zaehler
"
    );
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    let d = out.diagnostics.iter().find(|d| d.code == "SC-2").expect("der Tippfehler wird gemeldet");
    let at = src[d.span.start as usize..d.span.end as usize].to_string();
    assert_eq!(at, "zaehlr", "der Span zeigt genau auf den Namen im Platzhalter");
}

#[test]
fn append_costs_the_capacity_of_its_source() {
    // 9.4.3: `append` kopiert, die obere Schranke ist die Kapazitaet der
    // Quelle — nicht ihre aktuelle Laenge, die statisch niemand kennt.
    let small = compile(
        "\
machine m:
    var a : bytes<4> = default
    var b : bytes<512> = default
    initial RUN
    state RUN:
        loop:
            b.append(a)
            n = 0
",
    )
    .0;
    let large = compile(
        "\
machine m:
    var a : bytes<256> = default
    var b : bytes<512> = default
    initial RUN
    state RUN:
        loop:
            b.append(a)
            n = 0
",
    )
    .0;
    let (s, l) = (small.machines[0].budget.expect("B"), large.machines[0].budget.expect("B"));
    assert!(
        l.activation.mem > s.activation.mem,
        "die groessere Quelle kostet mehr: {} vs {}",
        l.activation.mem,
        s.activation.mem
    );
    assert_eq!(l.activation.mem - s.activation.mem, 252, "genau die Differenz der Kapazitaeten");
}

#[test]
fn size_lists_the_dfa_tables_as_open_until_codegen() {
    // 11.5 zaehlt die DFA-Tabellen der Muster zu den Posten. Die Rechnung
    // steht, aber gefuellt werden die Tabellen erst vom Codegen — der
    // Interpreter gleicht direkt ab (plan/m2.md 1.1). Der Posten steht
    // darum auf `offen`: eine Null, die noch niemand gerechnet hat, ist
    // kein Messwert. Schlaegt der Test fehl, weil er nun `exakt` ist, hat
    // der Codegen die Tabellen gebaut — dann ist das die richtige Meldung.
    let (p, _, _) = compile(
        "\
machine m:
    initial RUN
    state RUN:
        loop:
            n = 0
",
    );
    let text = takt_mir::analysis::size::size(&p).lines().join("\n");
    let line = text.lines().find(|l| l.contains("DFA")).expect("der Posten steht in der Liste");
    assert!(line.contains("offen"), "ohne Codegen ist er offen: {line}");
}

#[test]
fn the_alert_polarity_lint_only_fires_on_the_same_comparison() {
    // Pruefung 63a (5.6): Ein `alert` nennt das zu meldende Ereignis, ein
    // `check` die einzuhaltende Invariante. Dieselbe Vergleichsrichtung in
    // beiden ist der beweisbare Fehlerfall; die entgegengesetzte ist der
    // dokumentierte Normalfall und muss schweigen.
    let same = warnings_of(
        "\
machine m:
    initial RUN
    state RUN:
        loop:
            check n < 50, \"zu gross\"
            alert n < 50, \"waechst\"
            n = 0
",
    );
    assert!(same.iter().any(|w| w.contains("SC-63")), "gleiche Richtung warnt: {same:?}");

    let opposite = warnings_of(
        "\
machine m:
    initial RUN
    state RUN:
        loop:
            check n < 50, \"zu gross\"
            alert n > 50, \"waechst\"
            n = 0
",
    );
    assert!(!opposite.iter().any(|w| w.contains("SC-63")), "die richtige Polaritaet schweigt: {opposite:?}");
}

/// Die Warnungen eines Maschinenrumpfs als Text.
fn warnings_of(body: &str) -> Vec<String> {
    let src = format!("{HEAD}{OUT}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    takt_sema::compile(&src, &options).diagnostics.iter().map(|d| format!("{d}")).collect()
}
