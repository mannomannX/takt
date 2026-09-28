# Zertifizierungspfad (Referenz 13.4)

Stand 2026-09-28, M10 Schritt 24. Dieses Dokument sammelt, was ein
Gutachter nach IEC 61508, IEC 61513 oder DO-178C über Takt wissen muss:
welche Eigenschaften die Sprache durch Konstruktion hat und welche
Prüfung sie erzwingt, wo die Trusted Computing Base liegt, wie
Anforderungen zu Prüfstellen zurückführen und welche Messungen die
Zeitzusagen belegen. Es behauptet nichts, was nicht ein Test hält; wo
ein Test fehlt, steht es als offen da.

## 1. Teilmenge im Sinne von MISRA und JPL Power of Ten

Jede Eigenschaft ist keine Konvention, sondern eine Prüfung des Compilers
(Referenz 10) oder eine Eigenschaft der Sprache, die kein Programm
umgehen kann.

| Eigenschaft | Durchgesetzt durch | Beleg |
|---|---|---|
| Keine Rekursion | Prüfung 11: Aufrufgraph azyklisch; Prüfung 52: Instanziierungsgraph der Generics azyklisch | `corpus-try/checks/SC-11/`, `SC-52/` |
| Beschränkte Schleifen | Prüfung 11: `for`-Schranken sind Konstanten; Ströme iterieren über ein Fenster fester Kapazität (9.6, Lemma 9.6.1) | `checks/SC-11/`, `takt-sema/tests/range_boundaries.rs` |
| Keine dynamische Allokation | Die Sprache hat keinen Heap: Sammlungen haben feste Kapazität (3.9), Ströme `CAP` und `CAPB` (8.6); `takt-rt-core` ist `no_std` ohne `alloc`; der MCU-Rahmen ruft weder `malloc` noch `printf` | `takt-conformance/tests/mcu_harness.rs` (`the_harness_is_freestanding`) |
| Keine Exceptions, kein undefinierter Zustand | Jede Operation ist total (4.1): Überlauf, Division durch null, Range und Index haben einen Fault-Pfad mit definiertem Ziel (5.3); Prüfung 9: Fault-Wald azyklisch, `FAULTED` erreichbar | Sätze 9.4.x als Tests, Differential Interpreter ≡ Codegen |
| Keine Zeiger, keine Funktionszeiger | Die Sprache hat Wertsemantik ohne Referenzen (3.9); `inout` ist ein Zeiger, den Prüfung 47 gegen Aliasing hält | `checks/SC-47/` |
| Rückgabewerte werden geprüft | `T?` und `T!E` erzwingen `.valid`/`.or` (Prüfung 45) | `checks/SC-45/` |
| Jede Prüfung zur Übersetzungszeit bewiesen oder gezählt | Intervallanalyse (3.4); im Zertifizierungsmodus (`--certification`) ist jede unbewiesene implizite Prüfung ein Fehler (Prüfung 24) | `takt-sema/tests/analysis.rs` |
| Kein `unsafe` im Werkzeug | `unsafe_code = "forbid"` für jedes Crate des Workspace; die Ausnahmen stehen in Abschnitt 2 | `takt-conformance/tests/tcb_manifest.rs` |

## 2. Trusted Computing Base (9.5)

Die Sätze aus Abschnitt 9 sagen, was ein *Programm* garantiert. Sie
decken nicht ab: Compiler und LLVM, `libtaktm`, native Funktionen, die
Runtime, Treiber, Betriebssystem und Hardware. Diese Teile sind die TCB;
ihre Grenze muss sichtbar sein, nicht verwischt.

**Die Grenze im Code.** Jedes Crate des Workspace erbt
`unsafe_code = "forbid"`, und `forbid` lässt sich nicht lokal aufheben.
Was `unsafe` braucht — Registerzugriff und die C-ABI des erzeugten
Rahmens —, liegt darum in eigenen Workspaces außerhalb, jedes mit
`#![allow(unsafe_code, reason = …)]` am Kopf und einem `SAFETY`-Kommentar
an jedem Zugriff. Die Tabelle ist das Manifest; `tcb_manifest.rs` hält sie
gegen die Verzeichnisse unter `crates/`: kein Crate außerhalb des
Workspace ohne Zeile, keine Zeile ohne Crate.

### Crates mit `unsafe`

| Crate | Grund |
|---|---|
| takt-board-esp32c6 | Register des ESP32-C6 (SYSTIMER, USB-Serial-JTAG, GPIO, MWDT), CSR-Zugriffe, `wfi` |
| takt-board-stm32f401 | Register des STM32F401 (TIM2, USART1, IWDG, GPIO, Flash), DWT, `wfi` |
| takt-mcu-program | die C-ABI des erzeugten Programms und seines Rahmens (12.1) |
| takt-native-abi | die C-Einstiege der kuratierten Natives (`takt_native_*`), die der erzeugte Code auf Wirt und Board ruft; gerechnet wird in `takt-native` und `takt-crypto` (4.5, FB-293) |
| takt-bringup-esp32c6 | Treiberfunktionen `takt_out_*`/`takt_in_*` hinter der C-ABI, statische Peripherie |
| takt-bringup-stm32f401 | dasselbe für die Black Pill; im Profil `rtos` dazu RTIC 2 und `rtic-sync` — das RTOS gehört dort zur TCB (12.8) |

