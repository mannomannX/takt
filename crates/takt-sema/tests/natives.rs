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

/// Eine Takt-Funktion, die einen Byteblock aus einer Hexfolge baut.
fn bytes_fn(name: &str, cap: usize, hex: &str) -> String {
    let items: Vec<String> =
        hex.as_bytes().chunks(2).map(|p| format!("0x{}", std::str::from_utf8(p).expect("ascii"))).collect();
    format!(
        "fn {name}() -> bytes<{cap}>:
    var b : bytes<{cap}> = default
    for x in [{}]:
        b.push(x as u8)
    return b
",
        items.join(", ")
    )
}

const KEY: &str = "60fed4ba255a9d31c961eb74c6356d68c049b8923b61fa6ce669622e60f29fb67903fe1008b8bc99a41ae9e95628bc64f2f1b20c2d7e9f5177a3c294d4462299";
const DIGEST: &str = "af2bdbe1aa9b6ec1e2ade1d694f41fc71a831d0268e9891562113d8a62add1bf";
const SIG: &str = "efd48b2aacb6a8fd1140dd9cd45e81d69d2c877b56aaf991c34d0ea84eaf3716f7cb1c942d657c41d436c7a1b6e29f65f3e900dbb9aff4064dc4ab2f843acda8";

/// `ecdsa_p256_verify` als Job (4.5): der Interpreter prueft die Signatur
/// ueber `takt-crypto` — RFC 6979 A.2.5, und eine gekippte Signatur faellt.
#[test]
fn the_signature_job_verifies_through_takt_crypto() {
    for (sig, want) in [(SIG.to_string(), "true"), (SIG.replacen("a8", "a9", 1), "false")] {
        let body = format!(
            "native job ecdsa_p256_verify(key: bytes<64>, digest: bytes<32>, sig: bytes<64>) -> bool with cost = 300, stack = 2048, duration = 30 ms, total
output ok : bool @ hw(\"o/ok\") with safe = false
output done : bool @ hw(\"o/done\") with safe = false
{}{}{}
machine m:
    initial RUN
    state RUN:
        sequence:
            job v = ecdsa_p256_verify(key = key(), digest = digest(), sig = sig())
            until v.done timeout 1 s -> FAILED
            ok = v.result.or(false)
            done = true
            -> DONE
    state DONE:
        when false: -> RUN
    state FAILED:
        when false: -> RUN
",
            bytes_fn("key", 64, KEY),
            bytes_fn("digest", 32, DIGEST),
            bytes_fn("sig", 64, &sig)
        );
        let p = compile(&body).expect("uebersetzt");
        let t =
            run(&p, &Trace::default(), &RunOptions { ticks: 60, ..Default::default() }).expect("Lauf").trace.render();
        assert!(t.contains("out done true"), "{t}");
        assert!(
            t.contains(&format!("out ok {want}")),
            "{want}:
{t}"
        );
    }
}

#[test]
fn the_signature_job_must_match_its_curated_signature() {
    let e = errors("native job ecdsa_p256_verify(key: bytes<64>, digest: bytes<16>, sig: bytes<64>) -> bool with cost = 300, stack = 2048, duration = 30 ms, total
");
    assert!(e.contains("(bytes<N>, bytes<32>, bytes<N>) -> bool"), "{e}");
}
