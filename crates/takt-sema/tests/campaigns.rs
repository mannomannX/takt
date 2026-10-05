//! Kampagnen (13.7): Senkung, Laufraum, Ueberlagerung des Parametervektors
//! und Pruefung 29.

use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, run};
use takt_mir::program::{StopOn, Sweep};
use takt_mir::{Program, hardware};
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 1 ms\n\n";

fn compile(body: &str, build: Build) -> Result<Program, Vec<String>> {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build, profile: None, ..Default::default() };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    if errors.is_empty() { Ok(out.program.expect("Programm")) } else { Err(errors) }
}

const PARAMS: &str = "
param A : int in 0..10 = 0
param B : Duration in 1 ms..10 ms = 1 ms
const LIMIT : int = 3
output o : int in 0..10 @ hw(\"o/o\") with safe = 0
machine m:
    initial RUN
    state RUN:
        loop:
            o = A
";

#[test]
fn the_campaigns_of_the_corpus_lower() {
    let src = include_str!("../../../corpus-try/44_campaign.takt");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
    let p = takt_sema::compile(src, &options).program.expect("Programm");
    let [sweep, first] = p.campaigns.as_slice() else { panic!("{:?}", p.campaigns) };
    assert_eq!((sweep.name.as_str(), sweep.program.as_deref()), ("gain_sweep", Some("44_campaign.takt")));
    assert_eq!(p.profiles[sweep.profile.expect("Profil").index()].name, "QUAL");
    assert_eq!((sweep.repeat, sweep.stop_on), (2, StopOn::Never));
    let [Sweep::Range { param, .. }] = sweep.sweeps.as_slice() else { panic!("{:?}", sweep.sweeps) };
    assert_eq!(p.params[param.index()].name, "GAIN");
    assert_eq!((first.repeat, first.stop_on), (1, StopOn::Fail));
    let [Sweep::List { values, .. }] = first.sweeps.as_slice() else { panic!("{:?}", first.sweeps) };
    assert_eq!(values.len(), 2);
    assert_eq!(takt_interp::campaign::runs(&p, sweep).expect("Laufraum").len(), 8);
}

#[test]
fn a_sweep_needs_a_parameter_a_positive_step_and_values_in_range() {
    for (item, want) in [
        ("sweep LIMIT = [1]", "ist kein Parameter"),
        ("sweep A = [1]\n    sweep A = [2]", "schon gesweept"),
        ("sweep A = 0..4 step 0", "positive"),
        ("sweep A = [11]", "Range"),
        ("repeat 0", "ab 1"),
        ("profile LIMIT", "kein Profil"),
    ] {
        let e = compile(&format!("{PARAMS}\ncampaign c:\n    {item}\n"), Build::Sim).expect_err(item);
        assert!(e.join("\n").contains(want), "{item}: {e:?}");
    }
}

#[test]
fn the_run_space_is_the_product_of_the_sweeps_times_repeat() {
    let p = compile(
        &format!("{PARAMS}\ncampaign c:\n    sweep A = 1..5 step 2\n    sweep B = [2 ms, 4 ms]\n    repeat 2\n"),
        Build::Sim,
    )
    .expect("uebersetzt");
    let runs = takt_interp::campaign::runs(&p, &p.campaigns[0]).expect("Laufraum");
    assert_eq!(runs.len(), 12);
    let ids: Vec<u32> = runs.iter().map(|r| r.id).collect();
    assert_eq!(ids, (1..=12).collect::<Vec<_>>());
    assert_eq!(runs[0].params, [("A".to_string(), "1".to_string()), ("B".to_string(), "2 ms".to_string())]);
    assert_eq!((runs[0].repeat, runs[1].repeat, runs[2].repeat), (1, 2, 1));
    assert_eq!(runs[2].params[1].1, "4 ms");
    assert_eq!(runs[4].params[0].1, "3");
    assert_eq!(runs[11].params, [("A".to_string(), "5".to_string()), ("B".to_string(), "4 ms".to_string())]);
}

#[test]
fn an_override_sets_the_parameter_before_the_first_tick() {
    let p = compile(PARAMS, Build::Sim).expect("uebersetzt");
    let overrides = vec![("A".to_string(), "7".to_string())];
    let options = RunOptions { ticks: 2, overrides, ..Default::default() };
    let r = run(&p, &Trace::default(), &options).expect("Lauf");
    assert!(r.trace.render().contains("t=0 out o 7"), "{}", r.trace.render());
    assert!(r.start_params.contains(&("A".to_string(), "7".to_string())), "{:?}", r.start_params);

    let bad = RunOptions { ticks: 2, overrides: vec![("A".to_string(), "11".to_string())], ..Default::default() };
    let e = run(&p, &Trace::default(), &bad).expect_err("ausserhalb der Range");
    assert!(format!("{e:?}").contains("Range"), "{e:?}");
}

#[test]
fn a_sweep_step_below_twice_the_jitter_is_rejected() {
    let body = "
param DELAY : Duration in 0 ms..10 ms = 1 ms
output pwm : bool @ hw(\"pwm/ch0\") with safe = false
machine m:
    initial RUN
    state RUN:
        enter:
            at DELAY:
                pwm = true
        when false: -> RUN
";
    let config = "# takt-hw 3\n[channel pwm/ch0]\ndirection = output\nraw = bool\nsafe = false\njitter_ns = 50000\n";
    let hw = hardware::parse(config).unwrap_or_else(|e| panic!("{e}"));
    for (step, rejected) in [("50 us", true), ("100 us", false)] {
        let p = compile(&format!("{body}\ncampaign c:\n    sweep DELAY = 0 ms..10 ms step {step}\n"), Build::Hw)
            .expect("uebersetzt");
        let diags = takt_sema::calibrated::check_bindings(&p, &hw);
        let hit = diags.iter().any(|d| d.code == "SC-29");
        assert_eq!(hit, rejected, "{step}: {diags:?}");
    }
}

/// Pruefung 29 an der Grenze `step >= 2 * jitter` (13.7): 99 us faellt
/// bei 50 us Jitter noch durch, 100 us und 150 us bestehen. Ein Parameter,
/// der in keinem `at` steht, darf beliebig fein gesweept werden.
#[test]
fn the_sweep_step_is_judged_against_twice_the_jitter_only_for_at_parameters() {
    let body = "
param DELAY : Duration in 0 ms..10 ms = 1 ms
param HOLD : Duration in 0 ms..10 ms = 1 ms
output pwm : bool @ hw(\"pwm/ch0\") with safe = false
output o : bool @ hw(\"o/o\") with safe = false
machine m:
    initial RUN
    state RUN:
        enter:
            at DELAY:
                pwm = true
        loop:
            o = HOLD > 5 ms
        when false: -> RUN
";
    let config = "# takt-hw 3\n[channel pwm/ch0]\ndirection = output\nraw = bool\nsafe = false\njitter_ns = 50000\n\n\
                  [channel o/o]\ndirection = output\nraw = bool\nsafe = false\njitter_ns = 50000\n";
    let hw = hardware::parse(config).unwrap_or_else(|e| panic!("{e}"));
    let sc29 = |sweep: &str| {
        let p = compile(&format!("{body}\ncampaign c:\n    {sweep}\n"), Build::Hw).expect("uebersetzt");
        takt_sema::calibrated::check_bindings(&p, &hw).into_iter().filter(|d| d.code == "SC-29").collect::<Vec<_>>()
    };
    for (step, rejected) in [("50 us", true), ("99 us", true), ("100 us", false), ("150 us", false)] {
        let d = sc29(&format!("sweep DELAY = 0 ms..10 ms step {step}"));
        assert_eq!(d.len(), usize::from(rejected), "{step}: {d:?}");
        assert!(d.iter().all(|d| d.is_error() && d.message.contains("2·jitter = 100 us")), "{step}: {d:?}");
    }
    assert!(sc29("sweep HOLD = 0 ms..10 ms step 1 us").is_empty(), "`HOLD` steht in keinem `at`");
}

/// 13.7: Jede Ablehnung eines Sweeps nennt Code und vollen Grund.
#[test]
fn every_malformed_campaign_item_names_its_reason() {
    for (item, code, want) in [
        ("sweep X = [1]", "SC-2", "`X` ist nicht definiert"),
        ("sweep LIMIT = [1]", "SC-3", "`LIMIT` ist kein Parameter"),
        ("sweep A = [1]\n    sweep A = [2]", "SC-2", "`A` wird schon gesweept"),
        ("sweep A = 0..4 step 0", "SC-3", "der Sweep-Schritt muss eine positive Zahl oder Dauer sein"),
        ("sweep A = 0..4 step -1", "SC-3", "der Sweep-Schritt muss eine positive Zahl oder Dauer sein"),
        ("sweep A = [11]", "SC-3", "Literal ausserhalb der Range"),
        ("sweep A = 0..20 step 1", "SC-3", "Literal ausserhalb der Range"),
        ("sweep A = [LIMIT * 4]", "SC-3", "Sweep-Wert ausserhalb der Range des Parameters"),
        ("sweep B = [2]", "SC-3", "Duration"),
        ("repeat 0", "SC-3", "`repeat` verlangt eine Zahl ab 1"),
        ("profile NONE", "SC-2", "`NONE` ist nicht definiert"),
        ("profile LIMIT", "SC-3", "`LIMIT` ist kein Profil"),
        ("stop_on maybe", "P", ""),
    ] {
        let e = compile(&format!("{PARAMS}\ncampaign c:\n    {item}\n"), Build::Sim).expect_err(item);
        assert_eq!(e.len(), 1, "{item}: {e:?}");
        assert!(e[0].contains(&format!("[{code}]")) && e[0].contains(want), "{item}: {e:?}");
    }
}

/// 12.5, 13.7: Eine Ueberlagerung nennt einen Parameter und ein Literal
/// seines Typs; alles andere bricht den Lauf vor dem ersten Tick ab.
#[test]
fn an_override_of_an_unknown_parameter_or_an_unreadable_value_is_refused() {
    let p = compile(PARAMS, Build::Sim).expect("uebersetzt");
    for (name, value) in [("Z", "1"), ("A", "abc"), ("B", "2")] {
        let options = RunOptions { ticks: 1, overrides: vec![(name.into(), value.into())], ..Default::default() };
        assert!(run(&p, &Trace::default(), &options).is_err(), "`{name} = {value}` angenommen");
    }
}

/// 13.7: Ein Bereichs-Sweep mit `lo > hi` ergaebe keinen Lauf, und die
/// Kampagne bestuende leer; das ist ein Fehler. `lo == hi` ist ein Lauf.
#[test]
fn a_sweep_range_with_lo_above_hi_is_an_error() {
    let e = compile(&format!("{PARAMS}\ncampaign c:\n    sweep A = 5..1 step 1\n"), Build::Sim).expect_err("lo > hi");
    assert_eq!(e.len(), 1, "{e:?}");
    assert!(e[0].contains("[SC-3]") && e[0].contains("leer"), "{e:?}");
    let e = compile(&format!("{PARAMS}\ncampaign c:\n    sweep B = 9 ms..2 ms step 1 ms\n"), Build::Sim)
        .expect_err("lo > hi");
    assert!(e.iter().any(|e| e.contains("[SC-3]") && e.contains("leer")), "{e:?}");
    let p = compile(&format!("{PARAMS}\ncampaign c:\n    sweep A = 3..3 step 1\n"), Build::Sim).expect("lo == hi");
    assert_eq!(takt_interp::campaign::runs(&p, &p.campaigns[0]).expect("Laufraum").len(), 1);
}
