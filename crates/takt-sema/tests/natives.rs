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
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    if errors.is_empty() { Ok(out.program.expect("Programm")) } else { Err(errors) }
}

fn errors(body: &str) -> String {
    compile(body).expect_err("Fehler erwartet").join("\n")
}

#[test]
fn a_declaration_must_match_the_curated_signature() {
    let e = errors("native fn sha256_update(ctx: Sha256Ctx) -> Sha256Ctx with cost = 10, stack = 544, total\n");
    assert!(e.contains("SC-31") && e.contains("(Sha256Ctx, bytes<N>) -> Sha256Ctx"), "{e}");
    let e = errors("native fn crc16(b: bytes<8>) -> u32 with cost = 10, stack = 64, total\n");
    assert!(e.contains("-> u16"), "{e}");
    let e = errors("native fn sha256(b: bytes<8>) -> bytes<16> with cost = 10, stack = 640, total\n");
    assert!(e.contains("-> bytes<32>"), "{e}");
}

#[test]
fn a_duration_belongs_to_a_job() {
    let e = errors("native fn crc32(b: bytes<8>) -> u32 with cost = 10, stack = 64, duration = 1 ms, total\n");
    assert!(e.contains("nur an einem `native job`"), "{e}");
}

/// T5: Der `stack`-Vertrag ist Blattkosten der Stack-Schranke; unter dem
/// gemessenen Bedarf waere sie falsch.
#[test]
fn a_stack_below_the_measured_need_is_refused() {
    let e = errors("native fn sha256(b: bytes<8>) -> bytes<32> with cost = 10, stack = 512, total\n");
    assert!(e.contains("SC-31") && e.contains("`stack = 640` oder mehr"), "{e}");
    compile("native fn sha256(b: bytes<8>) -> bytes<32> with cost = 10, stack = 4096, total\n")
        .expect("mehr ist erlaubt");
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
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
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

/// Eine Takt-Funktion, die einen Byteblock aus einer Hexfolge baut; `-`
/// ist der leere Block.
fn bytes_fn(name: &str, cap: usize, hex: &str) -> String {
    if hex == "-" {
        return format!("fn {name}() -> bytes<{cap}>:\n    var b : bytes<{cap}> = default\n    return b\n");
    }
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

fn unhex(hex: &str) -> Vec<u8> {
    if hex == "-" {
        return Vec::new();
    }
    hex.as_bytes()
        .chunks(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).expect("ascii"), 16).expect("hex"))
        .collect()
}