**Rechnen außerhalb der TCB.** Was in einem Board-Crate keine
Registerarbeit ist — Perioden in Timer-Schritte, Zyklen in Nanosekunden,
die Messschleife von `driver-test` —, steht in `takt-board-support`,
im Workspace, unter `forbid` und mit Tests auf dem Wirt.

### Native Funktionen

Die kuratierte Menge (4.5, 13.8). Jede ist bitgleich über alle Ziele,
panikfrei unter Fuzzing und mit gemessenem Stack gegen ihren Vertrag
geprüft; Projekt-Natives außerhalb dieser Menge verlangen
`tcb_policy = reviewed(…)` und eine Zeile in `natives.review` (4.5).

| Native | Crate | Abhängigkeit |
|---|---|---|
| crc32 | takt-native | keine |
| crc32c | takt-native | keine |
| crc16 | takt-native | keine |
| sum8 | takt-native | keine |
| sha256 | takt-native | keine |
| hmac_sha256 | takt-native | keine |
| sha256_init | takt-native | keine |
| sha256_update | takt-native | keine |
| sha256_final | takt-native | keine |
| ecdsa_p256_verify | takt-crypto | p256 0.13 (RustCrypto), Feature `ecdsa` |

Der Lauf-Header nennt, was ein Lauf davon benutzt hat (12.5,
`takt-interp/src/record.rs`).

Eine Implementierung je Native: Interpreter und Linux-Runtime rufen
`takt-native` direkt, der erzeugte Code dieselbe Rechnung über die
Einstiege aus `takt-native-abi` — der Rahmen bringt keine eigenen mit.
Gemessen wird der Stack am Einstieg (FB-293).

**Offen.** `rsa3072_verify`, `aes_gcm_decrypt` und `fft256` (11.4) fehlen
der Menge noch (M10 Schritt 21, FB-345).

## 3. Rückverfolgung (13.4)

- Jede Prüfstelle (`check`, `expect`, `alert`, `verify`) und jede
  Transition trägt eine stabile ID; `req "…"` verbindet sie mit einer
  Anforderung.
- `takt check --report` schließt mit dem Abschnitt „Anforderungen": je ID
  ihre Stellen mit Art, Datei, Zeile, Maschine und Block. `takt test`
  ergänzt je Stelle die Szenarien, die sie durchliefen.
- Davor steht das Gate: je Prüfung, die Zahlen des Ziels braucht (12, 28,
  29, 32, 39, 59, 60), ob sie urteilt — „ok", „verletzt", „nicht
  entscheidbar" mit der fehlenden Zahl oder „ohne Belang". Eine fehlende
  Messung ist dort sichtbar und nie eine stille Annahme.
- `takt test` berichtet die Coverage je Szenario-Lauf und nennt jede nicht
  erreichte Stelle mit Zeile (13.2); `takt driver-test` verlangt, dass jede
  Reaktion einer `driver machine` ausgelöst wurde (13.8).

## 4. Zeitzusagen und Messungen

Die Zeitzusage „der Tick hält" ist eine Rechnung — Operationen je
Aktivierung (9.4.3) mal kalibrierte Kosten je Operationsklasse — und die
Kalibrierung ist eine Messung, kein Datenblattwert.

| Messung | Werkzeug | Ablage |
|---|---|---|
| Kosten je Operationsklasse, `T_IO`, Tick-Jitter, Stack-Reserve | `takt bench --board NAME` | `corpus-try/hw/<board>.hw`, Bericht `corpus-try/hw/<board>.report` (`grammar/conformance-report.md`) |
| `guard`, `jitter` und Abtastlatenz über eine Drahtbrücke | `takt driver-test --board NAME` | Kanäle `gpio/loop_out`, `gpio/loop_in` der Konfiguration |
| Bitgleichheit und Stack der Natives | Board-Suite `the_natives_agree_with_the_host` | Testausgabe |
| Semantik auf dem Ziel | Board-Suiten, Korpus gegen den Interpreter | `takt-conformance/tests/board_*.rs` |

Die Konfiguration trägt das Ergebnis, der Bericht die Umstände (Datum,
Board, Wiederholungen, Streuung); `takt bench` schreibt beide in einem
Lauf (13.8).

## 5. Qualifizierung des Werkzeugs

Der Interpreter ist die ausführbare Semantik (Abschnitt 9). Jede
Compiler-Version wird gegen ihn gehalten: der Korpus mit Stimuli auf dem
Wirt und auf beiden Boards, Golden-Traces und die Fuzzer über Grammatik
und Maschinen (`fuzz.rs`, `fuzz_machines.rs`); eine Abweichung ist ein
Fehler des Codegens, nie der Semantik (Satz 9.4.4). Die Mutationstests der
Fault-Pfade, die 13.1 außerdem nennt, stehen noch aus. Ein qualifizierbarer Codegen nach
dem Vorbild von SCADE ist eine spätere Investition; die Architektur —
kleine MIR, Referenzinterpreter, differentielles Testen — ist darauf
ausgelegt. Versionierte Formate (MIR, Trace, Konfiguration, Bericht)
bleiben lesbar; für die MIR liegt je Version die Datei ihres Compilers
als Golden bei (`takt-sema/tests/mir-golden`).

## 6. Zielnormen

IEC 61508 (SIL 2–3), IEC 61513 (Kerntechnik), DO-178C (vom Ground
Support Equipment bis zum Flugsystem). Dieses Dokument ist kein
Sicherheitsnachweis für ein Produkt; es sagt, welche Belege Takt dafür
liefert und wo sie liegen.
