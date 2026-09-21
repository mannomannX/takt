//! Prüfung 59: Ein gepolltes Gerät läuft zwischen zwei Ticks nicht über
//! (12.9, FB-10).
//!
//! **Was hier belegt wird.** Die Ungleichung `fifo_depth / byte_rate >=
//! P_m + jitter + wcet_poll` urteilt nur, wenn alle vier Größen da sind.
//! Fehlt eine, ist das ein Fehler und kein Schweigen — sonst wäre eine
//! unentscheidbare Prüfung von einer bestandenen nicht zu unterscheiden.
//! `with polling = unchecked` ist der ausdrückliche Verzicht.

use takt_diag::Policy;
use takt_mir::hardware::{self, Hardware, Target};

/// Die Kalibrierung ist grob, aber vollständig: `wcet_poll` ist damit eine
/// Zahl und keine Lücke.
const TARGET: &str = "\
# takt-hw 1
[target.probe]
core_hz = 84000000
i32 = 11905
i64 = 35715
f32 = 11905
f64 = 1190500
mem = 23810
call = 47620
native = 0
t_io = 120000
";

/// Ein Gerät mit FIFO, die Tickquelle mit gemessenem Jitter, und der
/// Lesekanal des Ports trägt das Gerät — das ist die ganze Bindung.
fn hw_with(fifo: &str, rate: &str, jitter: &str) -> Hardware {
    let text = format!(
        "{TARGET}\n\
         [device.uart]\n\
         driver = \"uart-polled\"\n\
         {fifo}{rate}\n\
         [channel sys/clock]\n\
         direction = input\n\
         {jitter}\n\
         [channel mmio/0x60000000/r]\n\
         direction = input\n\
         device = uart\n"
    );
    hardware::parse(&text).expect("Konfiguration lesbar")
}

fn ziel() -> Target {
    hardware::parse(TARGET).expect("lesbar").target("probe").expect("Ziel").clone()
}

fn compile(src: &str) -> takt_mir::Program {
    let options = takt_sema::Options { policy: Policy::default(), build: takt_sema::Build::Sim, profile: None };
    let out = takt_sema::compile(src, &options);
    let fehler: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(fehler.is_empty(), "{}", fehler.join("\n"));
    out.program.expect("Programm")
}

/// `every` setzt die Periode `P_m`, `attrs` den Maschinenkopf.
fn program(every: &str, attrs: &str) -> takt_mir::Program {
    compile(&format!(
        "system:\n    language = 1\n    tick = 1 ms\n    tick_source = hw(\"sys/clock\")\n\n\
         record UartRegs layout little:\n    status : u8 with bits:\n        tx_full : bool at 0 ro\n\n\
         port ust : UartRegs @ mmio(0x60000000)\n\n\
         output busy : bool @ hw(\"o/busy\") with safe = false\n\n\
         driver machine uart0{every}{attrs}:\n    initial RUN\n\n    \
         state RUN:\n        loop:\n            busy = ust.status.tx_full\n"
    ))
}

fn codes(d: &[takt_diag::Diagnostic]) -> Vec<&str> {
    d.iter().map(|x| x.code).collect()
}

/// **Ein FIFO, das langsamer vollläuft als der Treeiber hinsieht, meldet
/// nichts.** 1024 Byte bei 11 520 B/s sind 88 ms; der Treiber sieht alle
/// 10 ms plus Jitter plus WCET hin.
#[test]
fn a_fifo_that_outlasts_the_period_is_accepted() {
    let p = program(" every 10 ms", "");
    let hw = hw_with("fifo_depth = 1024\n", "byte_rate = 11520\n", "jitter_ns = 250000\n");
    let d = takt_sema::calibrated::polling(&p, &hw, Some(&ziel()));
    assert!(d.is_empty(), "{:?}", codes(&d));
}

/// **Ein FIFO, das schneller vollläuft, ist ein Fehler mit beiden Zahlen.**
/// 8 Byte bei 11 520 B/s sind 694 us — weniger als eine Periode.
#[test]
fn a_fifo_that_overruns_between_ticks_is_an_error() {
    let p = program(" every 10 ms", "");
    let hw = hw_with("fifo_depth = 8\n", "byte_rate = 11520\n", "jitter_ns = 250000\n");
    let d = takt_sema::calibrated::polling(&p, &hw, Some(&ziel()));
    assert_eq!(codes(&d), ["SC-59"], "{d:?}");
    let text = format!("{}", d[0]);
    assert!(text.contains("zu selten"), "{text}");
}

/// **Fehlt eine Größe, ist die Prüfung nicht entscheidbar — und sagt es.**
#[test]
fn a_missing_quantity_is_reported_not_passed_over() {
    let p = program(" every 10 ms", "");
    let hw = hw_with("fifo_depth = 1024\n", "", "jitter_ns = 250000\n");
    let d = takt_sema::calibrated::polling(&p, &hw, Some(&ziel()));
    assert_eq!(codes(&d), ["SC-59"], "{d:?}");
    let text = format!("{}", d[0]);
    assert!(text.contains("byte_rate"), "{text}");
}

/// **Ohne Kalibrierung fehlt `wcet_poll`** — dieselbe Meldung, andere Lücke.
#[test]
fn without_a_calibration_the_wcet_is_the_missing_one() {
    let p = program(" every 10 ms", "");
    let hw = hw_with("fifo_depth = 1024\n", "byte_rate = 11520\n", "jitter_ns = 250000\n");
    let d = takt_sema::calibrated::polling(&p, &hw, None);
    assert_eq!(codes(&d), ["SC-59"], "{d:?}");
    assert!(format!("{}", d[0]).contains("wcet_poll"), "{:?}", d[0]);
}

/// **`with polling = unchecked` verzichtet ausdrücklich.** Auch dort, wo
/// die Rechnung fehlschlüge — das ist der Sinn der Freigabe.
#[test]
fn an_explicit_release_silences_the_check() {
    let p = program(" every 10 ms", " with polling = unchecked");
    let hw = hw_with("fifo_depth = 8\n", "byte_rate = 11520\n", "jitter_ns = 250000\n");
    let d = takt_sema::calibrated::polling(&p, &hw, Some(&ziel()));
    assert!(d.is_empty(), "{:?}", codes(&d));
}

/// **Ein Gerät ohne FIFO-Angaben ist kein gepolltes Gerät.** Prüfung 59
/// gilt für gepollte Geräte; ein GPIO hat kein FIFO und keine Frist.
#[test]
fn a_device_without_a_fifo_is_not_polled() {
    let p = program(" every 10 ms", "");
    let hw = hw_with("", "", "jitter_ns = 250000\n");
    let d = takt_sema::calibrated::polling(&p, &hw, Some(&ziel()));
    assert!(d.is_empty(), "{:?}", codes(&d));
}

/// **Ohne Kanal, der das Gerät nennt, gibt es keine Zuordnung.** Die
/// Adresse ist der Schlüssel (8.10); fehlt der Eintrag, pollt niemand.
#[test]
fn a_device_no_port_points_at_is_not_attributed_to_a_machine() {
    let p = program(" every 10 ms", "");
    let text = format!(
        "{TARGET}\n[device.uart]\ndriver = \"uart-polled\"\nfifo_depth = 8\nbyte_rate = 11520\n\n\
         [channel sys/clock]\ndirection = input\njitter_ns = 250000\n"
    );
    let hw = hardware::parse(&text).expect("lesbar");
    let d = takt_sema::calibrated::polling(&p, &hw, Some(&ziel()));
    assert!(d.is_empty(), "{:?}", codes(&d));
}
