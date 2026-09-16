//! Jobs (4.5, Pruefung 44): `job v = f(args)` startet eine `native job`,
//! `v.done` und `v.result` sind Inputs, und das Modell des Interpreters
//! liefert das Ergebnis `ceil(duration / T0)` Ticks nach dem Start.

use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, run};
use takt_mir::Program;
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 10 ms\n\n";
const JOB: &str =
    "native job sha256(b: bytes<64>) -> bytes<32> with cost = 60000, stack = 512, duration = 25 ms, total\n";

fn compile(body: &str) -> Result<Program, Vec<String>> {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    if errors.is_empty() { Ok(out.program.expect("Programm")) } else { Err(errors) }
}

fn ok(body: &str) -> Program {
    compile(body).unwrap_or_else(|e| panic!("unerwartete Fehler:\n{}", e.join("\n")))
}

fn errors(body: &str) -> String {
    compile(body).expect_err("Fehler erwartet").join("\n")
}

fn trace(p: &Program, stimulus: &str, ticks: u64) -> String {
    let stimulus = Trace::parse(stimulus).expect("Stimulus");
    run(p, &stimulus, &RunOptions { ticks, ..Default::default() }).expect("Lauf").trace.render()
}

/// Die Sequenz haengt am Handle: `job`, `until v.done`, dann das Ergebnis.
const HASHING: &str = "
output word : u32 @ hw(\"o/word\") with safe = 0
fn first(d: bytes<32>) -> u32:
    return (d[0] as u32) | ((d[1] as u32) << 8) | ((d[2] as u32) << 16) | ((d[3] as u32) << 24)
machine m:
    var msg : bytes<64> = default
    var d   : bytes<32> = default
    initial RUN
    state RUN:
        sequence:
            msg.push(0x61)
            msg.push(0x62)
            msg.push(0x63)
            job v = sha256(msg)
            until v.done timeout 1 s -> FAILED
            d = v.result.or(default)
            word = first(d)
            -> DONE
    state DONE:
        when false: -> RUN
    state FAILED:
        when false: -> RUN
";

#[test]
fn a_job_completes_after_its_duration() {
    // 25 ms bei 10 ms Tick: fertig im dritten Tick nach dem Start.
    let p = ok(&format!("{JOB}{HASHING}"));
    let t = trace(&p, "", 10);
    assert!(t.contains("t=3 job m v done"), "{t}");
    assert!(t.contains("t=3 out word 3205920954"), "{t}");
    assert!(t.contains("t=4 state m DONE"), "{t}");
}

#[test]
fn a_recording_moves_the_completion_tick() {
    // 4.5: Die Aufzeichnung ersetzt das Modell.
    let p = ok(&format!("{JOB}{HASHING}"));
    let t = trace(&p, "t=6 job m v done\n", 10);
    assert!(t.contains("t=6 job m v done"), "{t}");
    assert!(!t.contains("t=3 job"), "{t}");
    assert!(t.contains("t=6 out word 3205920954"), "{t}");
}

#[test]
fn a_fault_cancels_the_running_job() {
    // 5.3: Der Fault-Uebergang bricht den Job ab — er endet nie.
    let p = ok(&format!(
        "{JOB}
output seen : bool @ hw(\"o/seen\") with safe = false
machine m:
    fault -> SAFE
    var msg : bytes<64> = default
    initial RUN
    state RUN:
        sequence:
            job v = sha256(msg)
            check false, \"boom\"
            until v.done timeout 1 s -> SAFE
            seen = true
    state SAFE:
        when false: -> RUN
"
    ));
    let t = trace(&p, "", 10);
    assert!(t.contains("fault m CheckFailed"), "{t}");
    assert!(!t.contains("job m v done"), "{t}");
}

#[test]
fn a_job_needs_a_native_job() {
    let e = errors(
        "native fn sha256(b: bytes<64>) -> bytes<32> with cost = 60000, stack = 512, total
machine m:
    var msg : bytes<64> = default
    initial RUN
    state RUN:
        sequence:
            job v = sha256(msg)
",
    );
    assert!(e.contains("SC-44") && e.contains("kein `native job`"), "{e}");
}

#[test]
fn a_machine_has_at_most_two_handles() {
    let e = errors(&format!(
        "{JOB}
machine m:
    var msg : bytes<64> = default
    initial RUN
    state RUN:
        sequence:
            job a = sha256(msg)
            job b = sha256(msg)
            job c = sha256(msg)
"
    ));
    assert!(e.contains("K_j = 2"), "{e}");
}

#[test]
fn a_handle_keeps_its_native() {
    let e = errors(&format!(
        "{JOB}native job crc32(b: bytes<64>) -> u32 with cost = 1600, stack = 16, duration = 1 ms, total
machine m:
    var msg : bytes<64> = default
    initial RUN
    state RUN:
        sequence:
            job v = sha256(msg)
            job v = crc32(msg)
"
    ));
    assert!(e.contains("traegt schon `sha256`"), "{e}");
}
