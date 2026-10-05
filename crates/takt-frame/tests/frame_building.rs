//! Die Bausteine des Rahmens ohne ein uebersetztes Programm (12.11, GEN-024):
//! Speicherform eines leeren Programms, der Kopf mit aufgefuelltem
//! Programmbereich, der Schutz fuer eine MPU, das Bemessen der Arena, die
//! Stummel der Treiber und die Arena als Rust-Typ.

use takt_frame::drivers::{Driver, Kind, Value, c_stubs};
use takt_frame::mcu::{Frame, McuHarness, arena_layout, build_with, rust_arena};
use takt_frame::text::Text;
use takt_llvm::symbols::Prefix;
use takt_mir::program::{Command, Config, Meta, Program};

fn empty() -> Program {
    Program::new(Config::new(1, 1_000_000))
}

/// Das Tripel des Wirts, fuer den clang die Arena bemisst.
fn host() -> &'static str {
    if cfg!(windows) { "x86_64-pc-windows-msvc" } else { "x86_64-unknown-linux-gnu" }
}

/// **Ein Programm ohne Kanaele hat je Bereich ein Byte** (11.2): Ein Zeiger
/// auf einen leeren Bereich waere keiner, den man weiterreicht.
#[test]
fn an_empty_program_has_one_byte_per_region() {
    let l = takt_frame::layout::of(&empty());
    assert_eq!((l.image, l.latch, l.params), (1, 1, 1));
    assert!(l.inputs.is_empty() && l.outputs.is_empty() && l.commands.is_empty() && l.parameters.is_empty());
}

/// Ein Programm nur mit Commands: je Command ein Byte im Abbild.
#[test]
fn a_program_with_only_commands_lays_out_one_byte_each() {
    let mut p = empty();
    for name in ["start", "stop"] {
        p.commands.push(Command { name: name.into(), wake: false, meta: Meta::default(), span: Default::default() });
    }
    let l = takt_frame::layout::of(&p);
    assert_eq!(l.commands.len(), 2);
    assert!(l.commands.iter().all(|c| c.size == 1));
    assert_ne!(l.commands[0].offset, l.commands[1].offset);
    assert!(l.image > l.commands.iter().map(|c| c.offset).max().unwrap_or(0));
}

/// **`pad_to` fuellt den Programmbereich auf, nie ab** (12.3, 12.11): Eine
/// Schutzregion deckt ihn genau; kleiner als die Arena aendert nichts.
#[test]
fn the_header_pads_the_program_area_only_upwards() {
    let p = empty();
    let arena = takt_llvm::arena::of(&p);
    let x = Prefix::default();
    let plain = Text::default().header(&x, &arena, None);
    assert!(!plain.contains("program_pad"), "{plain}");
    assert_eq!(Text::default().header(&x, &arena, Some(arena.bytes.saturating_sub(1))), plain, "kleiner: nichts");
    let padded = Text::default().header(&x, &arena, Some(arena.bytes + 100));
    assert!(padded.contains("unsigned char program_pad[100];"), "{padded}");
}

/// **Mit Schutz steht der Tick an der Grenze der Region** (12.3): `protect
/// = 4096` legt die Runtime hinter 4096 Byte Programmbereich, der Uebersetzer
/// bestaetigt den Versatz, und die Arena ist entsprechend groesser.
#[test]
fn a_protected_frame_puts_the_tick_at_the_region_boundary() {
    let p = empty();
    let x = Prefix::default();
    let plain = build_with(&p, Frame::default());
    let guarded = build_with(&p, Frame { protect: Some(4096), ..Frame::default() });
    assert_eq!(guarded.tick_at, 4096);
    assert!(guarded.source.contains(&format!("offsetof(struct {x}_arena, tick) == 4096")), "{}", guarded.source);
    let (small, _) = arena_layout(&plain, &x, host(), &[]).expect("clang bemisst die Arena (FB-392)");
    let (big, align) = arena_layout(&guarded, &x, host(), &[]).expect("clang bemisst die Arena (FB-392)");
    assert!(big >= 4096 && big > small, "{small} -> {big}");
    assert!(align >= 8, "jeder Bereich auf acht Byte: {align}");
}

