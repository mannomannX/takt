//! Native Funktionen mit Digest, Kontext und Record-Argument (4.5,
//! Pruefung 31): die Deklaration muss zur kuratierten Signatur passen, und
//! der Interpreter fuehrt `sha256_init/update/final` ueber die kanonische
//! Byteform von `Sha256Ctx` (5.9).

use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, run};
use takt_mir::Program;
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 1 ms\n\n";

fn compile(body: &str) -> Result<Program, Vec<String>> {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    if errors.is_empty() { Ok(out.program.expect("Programm")) } else { Err(errors) }
}

fn errors(body: &str) -> String {
    compile(body).expect_err("Fehler erwartet").join("\n")
}

#[test]
fn a_declaration_must_match_the_curated_signature() {
    let e = errors("native fn sha256_update(ctx: Sha256Ctx) -> Sha256Ctx with cost = 10, stack = 64, total\n");
    assert!(e.contains("SC-31") && e.contains("(Sha256Ctx, bytes<N>) -> Sha256Ctx"), "{e}");
    let e = errors("native fn crc16(b: bytes<8>) -> u32 with cost = 10, stack = 64, total\n");
    assert!(e.contains("-> u16"), "{e}");
    let e = errors("native fn sha256(b: bytes<8>) -> bytes<16> with cost = 10, stack = 64, total\n");
    assert!(e.contains("-> bytes<32>"), "{e}");
}

#[test]
fn a_duration_belongs_to_a_job() {
    let e = errors("native fn crc32(b: bytes<8>) -> u32 with cost = 10, stack = 64, duration = 1 ms, total\n");
    assert!(e.contains("nur an einem `native job`"), "{e}");
}

#[test]
fn an_unknown_native_lists_the_curated_set() {
    let e = errors("native fn md5(b: bytes<8>) -> u32 with cost = 10, stack = 64, total\n");
    assert!(e.contains("kuratierten Menge") && e.contains("sha256_update"), "{e}");
}

/// Das Programm aus `corpus-try/39_sha256.takt`: einmal am Stueck, einmal
/// in Chunks ueber `Sha256Ctx`, dazu ein HMAC — je das erste Wort des
/// Digests als Output.
const PROGRAM: &str = include_str!("../../../corpus-try/39_sha256.takt");

#[test]
fn the_chunked_digest_equals_the_one_shot() {
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(PROGRAM, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let p = out.program.expect("Programm");
    let t = run(&p, &Trace::default(), &RunOptions { ticks: 2, ..Default::default() }).expect("Lauf").trace.render();
    // FIPS 180-4, `abc`: ba7816bf…; RFC 2104 mit Schluessel `Jefe` ueber `abc`.
    assert!(t.contains("t=0 out one_shot 3205920954"), "{t}");
    assert!(t.contains("t=0 out chunked 3205920954"), "{t}");
    assert!(t.contains("t=0 out mac 1340929148"), "{t}");
}
