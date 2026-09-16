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
    let options = Options { policy: Policy::default(), build, profile: None };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    if errors.is_empty() { Ok(out.program.expect("Programm")) } else { Err(errors) }
}

const PARAMS: &str = "
param A : int in 0..10 = 0
param B : Duration in 1 ms..10 ms = 1 ms
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
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
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
        ("sweep X = [1]", "ist kein Parameter"),
        ("sweep A = [1]\n    sweep A = [2]", "schon gesweept"),
        ("sweep A = 0..4 step 0", "positive"),
        ("sweep A = [11]", "Range"),
        ("repeat 0", "ab 1"),
        ("profile NONE", "kein Profil"),
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
