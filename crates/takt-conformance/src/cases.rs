//! Die Laeufe je Korpusprogramm, die der Vergleich der drei Ausfuehrer
//! rechnet (M11 Schritt 28): Interpreter, erzeugter Code und Modell sehen
//! je Lauf denselben Stimulus ueber dieselben Ticks.
//!
//! Jedes Programm laeuft ohne Eingaben ueber seine Fristen (KON1-006) —
//! der Weg, auf dem jeder Input `Bad` bleibt —, wo ein Stimulus
//! geschrieben ist, auch mit ihm, mit den Eingaben aus seinen Deklarationen
//! (`crate::generated`, Schritt 29a) und mit jedem Pfad, den der Solver zu
//! einer Pruefstelle, einem Uebergang oder einer Verletzung fand
//! (`crate::paths`, Schritt 28c).

use takt_mir::Program;

/// Ein Lauf: wie er heisst, sein Stimulus im Trace-Format und seine Ticks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Case {
    /// `still` ohne Eingaben, `stimulus` mit dem geschriebenen, `erzeugt …`
    /// mit erzeugten, `pfad …` mit einem des Solvers.
    pub label: String,
    /// Der Stimulus.
    pub stimulus: String,
    /// Die Tickzahl.
    pub ticks: u64,
}

/// Die Laeufe des Programms `name`.
pub fn cases(name: &str, p: &Program) -> Vec<Case> {
    let mut out = vec![Case { label: "still".into(), stimulus: String::new(), ticks: ticks_for(p) }];
    if let Some((stimulus, ticks)) = case(name) {
        out.push(Case { label: "stimulus".into(), stimulus, ticks });
    }
    out.extend(crate::generated::cases(p));
    let paths = crate::paths::load(name).into_iter();
    out.extend(paths.map(|p| Case { label: format!("pfad {}", p.label), stimulus: p.stimulus, ticks: p.ticks }));
    out
}

/// Die Obergrenze der Tickzahl eines Korpusprogramms (KON1-006): Fristen
/// darueber (`after 3 s` bei 50 us Tick, `after 7 d`) erreicht der
/// Vergleich nicht, und `UNFIRED` nennt ihre Transitionen.
pub const MAX_TICKS: u64 = 2000;

/// Wie viele Ticks ein Korpusprogramm laeuft (KON1-006): die Summe seiner
/// statischen Fristen (`after`, `timeout`, `within`, `every`, Dauern in
/// Ausdruecken) in Ticks und ein Rand, mindestens 60 und hoechstens
/// [`MAX_TICKS`]. Die Summe statt der laengsten Frist, weil Fristen
/// hintereinander liegen: `after 200 ms`, dann `until … timeout 500 ms`
/// erreicht den Timeout-Pfad erst nach 700 ms.
pub fn ticks_for(p: &Program) -> u64 {
    let mut sum = 0i64;
    for m in &p.machines {
        takt_mir::visit::for_each_expr_machine(m, &mut |e| {
            if let takt_mir::expr::ExprKind::Duration(d) = &e.kind {
                sum = sum.saturating_add((*d).max(0));
            }
        });
    }
    let ticks = u64::try_from(sum / p.config.tick.max(1)).unwrap_or(u64::MAX);
    ticks.saturating_add(10).clamp(60, MAX_TICKS)
}