/// **Ohne die Konstanten im IR ist die Arena nicht zu bemessen** (12.11):
/// ein Fehler mit dem fehlenden Namen, kein geratener Wert.
#[test]
fn a_frame_without_the_arena_constants_is_an_error() {
    let x = Prefix::default();
    let mut h: McuHarness = build_with(&empty(), Frame::default());
    h.source = "int nothing_here;\n".into();
    let e = arena_layout(&h, &x, host(), &[]).expect_err("keine Konstanten");
    assert!(e.contains("arena_bytes") && e.contains("fehlt"), "{e}");
}

/// **Die Stummel sind ausdruecklich und eindeutig** (12.6): Ein Eingang
/// liefert nichts, ein Strom nichts, ein Ausgang gilt als bestaetigt, ein
/// Geraet als lebendig, ein Sendepuffer als unbekannt (-1).
#[test]
fn the_driver_stubs_answer_conservatively() {
    let value = Some(Value { c: "int32_t", rust: "i32" });
    let driver = |kind, method: &str| Driver {
        kind,
        method: method.into(),
        address: "dev/ch".into(),
        value: if matches!(kind, Kind::Input | Kind::Output) { value } else { None },
        doc: String::new(),
    };
    let drivers = [
        driver(Kind::Input, "in_dev_ch"),
        driver(Kind::Poll, "poll_dev_rx"),
        driver(Kind::Output, "out_dev_o"),
        driver(Kind::Free, "free_dev_tx"),
        driver(Kind::Alive, "alive_dev"),
    ];
    let stubs = c_stubs(&drivers, &Prefix::default());
    let line =
        |method: &str| stubs.lines().find(|l| l.contains(method)).unwrap_or_else(|| panic!("{method}:\n{stubs}"));
    for (method, result) in [
        ("in_dev_ch", "return 0;"),
        ("poll_dev_rx", "return 0;"),
        ("out_dev_o", "return 1;"),
        ("free_dev_tx", "return -1;"),
        ("alive_dev", "return 1;"),
    ] {
        assert!(line(method).contains(result), "{method}: {}", line(method));
    }
}

/// **Die Arena als Rust-Typ traegt Groesse und Ausrichtung des
/// Uebersetzers**, auch ueber acht Byte (12.11).
#[test]
fn the_rust_arena_carries_any_alignment() {
    for (bytes, align) in [(1u64, 1u64), (100, 8), (4100, 32), (65536, 4096)] {
        let text = rust_arena(bytes, align);
        assert!(text.contains(&format!("#[repr(C, align({align}))]")), "{text}");
        assert!(text.contains(&format!("MaybeUninit<[u8; {bytes}]>")), "{text}");
    }
}

/// **Der Kopfwaechter heisst `TAKT_<PRAEFIX>_H`** (12.11, GEN-019): Er
/// beginnt mit `TAKT_` und trifft so keinen reservierten Makronamen (`E…`,
/// C11 7.31.3); zwei Programme haben zwei Waechter.
#[test]
fn the_header_guard_is_takt_prefix_h() {
    for name in ["valve", "evalve"] {
        let x = Prefix::new(name).expect("Praefix");
        let h = build_with(&empty(), Frame { prefix: x.clone(), ..Frame::default() });
        let guard = format!("TAKT_{}_H", name.to_uppercase());
        assert!(
            h.header.contains(&format!(
                "#ifndef {guard}
#define {guard}
"
            )),
            "{}",
            h.header
        );
        assert!(!h.header.contains(&format!("{}_TAKT_H", name.to_uppercase())), "{}", h.header);
    }
}