/// Ein Programm, das den Job `fun` einmal mit den Bloecken `args` startet
/// (Name und Kapazitaet je Argument in `params`) und nach der
/// Fertigstellung `body` ausfuehrt, im selben Tick wie `done = true`.
fn job_program(decl: &str, fun: &str, params: &[(&str, usize)], args: &[String], outputs: &str, body: &str) -> String {
    assert_eq!(params.len(), args.len(), "{fun}: {args:?}");
    let blocks: String = params.iter().zip(args).map(|((name, cap), hex)| bytes_fn(name, *cap, hex)).collect();
    let call: Vec<String> = params.iter().map(|(name, _)| format!("{name} = {name}()")).collect();
    format!(
        "{decl}
output done : bool @ hw(\"o/done\") with safe = false
{outputs}{blocks}
machine m:
    initial RUN
    state RUN:
        sequence:
            job v = {fun}({})
            until v.done timeout 1 s -> STUCK
{body}            done = true
            -> DONE
    state DONE:
        when false: -> RUN
    state STUCK:
        when false: -> RUN
",
        call.join(", ")
    )
}

/// Der Wert von `name` im Tick `t`: die letzte Zeile bis dahin, denn ein
/// unveraenderter Wert steht nicht im Trace.
fn value_at(trace: &str, t: u64, name: &str) -> Option<String> {
    trace
        .lines()
        .filter_map(|l| {
            let (tick, rest) = l.strip_prefix("t=")?.split_once(' ')?;
            let v = rest.strip_prefix("out ")?.strip_prefix(name)?.strip_prefix(' ')?;
            Some((tick.parse::<u64>().ok()?, v.to_string()))
        })
        .take_while(|(k, _)| *k <= t)
        .last()
        .map(|(_, v)| v)
}

/// Laeuft das Programm und liefert die Werte von `names` im Tick von
/// `out done true`, dazu den Trace.
fn at_done(src: &str, names: &[&str]) -> (Vec<String>, String) {
    let p = compile(src).unwrap_or_else(|e| panic!("{}", e.join("\n")));
    let t = run(&p, &Trace::default(), &RunOptions { ticks: 60, ..Default::default() }).expect("Lauf").trace.render();
    let done: u64 = t
        .lines()
        .find_map(|l| l.strip_suffix(" out done true")?.strip_prefix("t=")?.parse().ok())
        .unwrap_or_else(|| panic!("kein `out done true`:\n{t}"));
    let values = names.iter().map(|n| value_at(&t, done, n).unwrap_or_else(|| panic!("`{n}` fehlt:\n{t}"))).collect();
    (values, t)
}

/// Jede Zeile des Krypto-Blocks von `fun` als Job (4.5): `ok` im Tick der
/// Fertigstellung ist das Ergebnis der Zeile, und bei `0` steht nie
/// `out ok true` im Trace (der Default `false` aus Tick 0 allein belegte
/// nichts).
fn verify_every_line(fun: &str, decl: &str, caps: [usize; 3], least: usize) {
    let lines = crypto_lines(fun);
    assert!(lines.len() >= least, "{fun}: nur {} Zeilen", lines.len());
    let params = [("key", caps[0]), ("digest", caps[1]), ("sig", caps[2])];
    for (i, (args, want)) in lines.iter().enumerate() {
        let src = job_program(
            decl,
            fun,
            &params,
            args,
            "output ok : bool @ hw(\"o/ok\") with safe = false\n",
            "            ok = v.result.or(false)\n",
        );
        let (got, t) = at_done(&src, &["ok"]);
        let want = match want.as_str() {
            "1" => "true",
            "0" => "false",
            other => panic!("{fun} Zeile {i}: Ergebnis `{other}`"),
        };
        assert_eq!(got[0], want, "{fun} Zeile {i}:\n{t}");
        if want == "false" {
            assert!(!t.contains("out ok true"), "{fun} Zeile {i}:\n{t}");
        }
    }
}

/// `ecdsa_p256_verify` als Job (4.5): der Interpreter prueft jede Zeile des
/// Blocks ueber `takt-crypto` — RFC 6979 A.2.5, ein eigener Schluessel und
/// die Faelle, die `0` ergeben.
#[test]
fn the_signature_job_verifies_through_takt_crypto() {
    verify_every_line(
        "ecdsa_p256_verify",
        "native job ecdsa_p256_verify(key: bytes<64>, digest: bytes<32>, sig: bytes<64>) -> bool with cost = 300, stack = 5600, duration = 30 ms, total",
        [64, 32, 64],
        7,
    );
}

#[test]
fn the_signature_job_must_match_its_curated_signature() {
    let e = errors("native job ecdsa_p256_verify(key: bytes<64>, digest: bytes<16>, sig: bytes<64>) -> bool with cost = 300, stack = 5600, duration = 30 ms, total
");
    assert!(e.contains("(bytes<N>, bytes<32>, bytes<N>) -> bool"), "{e}");
}

/// Uebersetzt mit eigenem `system:`-Block.
fn compile_with(system: &str, body: &str) -> Result<Program, Vec<String>> {
    let src = format!("system:\n    language = 1\n    tick = 1 ms\n{system}\n{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    if errors.is_empty() { Ok(out.program.expect("Programm")) } else { Err(errors) }
}

const PROJECT: &str =
    "native fn crc_custom(b: bytes<64>) -> u32 from \"crypto.rs\" with cost = 5000, stack = 128, total
output w : u32 @ hw(\"o/w\") with safe = 0
machine m:
    var b : bytes<64> = default
    initial RUN
    state RUN:
        loop:
            w = crc_custom(b)
";

/// 4.5, 9.5: Ein Projekt-Native braucht die Richtlinie, laeuft nicht im
/// Interpreter und steht im TCB-Manifest des Kopfes.
#[test]
fn a_project_native_needs_the_tcb_policy_and_lands_in_the_manifest() {
    let e = compile_with("", PROJECT).expect_err("ohne Richtlinie").join("\n");
    assert!(e.contains("tcb_policy = allowlist(crc_custom)"), "{e}");
    let e = compile_with("    tcb_policy = allowlist(other)\n", PROJECT).expect_err("nicht genannt").join("\n");
    assert!(e.contains("tcb_policy"), "{e}");
    let e = compile_with("    tcb_policy = curated_only\n", PROJECT).expect_err("kuratiert").join("\n");
    assert!(e.contains("tcb_policy"), "{e}");

    let p = compile_with("    tcb_policy = allowlist(crc_custom, other)\n", PROJECT).expect("erlaubt");
    assert_eq!(p.natives[0].from.as_deref(), Some("crypto.rs"));
    assert_eq!(p.config.tcb_allowlist, ["crc_custom", "other"]);
    let header = takt_interp::record::Header::of(&p, None, &[], 1).render();
    assert!(header.contains("#! tcb projekt crc_custom from crypto.rs"), "{header}");
    assert!(!header.contains("#! tcb takt-native"), "{header}");
    let e = run(&p, &Trace::default(), &RunOptions { ticks: 1, ..Default::default() }).expect_err("Interpreter");
    assert!(format!("{e:?}").contains("laeuft nicht im Interpreter"), "{e:?}");
}

#[test]
fn the_manifest_names_takt_native_and_takt_crypto() {
    let p = compile("native fn sha256(b: bytes<64>) -> bytes<32> with cost = 60000, stack = 640, total\n")
        .expect("uebersetzt");
    let header = takt_interp::record::Header::of(&p, None, &[], 1).render();
    assert!(header.contains("#! tcb takt-native\n"), "{header}");
    let p = compile("native job ecdsa_p256_verify(key: bytes<64>, digest: bytes<32>, sig: bytes<64>) -> bool with cost = 300, stack = 5600, duration = 30 ms, total\n").expect("uebersetzt");
    let text = takt_interp::record::Header::of(&p, None, &[], 1).render();
    assert!(text.contains("#! tcb takt-crypto ecdsa_p256_verify: p256"), "{text}");
    let read = takt_interp::record::Header::parse(&text).expect("lesbar");
    assert_eq!(read.tcb.len(), 1);
    assert!(read.tcb[0].contains("p256 0.13 (RustCrypto)"), "{:?}", read.tcb);
}

/// Die Zeilen einer Funktion aus den Krypto-Bloecken von
/// `grammar/takt-native.md`: Eingaben als Hex, Ergebnis.
fn crypto_lines(fun: &str) -> Vec<(Vec<String>, String)> {
    let spec = include_str!("../../../grammar/takt-native.md");
    spec.lines()
        .filter_map(|l| l.trim().strip_prefix(fun)?.split_once(':'))
        .map(|(args, want)| (args.split_whitespace().map(str::to_string).collect(), want.trim().to_string()))
        .collect()
}

/// `rsa3072_verify` als Job (4.5): RSASSA-PSS ueber `takt-crypto`, jede
/// Zeile des Blocks — zwei gueltige Signaturen, die Faelle mit `0` bis zum
/// um ein Byte zu kurzen Schluessel.
#[test]
fn the_rsa_job_verifies_through_takt_crypto() {
    verify_every_line(
        "rsa3072_verify",
        "native job rsa3072_verify(key: bytes<384>, digest: bytes<32>, sig: bytes<384>) -> bool with cost = 900, stack = 10464, duration = 50 ms, total",
        [384, 32, 384],
        8,
    );
}

/// `aes_gcm_decrypt` als Job (4.5), jede Zeile des Blocks: bei passendem
/// Tag Laenge und CRC-32 des Klartexts (leeres `aad` und leere Nachricht
/// eingeschlossen), sonst `Err(FAILED)` ohne Klartext — auch fuer einen
/// Schluessel von 24 Byte und eine Nonce von 8 Byte.
#[test]
fn the_aes_job_decrypts_or_fails() {
    let lines = crypto_lines("aes_gcm_decrypt");
    assert!(lines.len() >= 9, "nur {} Zeilen", lines.len());
    let params = [("key", 32), ("nonce", 12), ("aad", 32), ("data", 64), ("tag", 16)];
    for (i, (args, want)) in lines.iter().enumerate() {
        let src = job_program(
            "native job aes_gcm_decrypt(key: bytes<32>, nonce: bytes<12>, aad: bytes<32>, data: bytes<64>, tag: bytes<16>) -> bytes<64> with cost = 300, stack = 2752, duration = 20 ms, total
native fn crc32(b: bytes<64>) -> u32 with cost = 1600, stack = 32, total",
            "aes_gcm_decrypt",
            &params,
            args,
            "output n : int in 0..64 @ hw(\"o/n\") with safe = 0
output sum : u32 @ hw(\"o/sum\") with safe = 0
output failed : bool @ hw(\"o/failed\") with safe = false
",
            "            n = v.result.or(default).len
            sum = crc32(v.result.or(default))
            failed = v.result.err.or(PENDING) == FAILED
",
        );
        let (got, t) = at_done(&src, &["n", "sum", "failed"]);
        let want = if want == "none" {
            ["0".to_string(), "0".to_string(), "true".to_string()]
        } else {
            let plain = unhex(want);
            [plain.len().to_string(), takt_native::crc::crc32(&plain).to_string(), "false".to_string()]
        };
        assert_eq!(got, want, "Zeile {i}:\n{t}");
        if want[2] == "false" {
            assert!(!t.contains("out failed true"), "Zeile {i}:\n{t}");
        }
    }
}

/// Die Kryptographie ist ein Job (4.5), und das Ergebnis von
/// `aes_gcm_decrypt` fasst den Klartext.
#[test]
fn crypto_natives_are_jobs_and_their_plaintext_fits() {
    let e = errors(
        "native fn rsa3072_verify(key: bytes<384>, digest: bytes<32>, sig: bytes<384>) -> bool with cost = 900, stack = 10464, total\n",
    );
    assert!(e.contains("`rsa3072_verify` ist ein Job"), "{e}");
    let e = errors(
        "native job aes_gcm_decrypt(key: bytes<32>, nonce: bytes<12>, aad: bytes<16>, data: bytes<64>, tag: bytes<16>) -> bytes<32> with cost = 300, stack = 2752, duration = 20 ms, total\n",
    );
    assert!(e.contains("fasst 32 Byte, `data` traegt bis zu 64"), "{e}");
    let e = errors(
        "native job aes_gcm_decrypt(key: bytes<32>, nonce: bytes<16>, aad: bytes<16>, data: bytes<64>, tag: bytes<16>) -> bytes<64> with cost = 300, stack = 2752, duration = 20 ms, total\n",
    );
    assert!(e.contains("bytes<12>"), "{e}");
}

/// `fft256` nimmt 256 Werte in der Breite des Programms (4.2, 4.5): Der
/// Impuls hat das flache Spektrum, gepackt wie CMSIS-DSP.
#[test]
fn fft256_transforms_an_impulse() {
    let e = errors("native fn fft256(x: [128] float) -> [128] float with cost = 8400, stack = 9000, total\n");
    assert!(e.contains("([256] float) -> [256] float"), "{e}");
    for system in ["", "    float = f32\n"] {
        let src = format!(
            "system:\n    language = 1\n    tick = 1 ms\n{system}
native fn fft256(x: [256] float) -> [256] float with cost = 8400, stack = 9000, total
output re1 : float @ hw(\"o/re1\") with safe = 0.0
output nyq : float @ hw(\"o/nyq\") with safe = 0.0
machine m:
    var x : [256] float = default
    var y : [256] float = default
    initial RUN
    state RUN:
        loop:
            x[0] = 1.0
            y = fft256(x)
            re1 = y[2]
            nyq = y[1]
"
        );
        let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
        let out = takt_sema::compile(&src, &options);
        let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
        assert!(errors.is_empty(), "{}", errors.join("\n"));
        let t = run(&out.program.expect("Programm"), &Trace::default(), &RunOptions { ticks: 1, ..Default::default() })
            .expect("Lauf")
            .trace
            .render();
        assert!(t.contains("out re1 1") && t.contains("out nyq 1"), "{system}:\n{t}");
    }
}

/// Der Trace eines Laufs von einem Tick mit `fft256` in der Breite von
/// `system`; `body` steht im `loop` von `RUN`, ein Fault fuehrt nach `SAFE`.
fn fft_trace(system: &str, outputs: &str, body: &str) -> String {
    let src = format!(
        "native fn fft256(x: [256] float) -> [256] float with cost = 8400, stack = 9000, total
output safe_entered : bool @ hw(\"o/safe\") with safe = false
{outputs}machine m:
    fault -> SAFE
    var x : [256] float = default
    var y : [256] float = default
    initial RUN
    state RUN:
        loop:
{body}    state SAFE:
        enter:
            safe_entered = true
"
    );
    let p = compile_with(system, &src).unwrap_or_else(|e| panic!("{}", e.join("\n")));
    run(&p, &Trace::default(), &RunOptions { ticks: 1, ..Default::default() }).expect("Lauf").trace.render()
}

/// `fft256` auf einem Signal, das kein Impuls ist: alle 256 Werte des
/// Interpreters bitgleich mit `takt_native::fft256`, je fuer `f64` und `f32`
/// (4.2, 4.5). Das Signal sind Vielfache von 1/64, exakt in beiden Breiten.
#[test]
fn fft256_matches_the_native_bit_for_bit() {
    for (system, wide) in [("", true), ("    float = f32\n", false)] {
        let t = fft_trace(
            system,
            "output spectrum : [256] float @ hw(\"o/spectrum[0:256]\") with safe = default\n",
            "            for i in range(256):
                x[i] = ((i * 37) % 101) as float / 64.0 - 0.75
            spectrum = fft256(x)
",
        );
        let got: Vec<&str> = t
            .lines()
            .find_map(|l| l.strip_prefix("t=0 out spectrum [")?.strip_suffix(']'))
            .unwrap_or_else(|| panic!("kein `out spectrum`:\n{t}"))
            .split(", ")
            .collect();
        let input: Vec<f64> = (0..256).map(|i| ((i * 37) % 101) as f64 / 64.0 - 0.75).collect();
        let bytes: Vec<u8> = if wide {
            input.iter().flat_map(|v| v.to_le_bytes()).collect()
        } else {
            input.iter().flat_map(|v| (*v as f32).to_le_bytes()).collect()
        };
        let mut out = [0u8; takt_native::fft::BYTES_F64];
        let n = takt_native::fft::fft256(&bytes, &mut out).expect("Laenge");
        let want: Vec<u64> = if wide {
            out[..n].chunks_exact(8).map(|c| u64::from_le_bytes(c.try_into().expect("8"))).collect()
        } else {
            out[..n].chunks_exact(4).map(|c| u64::from(u32::from_le_bytes(c.try_into().expect("4")))).collect()
        };
        let got: Vec<u64> = got
            .iter()
            .map(|v| {
                if wide {
                    v.parse::<f64>().expect("f64").to_bits()
                } else {
                    u64::from(v.parse::<f32>().expect("f32").to_bits())
                }
            })
            .collect();
        assert_eq!(got.len(), 256, "{system}");
        // Kein Impuls: das Spektrum ist nicht flach.
        assert!(want.iter().collect::<std::collections::BTreeSet<_>>().len() > 100, "{system}");
        assert_eq!(got, want, "{system}");
    }
}

/// 4.1: NaN und Inf existieren in der Sprache nicht. Sind alle Werte
/// endlich, aber ihre Summe nicht (256 mal 1e308, in `f32` 3e38), liefert
/// `fft256` Inf und NaN; die Anweisung des Aufrufs faultet
/// `Arithmetic(NonFinite)`, statt sie weiterzugeben (INT-023).
#[test]
fn a_non_finite_fft_result_faults() {
    for (system, big) in [("", "1.0e308"), ("    float = f32\n", "3.0e38")] {
        let t = fft_trace(
            system,
            "output re0 : float @ hw(\"o/re0\") with safe = 0.0\n",
            &format!(
                "            for i in range(256):
                x[i] = {big}
            y = fft256(x)
            re0 = y[0]
"
            ),
        );
        assert!(t.contains("t=0 fault m Arithmetic(NonFinite)"), "{system}:\n{t}");
        assert!(t.contains("out safe_entered true"), "{system}:\n{t}");
        assert!(!t.contains("NaN") && !t.contains("inf"), "{system}:\n{t}");
    }
}