/// Ein Stimulus, der das Programm `name` bewegt, mit seiner Tickzahl —
/// geschrieben fuer die Suite `beweiser` (M11 Schritt 27), wo ein Lauf
/// ohne Eingaben kaum mehr als den Anfangszustand zeigt.
fn case(name: &str) -> Option<(String, u64)> {
    Some(match name {
        // Ein Blinker mit `after` und einem Command.
        "16_timing.takt" => ("t=3 cmd go\nt=40 cmd go\n".to_string(), 60),
        // Ein Range-Fault nimmt in beiden den Fault-Pfad.
        "19_faults.takt" => (String::new(), 10),
        // Ein bewachter `check` mit Inputs.
        "01_minimal.takt" => {
            let pressure = |k: u32| if (10..20).contains(&k) { 70 } else { 40 };
            let mut stim: String = (0..=30).map(|k| format!("t={k} in tank_p {} bar\n", pressure(k))).collect();
            stim.push_str("t=2 cmd start\nt=25 cmd reset\n");
            (stim, 30)
        }
        // Ein Block mit Zustand (5.7): `step` eingebettet, der Zustand in Feldern.
        "48_contracts.takt" => ((0..=12).map(|k| format!("t={k} in level {}\n", f64::from(k) - 4.0)).collect(), 12),
        // Zusammengesetzte Werte (M11 Schritt 27a): Records, Arrays, Enums
        // mit Feldern, `case` mit Bindungen, Werten und Bereichen.
        "32_next_run_after.takt"
        | "35_persist.takt"
        | "80_payload_variants.takt"
        | "81_persist_variants.takt"
        | "83_durations.takt"
        | "95_boundary_ranges.takt"
        | "96_record_outputs.takt"
        | "113_case_ranges.takt" => (String::new(), 20),
        // Trigger (7.5, M11 Schritt 27c-17): einer feuert armiert, einer kommt
        // waehrend `CUT` und loest nach dem erneuten `arm` nichts aus.
        "65_trigger.takt" => (
            "t=2 in dut_log Erasing sector 7\nt=5 in dut_log Erasing sector 8\nt=20 in dut_log Erasing sector 9\n"
                .to_string(),
            30,
        ),
        // Die Chunk-Natives (M11 Schritt 27c-15): einmal in einem Zustand.
        "39_sha256.takt" => (String::new(), 3),
        // Das Abbild in 256 Chunks zu 256 Byte, zwei je Tick, sobald
        // `HASHING` liest (vorher liefe der Ring ueber), dann die Signatur
        // ueber den Digest mit dem leeren Schluessel: `UPD_FAILED` im Tick
        // 160 (FB-487).
        "07_embedded_field.takt" => {
            let mut stim = String::new();
            for k in 0..=170u32 {
                stim.push_str(&format!(
                    "t={k} in cell_mv 3700 mV\nt={k} in charger true\nt={k} in chg_status 3\n\
                     t={k} in flash_status IDLE\nt={k} in on_trial true\n"
                ));
            }
            for k in 11..=138u32 {
                for j in 0..2u32 {
                    let chunk: String = (0..256u32).map(|i| format!("{:02x}", (k * 7 + j * 13 + i) % 256)).collect();
                    stim.push_str(&format!("t={k} in flash_rx 0x{chunk}\n"));
                }
            }
            (stim, 170)
        }
        "89_fault_paths.takt" => ((0..=20).map(|k| format!("t={k} in p {} bar\n", (k * 7) % 100)).collect(), 20),
        // `check … for 5 ms`: vier Ticks ueber der Grenze faulten nicht,
        // elf schon; dazu ein Start.
        "03_sequences_and_faults.takt" => {
            let chamber = |k: u32| if (10..14).contains(&k) || (20..31).contains(&k) { 260 } else { 10 };
            let mut stim: String = (0..=40)
                .map(|k| format!("t={k} in chamber_p {} bar\nt={k} in supply_p 50 bar\n", chamber(k)))
                .collect();
            stim.push_str("t=2 cmd start\n");
            (stim, 40)
        }
        // `check … for 20 ms within 100 ms`: 25 Ticks ueber der Grenze.
        "14_latency.takt" => {
            let tank = |k: u32| if (10..35).contains(&k) { 390 } else { 100 };
            ((0..=50).map(|k| format!("t={k} in tank_p {} bar\n", tank(k))).collect(), 50)
        }
        "27_every.takt" | "75_implicit_checks.takt" => (String::new(), 40),
        // Zeilen vom Rand (Schritt 27c-2): je Tick eine, wie `MAXPT` erlaubt.
        "100_dispatch.takt" => {
            let lines = [
                "code 42",
                "code 600",
                "x ERR 12345 y",
                "WARN now",
                "key:value",
                "plain",
                "ERR -7",
                "a:b:c",
                "äöü:wörd",
                "code 499",
                "code -5",
            ];
            (lines.iter().enumerate().map(|(k, l)| format!("t={} in rx {l:?}\n", k + 1)).collect(), 20)
        }
        // Der letzte Sektor liegt ausserhalb von `last` und faultet im Handler.
        "23_patterns.takt" => {
            let lines = ["READY", "Erasing sector 12", "noise", "Erasing sector 7", "Erasing sector 1234"];
            (lines.iter().enumerate().map(|(k, l)| format!("t={} in rx_log {l:?}\n", 2 * k + 1)).collect(), 16)
        }
        "49_record_streams.takt" => (
            "t=1 in edges Pulse(true, 3)\nt=2 in edges Pulse(false, 2)\nt=3 in edges Pulse(false, 7)\n\
             t=5 in rx \"go 5\"\nt=6 in rx \"go 12\" t=55000000\nt=7 in rx \"no\"\nt=8 in rx \"stop\"\n\
             t=9 in edges Pulse(true, 9)\n"
                .to_string(),
            14,
        ),
        "104_linear_has.takt" | "117_many_text_handlers.takt" | "51_text_into_bytes.takt" => (String::new(), 30),
        // Funktionen aus `libtaktm`, Einheiten und Bits (Schritt 27a-3).
        "101_correct_math.takt"
        | "102_correct_math_f32.takt"
        | "103_math_domains.takt"
        | "115_affine_unit.takt"
        | "67_bitfield_access.takt"
        | "86_units.takt"
        | "97_fast_math.takt" => (String::new(), 20),
        // Ergebnisse, `every` und `check … for` in einer Schleife (Schritt 27a-4).
        "84_defaults.takt" | "93_confirmations.takt" | "54_inout.takt" => (String::new(), 40),
        // Eine Tabelle mit `interp`: die Zellspannung laeuft ueber alle Abschnitte.
        "02_units_and_data.takt" => {
            let stim: String = (0..=40)
                .map(|k| {
                    format!(
                        "t={k} in oven_t {} degC
t={k} in cell_v {} V
",
                        150 + k * 3,
                        2.8 + f64::from(k) * 0.04
                    )
                })
                .collect();
            (stim, 40)
        }
        // Generische Funktionen mit Schleifen und `break` (Schritt 27a-3).
        "62_type_generics.takt" => (
            (0..=20)
                .map(|k| {
                    format!(
                        "t={k} in a {}
t={k} in b {}.5 V
",
                        (k * 13) % 101,
                        k % 5
                    )
                })
                .collect(),
            20,
        ),
        // Ein Antrieb mit Park-Transformation, Sinus und Kosinus; die Eingaben
        // je Tick, denn sie veralten nach zwei.
        "11_foc_drive.takt" => {
            let mut stim = String::new();
            for k in 0..=40 {
                let theta = f64::from(k % 60) * 0.1;
                stim.push_str(&format!(
                    "t={k} in i_u 1.5 A\nt={k} in i_v -0.5 A\nt={k} in v_dc 48 V\nt={k} in theta_elec {theta}\n\
                     t={k} in omega_mech 100 1/s\nt={k} in temp_inverter 25 degC\nt={k} in temp_motor 30 degC\n"
                ));
            }
            stim.push_str("t=2 cmd cmd_start\n");
            (stim, 40)
        }
        // Bytes vom Bus: der Handler setzt ein Bitfeld.
        "12_bitfields.takt" => ((1..=8).map(|k| format!("t={k} in can_rx {}\n", k * 3)).collect(), 20),
        // Ausgabestroeme (Schritt 27c-3): Sendepuffer, Abholen je Tick,
        // `free`, `idle`, `sent`, ein `sim`-gespeister Eingabestrom.
        "24_send_has.takt" => {
            let lines = ["no error", "an ERR here", "ERR", "plain", "ERRERR"];
            (lines.iter().enumerate().map(|(k, l)| format!("t={} in rx {l:?}\n", 2 * k + 1)).collect(), 14)
        }
        "43_sent.takt" => ("t=2 cmd go\n".to_string(), 12),
        "25_format.takt" | "79_byte_literals.takt" | "92_idle_streams.takt" | "118_tx_idle.takt" => (String::new(), 40),
        // Interne Stroeme (Schritt 27c): Ring, Cursor je Leser, Handler je Ebene.
        "106_machine_handler.takt" | "72_handler_levels.takt" | "53_stream_kinds.takt" | "88_capture_segments.takt" => {
            (String::new(), 40)
        }
        // Drahtformat und Ausschnitte (Schritt 27a-5).
        "52_padding_fields.takt" | "13_framing.takt" | "76_stream_views.takt" => (String::new(), 30),
        // Schleifen ueber der alten Grenze von 256 Durchlaeufen (Schritt 27a-6).
        "78_length_guards.takt" => (String::new(), 80),
        // Jede Runde faultet anders; der Fault-Zustand gibt `last_fault` aus (Schritt 27a-7).
        "98_last_fault.takt" => (String::new(), 40),
        // Captures (Schritt 27c-7): Kopf und Abtastwerte, auch fehlende.
        "66_capture.takt" => (
            "t=1 in wave 5000000;2;2;1000.0;[1.0, -2.0, 3.0, 0.5]\n\
             t=4 in wave 38000000;1;3;2000.0;[0.25, 0.75]\n\
             t=9 in wave 85000000;0;4;1000.0;[-1.5, -0.5, 2.5, 4.0]\n"
                .to_string(),
            20,
        ),
        // `resume` (5.12): zurueck in FIRST, dann in SECOND (Schritt 27c-8).
        "60_resume.takt" => ("t=1 cmd pause\nt=3 cmd work\nt=7 cmd pause\nt=9 cmd work\n".to_string(), 12),
        // Matrizen (Schritt 27c-12): Kalman-Filter, QP, Faults aus `inv` und `solve`.
        "46_matrices.takt" | "77_float_faults.takt" => (String::new(), 30),
        "69_qp_box.takt" | "87_fault_kinds.takt" | "108_singular_solve.takt" => (String::new(), 20),
        // Gescopte Instanzen (Schritt 27c-11): Ein- und Austritt, ein Abort.
        "63_scoped_instances.takt" => (
            "t=0 in mode 0
t=5 in mode 1
t=9 in mode 0
t=15 in mode 1
"
            .to_string(),
            25,
        ),
        "64_scoped_exit.takt" | "121_scoped_abort.takt" => (String::new(), 20),
        // Der Besitzer liest seine Instanz (FB-483): Eintritt, Austritt, Wiedereintritt.
        "122_owner_reads_instance.takt" => ("t=2 cmd go\nt=12 cmd go\nt=15 cmd go\n".to_string(), 30),
        // Samples (Schritt 27c-10): je Tick ein volles Array, ab Tick 25 ueber `I_MAX`.
        "04_blocks_and_multirate.takt" => {
            let mut s = String::from(
                "t=0 in speed 1500.0
t=0 in fan_sp_a 40.0
t=0 in fan_sp_b 60.0
",
            );
            for k in 0..=40u32 {
                let peak = if k >= 25 { 45.0 } else { 5.0 + f64::from(k) * 0.25 };
                let items: Vec<String> = (1..=16).map(|j| format!("{:?}", peak * f64::from(j) / 16.0)).collect();
                s.push_str(&format!(
                    "t={k} in i_phase [{}]
",
                    items.join(", ")
                ));
            }
            (s, 40)
        }
        // Jobs (Schritt 27c-8): Die Aufzeichnung verlegt die Fertigstellung.
        "40_jobs.takt" => ("t=6 job m v done start=0\n".to_string(), 12),
        // Registerports (Schritt 27c-6).
        "68_uart_port.takt" | "120_port_writes.takt" => (String::new(), 30),
        // Ein FIFO-Datenregister (FB-431): zwei Schuebe, die es ueberlaufen lassen.
        "123_fifo_port.takt" => ("t=5 cmd burst\nt=9 cmd burst\n".to_string(), 30),
        // Instanz-Arrays (Schritt 27a-10).
        "74_instance_index.takt" => (String::new(), 20),
        // Records ueber eine `sim`-Bindung (Schritt 27c-5).
        "55_frames_with_bytes.takt" => (String::new(), 20),
        // Natives der kuratierten Menge (Schritt 27c-4).
        "20_native.takt" => (String::new(), 20),
        // Maps (Schritt 27a-9).
        "42_map.takt" | "114_for_pairs.takt" => (String::new(), 30),
        // Reduktionen ueber Arrays (Schritt 27a-8).
        "26_samples.takt" | "71_places.takt" => (String::new(), 30),
        // Geplante Ausgaben (Schritt 27b-5): `at`, `pulse`, `cancel`, ein
        // Timeout, der die Warteschlange leert.
        "28_scheduled.takt"
        | "82_scheduled_sleep.takt"
        | "107_cancel_and_pulse.takt"
        | "119_timeout_cancels_schedule.takt" => (String::new(), 40),
        // Handshake, Sektoren, `PANIC`, ein Ping und ein Datenrahmen vom Bus,
        // eine Flanke, auf die `at e.t + CUT_DELAY` folgt.
        "05_streams_and_protocol.takt" => (
            "t=2 cmd start\nt=4 in rx_log \"READY v1.2\"\nt=8 in rx_log \"SECTOR 3 of 10\"\n\
             t=9 in rx_log \"PANIC now\"\nt=10 in rx_bus 0x5aa501020000\nt=11 in rx_bus 0x5aa502030000\n\
             t=12 in edges Edge(true)\nt=14 in rx_log \"SECTOR 12 of 10\"\n"
                .to_string(),
            40,
        ),
        // Rahmen mit Kopf, Nutzlast und Pruefsumme: gueltig, zu kurz, fremde
        // Konstante, Laenge ausserhalb der Range, Laenge ueber dem Rahmen,
        // falsche Pruefsumme, die volle Nutzlast.
        "13_protocol_analysis.takt" => {
            let full = format!("50aa0240f0{}3f00", "01".repeat(64));
            let frames = [
                "50aa0103350102 03 05",
                "50aa01",
                "51aa0103350102 03 05",
                "50aa0150350102 03 05",
                "50aa0109350102 03 05",
                "50aa0103350102 03 06",
                &full,
            ];
            let stim = frames
                .iter()
                .enumerate()
                .map(|(k, f)| format!("t={} in rx 0x{}\n", 2 * k + 1, f.replace(' ', "")))
                .collect();
            (stim, 20)
        }
        _ => return None,
    })
}
