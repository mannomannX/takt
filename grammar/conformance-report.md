# Takt — Konformitätsbericht (`takt bench --conformance`)

Normative Spezifikation des Berichtsformats in
`crates/takt-conformance/src/report.rs` (Referenz 13.8; Plan `plan/m10.md`,
Abschnitt 2.2 und 2.10).

Der Bericht ist das **Protokoll einer Messung**: Er sagt, wann, auf welchem
Board, in welchem Profil und mit wie vielen Wiederholungen gemessen wurde und
wie die Messwerte streuten. Die Hardware-Konfiguration (`takt-hw`, 8.10) trägt
das **Ergebnis**, das der Compiler liest. 13.8 legt die Richtung fest: Der
Bericht ist die Quelle, die Konfiguration die Ableitung, und `takt bench`
schreibt beide in einem Lauf, damit sie nicht auseinanderlaufen. Wer eine Zahl
der Konfiguration anzweifelt, findet hier, wie sie entstand.

## R1 Aufbau

Die erste nicht leere Zeile ist `# takt-conformance <version>`. Danach folgen
Abschnitte in eckigen Klammern mit Zuweisungen `schlüssel = wert`. Leerzeilen
und alles ab `#` sind Kommentar. Texte stehen in Anführungszeichen, Zahlen
ohne, Streuungen als `min mean max` in Zyklen des Kerns, Zeiten in
Pikosekunden.

Ein Leser nimmt jede Version bis zu seiner eigenen und lehnt eine neuere ab
(11.3). Er ist streng: Ein unbekannter Abschnitt oder Schlüssel ist ein Fehler
mit Zeilennummer — ein Bericht ist ein Beleg, und ein still überlesenes Feld
wäre einer weniger.

## R2 Abschnitte

| Abschnitt | Einmal | Schlüssel |
|---|---|---|
| `[run]` | ja | `date` (`"JJJJ-MM-TT"`, UTC), `board`, `target` (Zielklasse wie `takt build --target`, 12.8), `profile` (12.8), `tool` (Werkzeug mit Version), `core_hz`, `runs` (Messungen je Kern) |
| `[calibration]` | höchstens | `t_io_ps` (7.2), `stretch` (Bruch `z/n`, `1/1` ohne Streckung), `stack_reserve` (Byte, 12.3; fehlt ohne Messung), `subnormal_failures` (4.2) |
| `[probe <name>]` | je Gewicht | `ps` (Gewicht vor der Streckung), `ops` (Unterschied der Operationen zwischen kleinem und großem Kern), `small`, `large` (Streuungen) |
| `[check <name>]` | je Kern | `measured_ps` (Maximum), `bound_ps` (`Σ N_c · c_target[c] + T_IO` der gestreckten Tabelle, dazu das Mehrgewicht jeder Division, jedes `fma` und jeder Wurzel, 7.2) |
| `[kernel <name>]` | je Referenzkern | `takt`, `c` (Streuungen), `same_digest` (`true`/`false`), `implicit_checks` (3.4) |
| `[native <name>]` | je kuratierter Native | `vectors` (gerechnete Vektoren aus `grammar/takt-native.md`), `same_result` (`true`, wenn jedes Ergebnis dem des Wirts gleicht), `stack` (größter gemessener Bedarf je Aufruf in Byte), `contract` (die Zusage `stack`, 4.5) |
| `[corpus]` | höchstens | `programs`, `deviations` |

`<name>` einer Probe ist der Schlüssel der Hardware-Konfiguration (`i32`,
`mem`, `f64_div`, `f32_fma`, `f64_sqrt`); ein Kern heißt nach seiner Probe
mit `klein` oder `groß` oder nach dem Referenzkern aus 13.8.

## R3 Was ein Bericht zusagt

- **Jeder gemessene Kern liegt unter seiner Schranke**: `measured_ps ≤
  bound_ps` in jedem `[check]`. Das stellt die Streckung sicher; ein Bericht,
  in dem es nicht gilt, ist ein Fehler des Werkzeugs.
- **Das Verhältnis Takt zu C ist nur mit `same_digest = true` eine Aussage
  über die Sprache.** Weichen die Werte ab, rechneten Kern und Referenz
  Verschiedenes.
- **`subnormal_failures = 0`** heißt: Der Kern rechnet mit Subnormalen, wie
  4.2 es verlangt. Jede andere Zahl ist ein Befund gegen die Laufzeit (sie
  schaltet Flush-to-Zero aus) oder gegen den Kern.
- **Eine Native bleibt in der kuratierten Menge**, wenn `same_result = true`
  und `stack ≤ contract` (13.8, 4.5). Sonst ist das ein Befund gegen die
  Implementierung oder gegen ihre Zusage.

## R4 Vektor

```text
# takt-conformance 1
[run]
date = "2026-09-25"
board = "stm32f401"
target = "thumbv7em"
profile = "baremetal"
tool = "takt 0.1.0"
core_hz = 84000000
runs = 200

[calibration]
t_io_ps = 5000
stretch = 21/20
stack_reserve = 1432
subnormal_failures = 0

[probe i32]
ps = 11905
ops = 192
small = 100 101 110
large = 292 293 300

[check i32 gross]
measured_ps = 3571500
bound_ps = 3600000

[kernel crc32]
takt = 9000 9010 9050
c = 8800 8800 8820
same_digest = true
implicit_checks = 2

[native sha256]
vectors = 9
same_result = true
stack = 312
contract = 512

[corpus]
programs = 44
deviations = 0
```

Geprüft von `report::tests::a_report_round_trips` (Schreiben und Lesen
ergeben denselben Bericht) und `an_unknown_key_is_an_error_with_its_line`.
