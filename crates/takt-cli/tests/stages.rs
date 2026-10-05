//! Die sechs Stufen der Kommandozeile laufen und melden, was sie sollen.
//!
//! Geprueft ist bisher jede Schicht fuer sich (Korpus, Golden-Traces,
//! Roundtrip); der Aufsatz darueber — Argumente, Exit-Code, Ausgabe — hatte
//! keinen Test. Dieser Rahmen deckt ihn ab: Jede Stufe laeuft gegen eine
//! Korpusdatei, und die beiden Faelle, in denen ein Exit-Code etwas
//! bedeutet (Fehler, nicht kanonisch), werden einzeln festgehalten.

use std::path::PathBuf;
use std::process::{Command, Output};

fn root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

fn takt(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_takt")).current_dir(root()).args(args).output().expect("takt startet")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// Jede Stufe nimmt eine gueltige Korpusdatei an und liefert Exit-Code 0.
#[test]
fn every_stage_accepts_a_corpus_file() {
    let file = "corpus-try/13_framing.takt";
    for stage in ["tokens", "parse", "fmt", "check", "mir", "size"] {
        let out = takt(&[stage, file]);
        assert!(out.status.success(), "`takt {stage}` scheitert an {file}:\n{}", String::from_utf8_lossy(&out.stderr));
        // `check` und `fmt` schweigen, wenn nichts zu melden ist; die
        // uebrigen vier geben ihr Zwischenergebnis aus.
        if !matches!(stage, "check" | "fmt") {
            assert!(!stdout(&out).is_empty(), "`takt {stage}` sagt nichts");
        }
    }
}

/// `check` meldet Fehler mit Exit-Code 1 — der Unterschied, an dem ein
/// Build-Skript haengt.
#[test]
fn check_fails_on_a_broken_file() {
    let out = takt(&["check", "corpus-try/n04_missing_initial.takt"]);
    assert!(!out.status.success(), "eine kaputte Datei muss scheitern");
    let text = stdout(&out) + &String::from_utf8_lossy(&out.stderr);
    assert!(text.contains("initial"), "die Meldung nennt die Ursache: {text}");
}

/// `fmt --check` unterscheidet kanonisch von nicht kanonisch, ohne die
/// Datei anzufassen.
#[test]
fn fmt_check_reports_canonical_form() {
    let out = takt(&["fmt", "--check", "corpus-try/13_framing.takt"]);
    assert!(out.status.success(), "der Korpus ist kanonisch: {}", stdout(&out));
}

/// `size` gibt die Posten aus, die 11.5 nennt, samt Herkunft.
#[test]
fn size_lists_its_items_with_origin() {
    let out = takt(&["size", "corpus-try/01_minimal.takt"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = stdout(&out);
    for item in ["Maschinenzustaende", "Stroeme", "Summe"] {
        assert!(text.contains(item), "Posten `{item}` fehlt:\n{text}");
    }
    assert!(text.contains("exakt") || text.contains("offen"), "die Herkunft steht dabei:\n{text}");
}

/// `size --baseline` (11.5, D1): eine gespeicherte Rechnung ist die
/// Messlatte; waechst RAM oder Flash, faellt der Aufruf.
#[test]
fn size_compares_with_a_baseline() {
    let file =
        std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("takt-size-{}.baseline", std::process::id()));
    let path = file.to_string_lossy().into_owned();
    let saved = takt(&["size", "corpus-try/01_minimal.takt", "--save-baseline", &path]);
    assert!(saved.status.success(), "{}", String::from_utf8_lossy(&saved.stderr));
    let same = takt(&["size", "corpus-try/01_minimal.takt", "--baseline", &path]);
    assert!(same.status.success() && stdout(&same).contains("keine Aenderung"), "{}", stdout(&same));
    let bigger = takt(&["size", "corpus-try/13_framing.takt", "--baseline", &path]);
    assert!(!bigger.status.success(), "ein groesseres Programm faellt gegen die Baseline:\n{}", stdout(&bigger));
    assert!(stdout(&bigger).contains("gewachsen"), "{}", stdout(&bigger));
    let _ = std::fs::remove_file(&file);
}

/// `mir --hash` liefert den Logik-Hash; er haengt nur an der Logik, also
/// liefert derselbe Aufruf denselben Wert.
#[test]
fn mir_hash_is_stable() {
    let a = takt(&["mir", "corpus-try/01_minimal.takt", "--hash"]);
    let b = takt(&["mir", "corpus-try/01_minimal.takt", "--hash"]);
    assert!(a.status.success(), "{}", String::from_utf8_lossy(&a.stderr));
    assert_eq!(stdout(&a), stdout(&b), "derselbe Lauf, derselbe Hash");
    assert!(!stdout(&a).trim().is_empty(), "der Hash steht da");
}

/// `sim` laeuft eine feste Zahl Ticks und schreibt einen Trace.
#[test]
fn sim_runs_for_the_requested_ticks() {
    let out = takt(&["sim", "corpus-try/13_framing.takt", "--ticks", "4"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = stdout(&out);
    assert!(text.contains("t=0"), "der Trace beginnt bei t=0:\n{text}");
    assert!(text.contains("state builder"), "der Zustand steht im Trace:\n{text}");
}

/// `latency` gibt je Output die Schranke aus, mit Aufschluesselung und dem
/// Vorbehalt der Zeitspalte (9.4.5).
#[test]
fn latency_reports_ticks_and_its_caveat() {
    let out = takt(&["latency", "corpus-try/14_latency.takt"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = stdout(&out);
    assert!(text.contains("Ticks"), "die Tickspalte steht da:\n{text}");
    assert!(text.contains("erkennen"), "die Aufschluesselung steht dabei:\n{text}");
    assert!(text.contains("Schedulability"), "der Vorbehalt der Zeitspalte steht dabei:\n{text}");
}

/// Eine Kopie im Testverzeichnis, je Test und Prozess eindeutig benannt.
fn scratch(name: &str, text: &str) -> PathBuf {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("takt-{}-{name}", std::process::id()));
    std::fs::write(&path, text).expect("schreibbar");
    path
}

/// `fmt --check` auf eine nicht kanonische Datei: Exit ungleich null, die
/// Meldung, und die Datei bleibt Byte fuer Byte, wie sie war.
#[test]
fn fmt_check_refuses_a_non_canonical_file_and_leaves_it_alone() {
    let text = "const  A=1\n";
    let path = scratch("crooked.takt", text);
    let out = takt(&["fmt", "--check", path.to_str().expect("Pfad")]);
    assert!(!out.status.success(), "nicht kanonisch muss scheitern");
    assert!(stdout(&out).contains("nicht kanonisch"), "{}", stdout(&out));
    assert_eq!(std::fs::read_to_string(&path).expect("lesbar"), text);
    let _ = std::fs::remove_file(&path);
}

/// Eine Datei mit Fehlern bleibt unveraendert (format.md, Einleitung), und
/// der Fehler zeigt auf die Stelle in der Datei, nicht im reparierten Text.
#[test]
fn fmt_leaves_a_broken_file_alone_and_points_into_it() {
    let text = "machine m:\n\tinitial = A\n";
    let path = scratch("broken.takt", text);
    let file = path.to_str().expect("Pfad");
    let out = takt(&["fmt", file]);
    assert!(!out.status.success(), "ein Fehler muss scheitern");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains(&format!("{file}:2:10: error[P]")), "Stelle hinter dem Tabulator: {err}");
    assert_eq!(std::fs::read(&path).expect("lesbar"), text.as_bytes(), "Byte fuer Byte unveraendert");
    let _ = std::fs::remove_file(&path);
}

/// `fmt --edition` traegt `language` genau einmal ein (2.5), auch hinter einer
/// Deklaration vor `system:`; danach ist die Datei kanonisch.
#[test]
fn fmt_edition_inserts_the_language_once() {
    let path = scratch("edition.takt", "const A = 1\nsystem:\n    tick = 1 ms\n");
    let file = path.to_str().expect("Pfad");
    let out = takt(&["fmt", "--edition", file]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = std::fs::read_to_string(&path).expect("lesbar");
    assert_eq!(text, "const A = 1\nsystem:\n    language = 1\n    tick     = 1 ms\n");
    let again = takt(&["fmt", "--edition", "--check", file]);
    assert!(again.status.success(), "ein zweiter Lauf aendert nichts: {}", stdout(&again));
    let _ = std::fs::remove_file(&path);
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// 11.1: Ein unbekannter Unterbefehl, eine fehlende Datei, ein unbekannter
/// Schalter und ein Schalter ohne Wert enden mit Exit ungleich null und
/// einer Meldung - kein Tippfehler laeuft still mit dem Vorgabewert.
#[test]
fn bad_arguments_are_refused_with_a_message() {
    let cases: [(&[&str], &str); 5] = [
        (&["nope"], "takt check|build"),
        (&["check", "gibtsnicht.takt"], "gibtsnicht.takt"),
        (&["sim", "corpus-try/13_framing.takt", "--ticks", "4", "--tikcs", "4"], "unbekannter Schalter `--tikcs`"),
        (&["test", "corpus-try/38_scenarios.takt", "--senario", "x"], "unbekannter Schalter `--senario`"),
        (
            &["driver-test", "--crate", "crates/takt-driver-probe", "corpus-try/13_framing.takt", "--ticks"],
            "`--ticks` verlangt einen Wert",
        ),
    ];
    for (args, message) in cases {
        let out = takt(args);
        assert!(!out.status.success(), "{args:?} bestand");
        assert!(stderr(&out).contains(message), "{args:?}: {}", stderr(&out));
    }
}

/// `replay` vergleicht nur mit `--golden`: ein vertippter Schalter (`--gloden`)
/// und eine kaputte Tickzahl duerfen nicht still in einen Lauf ohne Vergleich
/// bzw. mit den Ticks der Aufzeichnung fallen.
#[test]
fn replay_refuses_a_misspelled_switch_and_a_bad_tick_count() {
    let record = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("takt-{}-replay.trace", std::process::id()));
    let record = record.to_str().expect("Pfad").to_string();
    let run = takt(&["run", "corpus-try/13_framing.takt", "--ticks", "3", "--record", &record]);
    assert!(run.status.success(), "{}", stderr(&run));
    let typo = takt(&["replay", "corpus-try/13_framing.takt", "--record", &record, "--gloden", &record]);
    assert!(!typo.status.success() && stderr(&typo).contains("unbekannter Schalter `--gloden`"), "{}", stderr(&typo));
    let bad = takt(&["replay", "corpus-try/13_framing.takt", "--record", &record, "--ticks", "abc"]);
    assert!(!bad.status.success() && stderr(&bad).contains("--ticks"), "{}", stderr(&bad));
    let _ = std::fs::remove_file(&record);
}

/// `bench --runs abc` faellt nicht still auf 200 Laeufe zurueck.
#[test]
fn bench_refuses_a_bad_run_count() {
    let out = takt(&["bench", "--board", "stm32f401", "--runs", "abc"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("--runs"), "{}", stderr(&out));
}

/// 11.5: Jeder Posten von `size` nennt seine Herkunft, und die Summe ist die
/// Summe der Posten ohne `offen`.
#[test]
fn size_sums_exactly_the_items_that_are_not_open() {
    let out = takt(&["size", "corpus-try/13_framing.takt"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    let mut sum = 0u64;
    let mut items = 0;
    for line in text.lines().skip(1).take_while(|l| !l.trim_start().starts_with("Summe")) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let (value, origin) = (fields[fields.len() - 2], fields[fields.len() - 1]);
        assert!(["exakt", "Vertrag", "gemessen", "offen"].contains(&origin), "Herkunft fehlt: {line}");
        let value: u64 = value.parse().unwrap_or_else(|_| panic!("kein Wert: {line}"));
        if origin != "offen" {
            sum += value;
        }
        items += 1;
    }
    assert!(items >= 10, "{text}");
    let total = text.lines().find(|l| l.trim_start().starts_with("Summe")).expect("Summe");
    assert_eq!(total.split_whitespace().nth(1), Some(sum.to_string().as_str()), "{text}");
}

/// `sim --ticks 4` laeuft die Ticks 0 bis 3; die Schlusszeile steht bei t=4,
/// danach kommt nichts mehr.
#[test]
fn sim_stops_after_the_requested_ticks() {
    let out = takt(&["sim", "corpus-try/13_framing.takt", "--ticks", "4"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    let ticks: Vec<u64> = text.lines().filter_map(|l| l.strip_prefix("t=")?.split(' ').next()?.parse().ok()).collect();
    assert_eq!(ticks.iter().max(), Some(&4), "{text}");
    assert!(text.contains("t=4 verdict-final"), "{text}");
}

/// 8.4: Ein unbekannter Profilname ist ein Fehler des Aufrufs mit Namen und
/// den vorhandenen Profilen, kein interner Fehler (`Bug(...)`).
#[test]
fn an_unknown_profile_is_a_user_error() {
    for args in [
        &["sim", "corpus-try/44_campaign.takt", "--ticks", "2", "--params-profile", "NOPE"][..],
        &["run", "corpus-try/44_campaign.takt", "--ticks", "2", "--params-profile", "NOPE"],
        &["test", "corpus-try/38_scenarios.takt", "--params-profile", "NOPE"],
    ] {
        let out = takt(args);
        let err = stderr(&out);
        assert!(!out.status.success(), "{args:?}");
        assert!(err.contains("Profil `NOPE` gibt es nicht") && !err.contains("Bug("), "{args:?}: {err}");
    }
}

/// Ein Stimulus mit einem Kanal, den das Programm nicht hat, ist ein
/// Eingabefehler mit Datei und Zeile wie ein Lesefehler des Stimulus, kein
/// interner Fehler (`Bug(...)`).
#[test]
fn a_stimulus_error_names_its_line() {
    let stim = scratch("stim.trace", "t=0 in p 5 bar\nt=1 in gibtsnicht 5\n");
    let path = stim.to_str().expect("Pfad");
    let out = takt(&["sim", "corpus-try/38_scenarios.takt", "--ticks", "2", "--stim", path]);
    let err = stderr(&out);
    assert!(!out.status.success());
    assert!(err.contains(&format!("{path}: Zeile 2:")) && err.contains("gibtsnicht"), "{err}");
    assert!(!err.contains("Bug("), "{err}");
    let _ = std::fs::remove_file(&stim);
}

/// Python fuer die Werkzeuge in `grammar/`.
fn python() -> Option<&'static str> {
    ["python", "python3"]
        .into_iter()
        .find(|p| Command::new(p).arg("--version").output().is_ok_and(|o| o.status.success()))
}

/// **Der Grammatik-Fuzzer mit festem Seed** (plan.md 3, SYN-011): zwanzig
/// Programme aus `takt.ebnf`, je fuenf Fassungen, durch Parser, Orakel und
/// Formatter dieses Binaers; keine Fassung scheitert, und die Abdeckung der
/// Alternativen sinkt nicht (Ratsche).
#[test]
fn the_grammar_fuzzer_finds_nothing_with_a_fixed_seed() {
    let Some(python) = takt_testkit::require("python", python(), "Python 3 auf den PATH") else { return };
    let keep = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("takt-fuzz-{}", std::process::id()));
    let out = Command::new(python)
        .args(["-X", "utf8", "grammar/fuzz_grammar.py", "--count", "20", "--seed", "1", "--keep"])
        .arg(&keep)
        .env("TAKT", env!("CARGO_BIN_EXE_takt"))
        .current_dir(root())
        .output()
        .expect("Fuzzer startet");
    let text = stdout(&out);
    let summary = text.lines().find(|l| l.starts_with("20 Programme x 5 Fassungen, ")).unwrap_or_default();
    assert!(out.status.success() && summary.contains(", 0 fehlerhaft;"), "{text}\n{}", stderr(&out));
    let hit: u32 = summary
        .split("Alternativen: ")
        .nth(1)
        .and_then(|r| r.split(' ').next())
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("keine Abdeckung: {summary}"));
    assert!(hit >= 272, "Abdeckung gesunken: {summary}");
    let _ = std::fs::remove_dir_all(&keep);
}

/// Ein Programm, dessen einziger Ausdruck `expr` ist.
fn with_expression(name: &str, expr: &str) -> PathBuf {
    scratch(
        name,
        &format!(
            "system:\n    language = 1\n    tick     = 10 ms\n\noutput y : int @ hw(\"o/y\") with safe = 0\n\n\
             machine m:\n    var a : int in 0..1 = 1\n\n    initial RUN\n\n    state RUN:\n        loop:\n            y = {expr}\n"
        ),
    )
}

/// 2.1: Mit den Grenzen der Ebenen (64) und der Knoten (256) ist die Rekursion
/// jedes Werkzeugs beschraenkt. Ein Ausdruck an der Grenze - als Summe und als
/// Kette in 15 Klammerebenen - laeuft durch jede Stufe der Kommandozeile, ohne
/// dass ein Stapel ueberlaeuft.
#[test]
fn an_expression_at_the_limit_passes_every_stage() {
    let sum = vec!["a"; 256].join(" + ");
    let mut nested = "a".to_string();
    for _ in 0..15 {
        nested = format!("({nested}){}", " + a".repeat(16));
    }
    for (name, expr) in [("deep-sum.takt", sum), ("deep-nested.takt", nested)] {
        let file = with_expression(name, &expr);
        let path = file.to_str().expect("Pfad");
        let ll = file.with_extension("ll");
        let stages: [&[&str]; 8] = [
            &["parse", "--ast", path],
            &["parse", "--debug", path],
            &["fmt", "--check", path],
            &["tokens", path],
            &["check", path],
            &["sim", path, "--ticks", "2"],
            &["mir", "--hash", path],
            &["build", path, "--emit", "ir", "--prefix", "deep", "--out", ll.to_str().expect("Pfad")],
        ];
        let failed: Vec<String> = stages
            .iter()
            .filter_map(|args| {
                let out = takt(args);
                (!out.status.success()).then(|| {
                    let err = stderr(&out);
                    format!("{name}: takt {}: {}", args[0], err.lines().last().unwrap_or_default())
                })
            })
            .collect();
        assert!(failed.is_empty(), "{}", failed.join("\n"));
        let _ = std::fs::remove_file(&file);
        let _ = std::fs::remove_file(&ll);
    }
}

/// Eine Summe aus 64 Gliedern liegt weit unter den Grenzen aus 2.1; `takt check`
/// lief damit auf dem Hauptthread (unter Windows 1 MiB) ueber. Die CLI bemisst
/// ihren Stapel nach den Grenzen.
#[test]
fn a_sum_of_64_links_passes_check() {
    let file = with_expression("sum-64.takt", &vec!["a"; 64].join(" + "));
    let out = takt(&["check", file.to_str().expect("Pfad")]);
    assert!(out.status.success(), "{}", stderr(&out));
    let _ = std::fs::remove_file(&file);
}

/// 7.3 (RT-003): `takt timing` nennt die vier Kennzahlen eines Laufs, dazu die
/// Ueberlaeufe. Uebergelaufen ist ein Tick, dessen Schritt erst nach dem
/// Beginn des naechsten auf dem festen Raster endet: drift + took > T0.
#[test]
fn timing_counts_overruns_on_the_raster() {
    let ms = 1_000_000;
    let lines = [(0, 4, 0), (1, 6, 5), (2, 11, 0), (3, 1, 12), (4, 5, 5)];
    let text: String = lines
        .iter()
        .map(|(k, took, drift)| format!("t={k} time took={} drift={} slept=0\n", took * ms, drift * ms))
        .collect();
    let file = scratch("timing.trace", &text);
    let out = takt(&["timing", file.to_str().expect("Pfad"), "--tick", "10000000"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let report = stdout(&out);
    for line in [
        "  verspaetete Ticks   1\n",
        "  verlorene Perioden  1\n",
        "  Ueberlaeufe         3\n",
        "  groesster Rueckstand 12000000 ns\n",
        "  laengster Schritt   11000000 ns\n",
    ] {
        assert!(report.contains(line), "`{}` fehlt:\n{report}", line.trim_end());
    }
    let _ = std::fs::remove_file(&file);
}
