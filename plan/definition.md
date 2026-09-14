# Takt — Sprachdesign v0.2.8 (Arbeitstitel)

*Eine deterministische, crash-freie Steuer- und Testsprache für Hardware.*

**Status:** Konsolidierte Spezifikation — Sprachreferenz mit Begründungen. Das Dokument beschreibt, was in der Sprache möglich ist, wie es formal definiert ist und warum; es schreibt keinen Einsatzzweck vor. Herkunft und Änderungsgeschichte: Anhang A.

**Notationskonvention.** Alles, was ein Nutzer tippt — Code, Muster, Einheiten und der Pseudocode der Semantik in Abschnitt 9 — ist reines ASCII (`->`, `=>`, `<=`, `!=`, `[U]`, `{n:int}`, `degC`, `uA`, `ohm`, `pct`). Mathematische Notation (Σ, ⟦ ⟧, ∎) bleibt der Prosa der Beweise vorbehalten.

**Stufen.** Die Sprache wird in Stufen umgesetzt (Abschnitt 15): v1 (Kern), v1.1, v1.2, v2, v3. Konstrukte späterer Stufen stehen an ihrer sachlichen Stelle und sind mit der Stufe markiert, damit das Langfristbild an einem Ort bleibt.

---

## 0. Leitprinzipien und Kurzfassung

### 0.1 Was die Sprache verspricht (und wie das Versprechen eingelöst wird)

| Versprechen | Einlösung im Entwurf |
|---|---|
| **Runtime safe — „if it compiles it cannot crash"** | Totale Semantik: keine partiellen Operationen, keine Exceptions, keine dynamische Allokation, keine Rekursion, beschränkte Schleifen; jeder Laufzeitfehler ist ein *Fault-Ereignis* mit definiertem Zielzustand. Satz 9.4.2. |
| **Deterministisch** (gleiches Verhalten, rechtzeitige Outputs) | Synchron-zyklisches Modell mit logischer Zeit; Unit-Delay-Kommunikation; strikte IEEE-Arithmetik mit eigener libm; statisches Kostenbudget pro Tick. Sätze 9.4.1, 9.4.3, 9.4.4. |
| **Performant** | AOT-Kompilation über LLVM, statischer Speicher, keine Indirektion im Tick. |
| **Accessible** (HW-Ingenieure, Techniker) | Python-artige Blocksyntax, drei kleine Konstruktebenen, `sequence` für Abläufe, implizite Sicherheitsprüfungen mit expliziten Ausnahmen, Fehlermeldungen mit Handlungsvorschlag. |
| **Expected states + runtime checks** | `state`, `check`, `alert`, `expect`, `on`-Handler, hierarchische Interlocks. |
| **Abort-Logik, Safety Interlocks** | Fault-Wald, `safe`-Werte je Output, `abort`, maschinenweite `loop:`-Checks. |
| **„Which line is running"** | Instrumentierte Statement-Position pro Maschine; Zustandspfad in Telemetrie. |
| **HIL: Sim ↔ HW durch Channel-Deklaration** | `@ hw(...)` vs. `@ sim(...)`; Plant-Modelle sind gewöhnliche Maschinen. |
| **Profile ohne neuen Code** | `param` + `profile`; Validierung zur Ladezeit gegen Ranges/Einheiten. |
| **Skalierung auf 10⁵–10⁶ Channels** | Programm deklariert nur genutzte Channels; Rest über Hardware-Konfiguration nur aufgezeichnet; Channel-Arrays. |
| **KI-freundlich** | Kleine Grammatik, lokales Reasoning ohne versteckten Zustand, Compiler als Gatekeeper, kanonischer Formatter. |
| **Läuft auf Linux-SoCs und MCUs** | Ein Programm, eine Semantik; zwei Runtimes (`std`, `no_std`) hinter derselben MIR. |
| **Live-Protokolle und Logs** (UART, CAN, Zähler) | `stream<E>` mit Cursor-Semantik, typisierte Muster → DFA, `on`-Handler, `until ... matches`, `send` (8.6–8.8). |
| **Zeitkritische Reaktionen unterhalb des Ticks** | Zeitgestempelte Elemente (`.t`), geplante Ausgaben `at`/`pulse`, explizite Latenzformel; Trigger auf I/O-Knoten (v1.2) (7.5). |
| **Testergebnisse** | `measure`, `verify`, `verdict`; `scenario`; `campaign` mit Sweeps (13.5–13.7). |
| **Feldgeräte** (Batterie, IoT) | `persist var`, `idle`-Zustände mit beweisbar unsichtbarem Schlaf, Bit-Operationen, Records mit `layout` (5.9, 5.10, 3.7, 3.10). |
| **Schnelle Regelkreise** (FOC, Stromregler) | Gekoppelte Schleifen in einer Maschine ohne Verzögerung; Tick-Quelle am PWM-Timer (`tick_source`, 7.1); Kaskaden über Maschinen mit einem Basis-Tick Latenz (5.1). |
| **Abort wirkt sofort** | Abort-Phase: alle Maschinen — aktiv oder nicht — führen ihren Fault-Pfad im selben Tick aus (5.4, 9.4). |
| **Zustandsschätzer, Filter, Prüfsummen, Kryptographie** | Dimensionierte Matrizen mit Einheitentupeln (3.11) und kuratierte native Funktionen mit Kostenvertrag (4.5). |
| **Inbetriebnahme** | `tunable param`: Reglerparameter live, atomar, aufgezeichnet (8.4). |
| **Robuster Feldbetrieb** | Inputs degradieren, Outputs faulten; `debounce`/`Suspect`, `check … for d` (3.5, 5.6, 12.6). |
| **FPU-lose MCUs** | Typisiertes Kostenmodell (9.4.3) und Einheiten auf Integern (3.2). |
| **C-Geschwindigkeit auf 32- und 64-Bit-Kernen** | Darstellungsverengung aus Ranges (3.4), programmweite Fließkommabreite `system: float` (4.2), explizites `fma`, Schnellvarianten mit Fehlerschranke (11.4); Nachweis je Zielklasse per `takt bench` (13.8, 12.8). |
| **Speicher auf kleinen MCUs** | Byte-Ringe für Streams (8.6), Overlay exklusiver Zustandsspeicher, statischer Scratch statt Stack-Kopien, `sched` nur bei Verwendung (11.2); statisches Speicherbudget je Profil mit Aufschlüsselung (`takt size`, 11.5); Stack-Zusammensetzung mit Verträgen und Schutzbereich (12.3). |
| **Ablauf- und Protokolllogik über Geräten mit langen Operationen** | Interne Streams (8.6), Jobs und Chunk-Natives für Berechnungen länger als ein Tick (4.5), `T!E` (3.8), Drahtformate mit Bitfeldern und Diskriminanten (3.7), Kommando+Status-Geräte mit Flash-Modell und Stromausfall-Injektion (8.11), Startprofil (12.8), System-Channels für Image-Wechsel und einmalig programmierbare Bits (12.7); Übernahme bestehenden C-Codes per `takt import-c` und Orakel-Modus (13.9). |
| **Weiterentwicklung ohne Bruch** | Editionen, reservierte Wörter und Membernamen, offene Enums, versionierte Formate (2.5, 11.3); die Konstrukte späterer Stufen — Generics (3.12), gescopte Instanzen (5.11), `resume` (5.12), Trigger (7.5), `capture` (8.9), `property` (13.3), Knoten (12.9) — sind heute schon in Grammatik und Semantik festgelegt. |

### 0.2 Die vierundzwanzig tragenden Entscheidungen

1. **Synchron-zyklische Multirate-Ausführung.** Ein Basis-Tick T₀; jede Maschine hat eine Periode n·T₀. Innerhalb eines Ticks sind Berechnungen logisch instantan.
2. **Drei Konstruktebenen.** `fn` (rein, zustandslos), `block` (zustandsbehaftet, ohne I/O), `machine` (I/O + Kontrolle). Nur Maschinen berühren Channels.
3. **Hierarchische Zustände, Parallelität nur zwischen Maschinen.** Keine AND-States, keine History-States in v1. Single-Writer-Regel für Outputs und veröffentlichte Variablen.
4. **Schwache Transitionen, starke Faults, Entry-Tick-Regel.** Normale Übergänge wirken frühestens nach einem vollen Tick; Faults wirken sofort. Der neu betretene Zustand führt seinen `loop:`-Körper noch im Eintritts-Tick aus (mit Checks, ohne Übergänge). Outputs werden am Tick-Ende committet → ein Fault korrigiert einen unsicheren Output *vor* seiner physischen Wirkung.
5. **Fault-Wald.** Jeder Zustand hat ein Fault-Ziel; die Ziel-Relation ist statisch azyklisch und endet im impliziten Zustand `FAULTED`, der alle Outputs der Maschine auf `safe`-Werte setzt.
6. **Kommunikation zwischen Maschinen ausschließlich mit Unit-Delay.** Dadurch ist die Ausführungsreihenfolge im Tick beweisbar irrelevant und es gibt keine Kausalitätsanalyse.
7. **Zeit ist logisch und ganzzahlig.** `Duration` in Nanosekunden (i64); Timer sind Tick-Zähler; Wall-Clock-Zeit existiert nur als expliziter Input-Channel.
8. **Totale Arithmetik.** `int` = i64 mit Overflow → Fault; `float` = endliche IEEE-754-Zahl in der vom Programm festgelegten Breite (Default binary64), nicht-endliche Ergebnisse → Fault; eigene, korrekt rundende Mathematikbibliothek; keine Fast-Math, keine FMA-Kontraktion (explizites `fma` erlaubt) → bitidentische Traces über Targets hinweg.
9. **Typsystem mit physikalischen Einheiten (nominal, explizite Konversion), affinen Temperaturen, Range-Typen (Intervallanalyse) und Qualitätsflags mit implizitem Validitäts-Check.**
10. **Sequenzen sind Zucker** über Zustandsmaschinen (formales Desugaring); Profile sind Parameter-Sets; Simulation ist dieselbe Semantik mit anderen Bindings.
11. **Ereignisströme sind beschränkte FIFOs mit Konsumenten-Cursor.** Eine einzige Konsumregel: „untersucht heißt konsumiert". Überlauf ist ein Fault, kein Datenverlust im Verborgenen.
12. **Muster sind typisierte reguläre Muster.** Sie werden zur Compile-Zeit zu deterministischen endlichen Automaten; Matching ist linear, total, ohne Backtracking; Captures sind typisiert und eindeutig.
13. **Zeit unterhalb des Ticks kommt aus Zeitstempeln und geplanten Ausgaben.** Die Präzision einer Ausgabe ist die des Hardware-Timers; die Latenzuntergrenze ist explizit: `D_min <= P_m + WCET + guard`.
14. **Beobachtung ist von Steuerung getrennt.** `alert`, `log`, `measure`, `verify`, `verdict` können nie einen Fault auslösen; `check`, `expect`, `abort` sind Steuerung. Ein Test berichtet, eine Sicherheitsfunktion bricht ab.
15. **Feldbetrieb ohne Sonderpfade.** `persist var` wählt nur den Anfangszustand; `idle`-Zustände erlauben Schlaf, dessen Unsichtbarkeit in der Trace bewiesen ist (Satz 9.9.1).
16. **Abort wirkt im selben Tick für alle Maschinen.** Nach den planmäßigen Schritten führt jede Maschine mit vorgemerktem Abort- oder Runtime-Fault ihren Fault-Pfad aus (Abort-Phase); die Kosten sind statisch (Σ F_m), die Ordnungsunabhängigkeit bleibt erhalten.
17. **Der Treiberrand ist defensiv.** Deklarierte Ranges werden am Rand erzwungen (Werte außerhalb sind `Bad`), Treiberverträge werden geprüft: Input-seitige Verletzungen degradieren die Channels des Treibers zu `Bad`, Output-seitige sind Faults. Erst dadurch sind die Annahmen der Intervallanalyse gültig.
18. **Entschärfung ist immer explizit.** Ein `check` ohne `for` löst beim ersten Verstoß aus, eine Range-Verletzung ohne `debounce` ist sofort `Bad`. Bestätigungszeiten (`check … for d`) und entprellte Qualität (`debounce`, `Suspect`) sind sichtbare Deklarationen mit beschränkter Haltedauer, nie versteckte Filter.
19. **Kosten sind typisiert.** Das Budget zählt Operationen nach Klassen (`i32`, `i64`, `f32`, `f64`, `mem`, `call`, `native`) mit kalibrierter Tabelle je Target; Einheiten gelten auch auf Integern, damit FPU-lose Kerne einheitensicher rechnen.
20. **Breite als Darstellung ist vom Compiler, Breite als Semantik vom Programm.** `int` bleibt überall ℤ₆₄; der Compiler rechnet in 32 Bit, wo bewiesene Intervalle das erlauben (Lemma 3.4, semantikneutral). Die Breite von `float` legt das Programm einmal fest (`system: float`), und die Simulation rechnet identisch. Kein Typ ist targetabhängig.
21. **Genauigkeit und Rundung sind deklarierte Eigenschaften.** FPUs laufen im IEEE-Modus (keine Flush-to-Zero), `fma` ist eine explizite, korrekt gerundete Primitive, schnelle Näherungen tragen eine dokumentierte Fehlerschranke — nichts davon hängt vom Target ab.
22. **Speicher kennt der Compiler, nicht der Entwickler.** Jeder Posten — Zustände (überlagert nach Geschwisterzuständen), Streams (Byte- und Deskriptor-Ringe), Prozessabbild, Scratch, Tabellen, Stack mit Verträgen und Reserven — ist statisch berechnet und wird gegen das Budget des Profils geprüft. Große Werte werden intern per Zeiger übergeben; die Wertsemantik der Sprache bleibt, weil sie keine Referenzen hat und Überlappung deshalb ein syntaktischer Test ist.
23. **Logik in Takt, Hardware und Kryptographie in der TCB.** Berechnungen länger als ein Tick sind Jobs, deren Fertigstellung ein Input ist, oder Chunk-Natives mit explizitem Zustand; Geräte mit langen Operationen sind Kommando und Status; Warteschlangen sind interne Streams. Eine Migration aus C ist per Konstruktion crash-frei, weil jede unbewiesene Annahme ein `check` wird, und sie ist bewiesen, wenn das Original als Orakel in der Simulation Tick für Tick dasselbe liefert.
24. **Die Sprache entwickelt sich nur mit Editionen weiter.** Ein Programm nennt seine Edition; reservierte Wörter, reservierte Membernamen, offene Enums und die Verdeckungsregel der Bibliothek sorgen dafür, dass Ergänzungen nichts brechen; Konstrukte späterer Stufen sind in Grammatik und Semantik festgelegt, bevor sie implementiert werden, damit die MIR nie umgebaut werden muss.

### 0.3 Nicht-Ziele (bewusst)
Turing-Vollständigkeit innerhalb eines Ticks · dynamische Datenstrukturen · Threads · Exceptions · Reflection · Hot-Swap eines laufenden Programms · Objektorientierung.

---

## 1. Programmiermodell

### 1.1 Das Bild in einem Absatz
Ein Takt-Programm ist eine Menge von Maschinen, die im Gleichtakt einer logischen Uhr laufen. Pro Basis-Tick liest die Runtime alle Inputs in ein Prozessabbild, führt die in diesem Tick aktiven Maschinen aus und schreibt danach alle Outputs. Eine Maschine ist eine hierarchische Zustandsmaschine: Zustände haben Eintrittsaktionen, einen pro Tick ausgeführten Körper mit Berechnungen und Prüfungen, Übergangsregeln und Austrittsaktionen. Prüfungen, die scheitern, sind keine Abstürze, sondern Übergänge in Fehlerzustände, die Outputs auf sichere Werte bringen. Zwischen Maschinen fließen Daten immer mit einem Tick Verzögerung. Alles, was das Programm tun kann, ist in Größe und Laufzeit vor dem Start bekannt.

### 1.2 Die drei Ebenen

| Ebene | Zustand | I/O | Aufrufbar aus | Typische Inhalte |
|---|---|---|---|---|
| `fn` | keiner | nein | überall | Mathematik, Umrechnungen, Skalierung, Interpolation |
| `block` | lokal, pro Instanz | nein | `machine`, `block` | Filter, PID, Ratenbegrenzer, Hysterese, Flanken, Timer |
| `machine` | Zustände + Variablen | ja | — (läuft selbst) | Sequenzen, Modi, Interlocks, Regelschleifen |

Diese Trennung ist der Kern der Wiederverwendbarkeit: `fn` und `block` sind ohne Hardware testbar und verhalten sich in Simulation und Betrieb identisch; Maschinen sind die einzigen Agenten mit Seiteneffekten.

### 1.3 Tick, Perioden, Hyperperiode
- `system: tick = 1 ms` legt T₀ fest (erlaubt: 10 µs … 1 s).
- `machine m every 10 ms:` legt P_m = n_m·T₀ fest (n_m ∈ ℕ⁺; Default n_m = 1). Optional `phase = k·T₀` zur Lastverteilung.
- Aktivierung von m im Basis-Tick k ⇔ k ≡ φ_m (mod n_m).
- Aktivierung per Abwärtszähler je Maschine (`countdown[m]`: Start `phase_m / T₀`, aktiv bei 0, danach n_m − 1, sonst Dekrement), Prüfung O(|M|) pro Tick; es gibt keine Laufzeittabelle (7.2). Die Hyperperiode H = kgV(n_m) dient nur der optionalen exakten Phasenanalyse zur Compile-Zeit.
- Nicht-harmonische Perioden sind unproblematisch: Die Spitzenlast ist bei Phase 0 in Tick 0 ohnehin Σ_m B_m; `phase` kann sie senken, was der Compiler exakt prüft, wenn H ≤ 10⁶ (7.2). Harmonische Perioden bleiben Empfehlung wegen des Phasen-Nutzens, nicht wegen des Speichers.
- Der Basis-Tick kann an ein Hardware-Ereignis gebunden werden (`tick_source`, 7.1), z. B. den PWM-Timer für phasenstarre Stromregelung.

### 1.4 Datenfluss
- **Inputs** werden zu Tick-Beginn gesampelt: Wert, Qualität ∈ {Good, Suspect, Stale, Bad} (3.5), Alter.
- **Outputs** werden während des Ticks in einem Latch gesetzt und nach Abschluss aller Maschinen des Ticks committet (Write-back). Option `output_timing = boundary` verzögert das Commit auf den nächsten Tick-Anfang (LET-Semantik, jitterfrei).
- **Veröffentlichte Variablen** (`pub var`) und Zustände anderer Maschinen sind mit Unit-Delay lesbar.
- **Commands** (Operator/Netzwerk) sind Inputs vom Typ Puls (ein Tick wahr).
- **Ereignisströme** (`stream<E>`, 8.6) liefern pro Tick eine beschränkte Folge zeitgestempelter Elemente in einen Puffer; Maschinen lesen sie über ein Fenster mit Cursor. Ausgabeströme (8.8) werden beim Commit gesendet.
- **Geplante Ausgaben** (`at`, 7.5) werden zum angegebenen Hardware-Zeitpunkt geschrieben; ihr Latch-Wert gilt ab dem Tick, der den Zeitpunkt enthält.
- **Abort-Phase** (5.4, 9.4): Nach den planmäßigen Schritten führen alle Maschinen mit vorgemerktem Abort- oder Runtime-Fault — aktiv oder nicht — ihren Fault-Pfad noch im selben Tick aus; ihre Outputs stehen am Commit dieses Ticks auf den Werten des Fault-Ziels.
- **Defensiver Treiberrand** (3.5, 12.6): Lieferungen werden vor dem Bilden des Prozessabbilds geprüft; Werte außerhalb deklarierter Ranges sind `Bad`, Input-seitige Vertragsverletzungen degradieren alle Channels des Treibers zu `Bad`, Output-seitige sind `Runtime(Driver)`-Faults.
- **`follows`** (7.2): Eine Maschine, die einer anderen folgt, liest deren veröffentlichte Größen im selben Tick frisch (nach deren Schritt) statt mit Unit-Delay; die Kanten bilden einen azyklischen Graphen.
- **Tunables** (8.4): `tunable param` sind Inputs mit Halte-Semantik — Änderungen werden an Tick-Grenzen atomar übernommen und aufgezeichnet.

### 1.5 Lebenszyklus
Vor dem ersten Commit stehen alle Outputs auf ihren `safe`-Werten (Runtime und I/O-Geräte). Tick 0: alle Maschinen betreten ihren Initialzustand (Eintrittsaktionen, Entry-Tick-Regel); `persist`-Variablen (5.9) sind zu diesem Zeitpunkt bereits geladen. Das Programm läuft bis zum Stopp durch die Runtime; beim Stopp und bei jedem Runtime-Fault gehen alle Outputs auf `safe`. Programmwechsel nur zwischen Läufen.

---

## 2. Lexik und Syntax

### 2.1 Lexik
- UTF-8; Blockstruktur durch `:` und Einrückung (4 Leerzeichen, Tabs sind Fehler; der Formatter ist kanonisch). Ausdrücke, Typen und Blöcke sind höchstens 64 Ebenen tief verschachtelt; tiefer ist ein Syntaxfehler mit Vorschlag, damit kein Werkzeug an pathologischen Eingaben scheitert.
- Kommentare `# …`. Zeilenfortsetzung innerhalb offener Klammern; ohne Klammer setzt ein hängendes Komma am Zeilenende fort, ebenso eine Folgezeile, die mit `.`, einem Infix-Operator, `and`, `or` oder `with` beginnt (Lexer L2.2a). Damit sind lange Attributlisten, Methodenketten und mehrzeilige Ausdrücke umbrechbar, ohne ein Fortsetzungszeichen wie den Backslash einzuführen. `->` und `..` setzen **nicht** fort, weil `-> ZIEL` und Bereichsmuster eigene Zeilen bilden; eine lange Funktionssignatur bricht deshalb innerhalb der Parameterliste um (dort trägt die Klammer), nicht vor dem Rückgabepfeil:

```
fn build_response(req: RdmHeader, own: Uid, rt: u8,
                  pd: bytes<231>, pdl: int in 0..231) -> bytes<264>:
    var out : bytes<264> = default
    out.push(rt)
    return out
```
- Namenskonventionen (vom Compiler geprüft, Warnung bei Verstoß): `snake_case` für Variablen, Channels, Funktionen, Blöcke, Maschinen; `UPPER_SNAKE_CASE` für Konstanten, Parameter, Zustände, Enum-Varianten; `PascalCase` für Typen.
- Literale: `42`, `0x1F`, `0b1010`, `0o17`, `1_000_000`, `4.25`, `1e-3`, `true`, `false`, `none`, `"text"`, Einheitenliterale `85 degC`, `4.25 V`, `5 K/min`, `0.0005 1/s`, Dauern `200 ms`, `1.5 s`, `30 min`, `7 d`.
- Musterliterale sind Strings mit typisierten Platzhaltern `{name:kind}` (8.7); `{{` und `}}` sind Escapes.
- Alles Tippbare ist ASCII; Einheiten heißen `degC`, `uA`, `ohm`, `pct`, `um` — keine Unicode-Zeichen in Bezeichnern, Operatoren oder Einheiten.

### 2.2 Schlüsselwörter
```
system import type enum record unit const param profile stream port node unitvec property assumption
input output command fn native block machine instance scenario campaign trigger
var pub persist signal tunable driver
initial state enter loop exit on when after fault sequence wait until expect repeat step every
check alert log abort measure verify verdict send at pulse cancel raise job arm disarm then bound
program sweep stop_on
match case if elif else for in range break return pass
and or not as matches has implies always never eventually stable once
true false none default with
reserviert (ohne Bedeutung, als Bezeichner verboten): region while yield await async spawn select where impl trait module export extern unsafe try catch throw class struct union static global volatile defer
```

**Regel.** Ein Wort ist Schlüsselwort, wenn es eine Aussage, Deklaration oder Klausel am Zeilenanfang einleitet (`check`, `state`, `bound`) oder in Ausdrücken als Operator oder Literal wirkt (`and`, `matches`, `none`). Alle anderen Wörter der Grammatik (2.3) sind *kontextuell*: Typnamen (`bool`, `bytes`, `mat`, `f64`), Attributnamen (`safe`, `rate`, `debounce`), Positionswörter (`layout`, `offset`, `timeout`, `idle`, `resume`, `from`) und Systemeinträge (`tick`, `language`) gelten nur an ihrer Stelle und bleiben andernorts gewöhnliche Bezeichner. Damit darf ein Record ein Feld `offset` oder `len` haben, und die Bibliothek darf Blöcke `rate` und `debounce` nennen. Als Einheitenname nach einer Zahl kommt ein kontextuelles Wort nicht vor (`3 timeout` ist die Zahl 3 vor der Klausel `timeout`). `now`, `time_in_state`, `tick` und `last_fault` sind eingebaute Bezeichner, keine Schlüsselwörter.

### 2.3 Grammatik (EBNF)

`NEWLINE`, `INDENT`, `DEDENT` stammen aus dem Tokenizer (wie in CPython). Produktionen mit Stufenvermerk (v1.1, v1.2) gehören zu späteren Stufen (Abschnitt 15).

```
(* ======================================================================
   Takt — Grammatik, Edition 1
   Quelle fuer Parser, Formatter, Fuzzer und Abschnitt 2.3 der Referenz.
   Pruefung: python grammar/check_grammar.py   (--sync schreibt 2.3)

   Notation
     name := alt | alt      Produktion; Folgezeilen beginnen mit Leerraum
     "x"                    Terminal (Schluesselwort, Operator, Interpunktion)
     NAME                   Token aus dem Tokenizer (siehe Abschnitt Tokens)
     [ ]  { }  ( )          optional, Wiederholung, Gruppe
     (* @stage vX *)        die ganze Produktion gehoert zu einer spaeteren Stufe;
                            gemischte Produktionen nennen die Stufe an der Alternative
     (* @start *)           zusaetzlicher Startsymbol (Teilsprachen in STRING)
     (* @check 8, 25 *)     statische Pruefungen aus Abschnitt 10, die an dieser
                            Produktion ansetzen, aber vom Compiler-Gate erzwungen werden

   Schluesselwoerter (2.2) sind die Woerter, die eine Aussage, Deklaration
   oder Klausel am Zeilenanfang einleiten oder in Ausdruecken als Operator
   oder Literal wirken. Alle anderen Terminale (Typnamen, Attributnamen,
   Positionswoerter wie layout, offset, timeout, idle) sind kontextuell: sie
   gelten nur an ihrer Stelle und bleiben andernorts als Bezeichner erlaubt.
   ====================================================================== *)

(* ---------------------------------------------------------------- Tokens *)

NEWLINE     := (* Zeilenende ausserhalb offener Klammern *)
INDENT      := (* Einrueckung um 4 Leerzeichen; Tabs sind Fehler (2.1) *)
DEDENT      := (* Rueckkehr auf eine aeussere Einrueckungsstufe *)
IDENT       := (* [a-z_][a-z0-9_]* — snake_case; eingebaute Groessen now, time_in_state, tick, last_fault sind IDENT (3.3) *)
UPPER_IDENT := (* [A-Z][A-Z0-9_]* — Konstanten, Parameter, Zustaende, Varianten, Einheiten-/Typ-/Konstantenvariablen *)
TYPE_IDENT  := (* [A-Z][A-Za-z0-9]* mit mindestens einem Kleinbuchstaben — PascalCase-Typnamen *)
KEYWORD     := (* jedes Wort aus 2.2; nach "." ist es ein Membername (2.5) *)
INT         := (* [0-9][0-9_]* *)
HEX         := (* 0x[0-9a-fA-F_]+ *)
BIN         := (* 0b[01_]+ *)
OCT         := (* 0o[0-7_]+ *)
FLOAT       := (* [0-9][0-9_]*\.[0-9_]+([eE][+-]?[0-9]+)? | [0-9]+[eE][+-]?[0-9]+ *)
DURATION    := (* Zahl, Leerraum, genau ein Zeitsuffix ns us ms s min h d; Wert exakt in ns (3.3, lexer.md L4.4) *)
STRING      := (* "..." mit Escapes \" \\ \n \t; Inhalt nach format_text bzw. pattern_text *)
TEXT_CHAR   := (* ein Zeichen innerhalb eines STRING, das nicht "{" oder "}" ist *)
ADDR_WORD   := (* [A-Za-z0-9_][A-Za-z0-9_.-]* — Segment einer Hardware-Adresse, z. B. daq1, ai0, 0x36 *)

RESERVED    := "region" | "while" | "yield" | "await" | "async" | "spawn" | "select" | "where"
             | "impl" | "trait" | "module" | "export" | "extern" | "unsafe" | "try" | "catch"
             | "throw" | "class" | "struct" | "union" | "static" | "global" | "volatile" | "defer"
             (* ohne Bedeutung, als Bezeichner verboten (2.5); "while" mit eigener Meldung *)

(* ---------------------------------------------------------------- Datei *)

file           := { NEWLINE | import | system_decl | type_decl | unitvec_decl | enum_decl | record_decl | unit_decl   (* @check 1 *)
                  | stream_decl | port_decl | node_decl | property_decl | assumption_decl
                  | const_decl | param_decl | profile_decl | channel_decl | command_decl
                  | fn_decl | native_decl | block_decl | machine_decl | instance_decl
                  | scenario_decl | campaign_decl | trigger_decl }

import         := "import" IDENT { "." IDENT } [ "as" IDENT ] NEWLINE
                | "import" "channels" "from" STRING NEWLINE                                  (* 8.2 *)
system_decl    := "system" ":" NEWLINE INDENT { system_item } DEDENT
system_item    := "tick" "=" duration_lit NEWLINE
                | "output_timing" "=" ( "asap" | "boundary" ) NEWLINE
                | "fault_is_fail" "=" ( "true" | "false" ) NEWLINE
                | "tick_source" "=" "hw" "(" STRING ")" NEWLINE                              (* STRING nach address_text *)
                | "tick_tolerance" "=" const_expr [ "for" int_lit "ticks" ] NEWLINE
                | "target" "=" IDENT NEWLINE                                              (* Laufzeitprofil, 12.8 *)
                | "float" "=" ( "f32" | "f64" ) NEWLINE                                    (* Breite von float, 4.2; Default f64 *)
                | "language" "=" INT NEWLINE                                                (* Edition, 2.5 *)   (* @check 49 *)

(* ---------------------------------------------------------------- Typen und Einheiten *)

type_decl      := "type" TYPE_IDENT "=" type NEWLINE
unitvec_decl   := "unitvec" UPPER_IDENT "=" "(" unit_expr { "," unit_expr } ")" NEWLINE    (* @stage v1.1 — 3.11 *)
enum_decl      := "enum" TYPE_IDENT [ "layout" int_type ] [ "open" ] ":" variant { "," variant } NEWLINE   (* layout: 3.7; open: 2.5 *)   (* @check 51 *)
                | "enum" TYPE_IDENT [ "layout" int_type ] [ "open" ] ":" NEWLINE INDENT { variant NEWLINE } DEDENT
variant        := UPPER_IDENT [ "=" int_lit ] [ "(" field { "," field } ")" ]             (* explizite Diskriminante, auch 0x00 *)   (* einzeilig mit Kommas ODER eingerueckt ohne; nicht gemischt *)
record_decl    := "record" TYPE_IDENT [ "layout" ( "little" | "big" ) [ "," "align" "=" int_lit ] ] ":" NEWLINE INDENT { record_field } DEDENT   (* @check 46 *)
record_field   := field NEWLINE
                | IDENT ":" int_type "with" "bits" ":" NEWLINE INDENT { bitfield NEWLINE } DEDENT   (* endet mit DEDENT, daher kein NEWLINE *)
field          := ( IDENT | "_" ) ":" type [ "=" const_expr ] [ "offset" "=" int_lit ] [ "with" "len" "=" IDENT ]   (* "_": Padding; Konstantenfeld, Offset, len_field: 3.7 *)   (* @check 37, 50 *)
bitfield       := IDENT ":" ( "bool" | int_type ) "at" int_lit [ ".." int_lit ]   (* @check 46 *)
unit_decl      := "unit" unit_name "=" number [ unit_lit ] NEWLINE                          (* ohne unit_expr: dimensionslos wie pct, 3.2 *)
                | "unit" unit_name "=" "affine" "(" unit_expr "," number ")" NEWLINE
unit_name      := IDENT | UPPER_IDENT | TYPE_IDENT   (* wie unit_term: bar, mV, V, Hz, KiB (3.2) *)

(* ---------------------------------------------------------------- Konstanten, Parameter, Channels *)

const_decl     := "const" UPPER_IDENT [ ":" type ] "=" const_expr NEWLINE
param_decl     := [ "tunable" ] "param" UPPER_IDENT ":" type "=" const_expr [ "with" attr { "," attr } ] NEWLINE   (* tunable: 8.4, v1.1; with: Metadaten 2.5 *)   (* @check 35 *)
profile_decl   := "profile" UPPER_IDENT ":" NEWLINE INDENT { UPPER_IDENT "=" const_expr NEWLINE } DEDENT

channel_decl   := ( "input" | "output" ) IDENT ":" type "@" binding [ "with" attr { "," attr } ] NEWLINE   (* @check 7, 17 *)
stream_decl    := "stream" "<" elem_type ">" IDENT "with" attr { "," attr } NEWLINE            (* interner Stream, 8.6 *)   (* @check 43 *)
port_decl      := "port" IDENT ":" TYPE_IDENT "@" "mmio" "(" HEX ")" NEWLINE                    (* @stage v1.2 — Treiberstufe *)
binding        := "hw" "(" STRING ")" | "sim" "(" STRING ")" | "none"                       (* STRING nach address_text *)   (* @check 7, 13 *)
attr           := "safe" "=" const_expr | "max_age" "=" duration_lit | "rate" "=" const_expr   (* @check 17, 28, 48 *)
                | "max_rate" "=" const_expr | "capacity" "=" int_lit | "framing" "=" framing
                | "overflow" "=" ( "fault" | "drop_oldest" | "drop" ) | "wake" "=" ( "true" | "false" )
                | "jitter" "=" duration_lit | "max_slew" "=" const_expr | "debounce" "=" int_lit
                | "capacity_bytes" "=" int_lit | "expect_len" "=" int_lit                      (* Byte-Ring, 8.6 *)
                | "irreversible" "=" "true"                                                    (* 12.7 *)
                | "label" "=" STRING | "display" "=" unit_expr | "group" "=" STRING | "doc" "=" STRING   (* Metadaten, 2.5; v1.1 *)
                | "budget" "=" "{" budget_item { "," budget_item } "}"                  (* je Maschine, 7.2 *)   (* @check 62 *)
budget_item    := ( "ram" | "wcet" ) "=" const_expr                                     (* wcet: braucht c_target, 13.8 *)
framing        := "raw" | "lines" | "cobs" | "length_prefixed" "(" IDENT ")" | "fixed" "(" INT ")"
command_decl   := "command" IDENT [ "with" attr { "," attr } ] NEWLINE                          (* wake: 5.10; Metadaten: 2.5 *)

(* ---------------------------------------------------------------- Funktionen, Natives, Bloecke *)

fn_decl        := "fn" IDENT [ generic_vars ] "(" [ params ] ")" [ "->" type ] ":" block   (* ohne Rueckgabetyp nur mit inout-Parameter, 3.9 *)   (* @check 11, 47 *)
native_decl    := "native" ( "fn" | "job" ) IDENT [ generic_vars ] "(" [ params ] ")" "->" type   (* @check 31 *)
                  [ "from" STRING ]                                                          (* Projekt-Native, 4.5; v1.1 *)
                  "with" "cost" "=" cost_spec "," "stack" "=" INT [ "," "duration" "=" duration_lit ] "," "total" NEWLINE   (* 4.5 *)
cost_spec      := int_lit | "{" cost_class ":" int_lit { "," cost_class ":" int_lit } "}"   (* ein Wert zaehlt in i32 (4.5) *)
cost_class     := "i32" | "i64" | "f32" | "f64" | "mem" | "call" | "native"               (* Operationsklassen, 9.4.3 *)
block_decl     := "block" IDENT [ generic_vars ] "(" [ params ] ")" ":" NEWLINE INDENT { var_decl NEWLINE }
                  ( step_decl { method_decl } | method_decl { method_decl } ) DEDENT
step_decl      := "step" "(" [ params ] ")" "->" type ":" block                              (* hoechstens einmal je Instanz und Tick, 5.7 *)   (* @check 11 *)
method_decl    := IDENT "(" [ params ] ")" [ "->" type ] ":" block                           (* weitere Methoden wie start/stop/elapsed, 11.4 *)
generic_vars   := "[" gvar { "," gvar } "]"          (* 3.12: Einheiten- (v1), Konstanten- (v1.1), Typvariablen (v1.2) *)   (* @check 52 *)
gvar           := UPPER_IDENT | "type" UPPER_IDENT [ ":" capability ] | "const" UPPER_IDENT [ "in" range ]   (* @check 52 *)
capability     := "pod" | "eq" | "ord" | "numeric" | "integer" | "float"
params         := param { "," param }
param          := [ "inout" ] IDENT ":" [ "input" | "output" ] type [ "=" const_expr ]      (* inout nur in fn, 3.9; input/output nur in machine *)   (* @check 47 *)

(* ---------------------------------------------------------------- Maschinen *)

machine_decl   := [ "driver" ] "machine" IDENT [ "(" params ")" ] [ "follows" IDENT { "," IDENT } ] [ "node" IDENT ]   (* driver: v1.2; node: 12.9, v2 *)   (* @check 33 *)
                  [ "every" duration_lit ] [ "phase" duration_lit ] [ "with" attr { "," attr } ] ":" NEWLINE INDENT machine_body DEDENT   (* follows: 7.2, v1.1; with: Metadaten 2.5 *)
machine_body   := { var_decl NEWLINE | persist_decl | signal_decl | fault_clause }   (* @check 8 *)   (* Reihenfolge: erst diese vier in beliebiger Folge, dann initial, dann loop/on/state *)
                  "initial" UPPER_IDENT NEWLINE [ loop_block ] { on_handler } { state_decl }
persist_decl   := "persist" "var" IDENT ":" type "=" const_expr [ "with" "min_interval" "=" duration_lit ] NEWLINE   (* @stage v1.1 — 5.9 *)   (* @check 23 *)
signal_decl    := "signal" IDENT NEWLINE
state_decl     := "state" UPPER_IDENT [ "idle" ] [ "resume" ] [ "with" attr { "," attr } ] ":" NEWLINE INDENT state_body DEDENT   (* idle: 5.10, v1.1; resume: 5.12, v1.2; with: Metadaten 2.5 *)   (* @check 8, 22, 54 *)
state_body     := { fault_clause | var_decl NEWLINE | instance_decl } [ "initial" UPPER_IDENT NEWLINE ]   (* instance_decl im Zustand: gescopte Instanz, 5.11 *)   (* @check 25, 53 *)
                  [ enter_block ] [ loop_block ] { on_handler } [ sequence_block ] { transition } [ exit_block ] { state_decl }
fault_clause   := "fault" "->" UPPER_IDENT NEWLINE   (* @check 9 *)
enter_block    := "enter" ":" action_block
exit_block     := "exit" ":" action_block
loop_block     := "loop" ":" block
on_handler     := "on" IDENT [ ( "matches" | "has" ) pattern ] [ "as" IDENT ] ":" block   (* die Bindung ist ein Wrapper: .t, .seq und der Inhalt unter .data bzw. .text; Elementfelder darunter (8.6, 8.7) *)   (* @check 27 *)
transition     := ( "when" guard | "after" duration_expr ) ":" trans_block   (* @check 14 *)
guard          := expr | postfix ( "matches" | "has" ) pattern [ "as" IDENT ] | postfix "as" IDENT   (* letzteres: naechstes Element, 8.7; postfix auch fuer m.fired, cells[i].done. "as" IDENT ist Bindung, ausser IDENT ist ein Skalartypname: dann Cast *)
trans_block    := goto_stmt NEWLINE | NEWLINE INDENT { stmt } goto_stmt NEWLINE DEDENT   (* @check 8 *)

sequence_block := "sequence" ":" NEWLINE INDENT { seq_item } DEDENT
seq_item       := stmt   (* @check 14 *)
                | "wait" duration_expr NEWLINE
                | "until" guard [ "timeout" duration_expr [ "->" UPPER_IDENT ] ] NEWLINE
                | "until" guard "timeout" duration_expr "else" ":" action_block           (* weicher Timeout, 6.2; der Block traegt sein Zeilenende *)
                | "expect" expr [ "," STRING ] NEWLINE
                | "repeat" const_expr ":" NEWLINE INDENT { seq_item } DEDENT
                | "step" STRING ":" NEWLINE INDENT { seq_item } DEDENT

instance_decl  := "instance" IDENT [ "[" IDENT "in" range "]" ] [ "resume" ] "=" IDENT "(" [ args ] ")" NEWLINE   (* resume: 5.11, v1.2 *)   (* @check 53 *)
node_decl      := "node" IDENT "@" "hw" "(" STRING ")" [ "with" "tick" "=" duration_lit ] NEWLINE          (* @stage v2 — 12.9 *)   (* @check 58 *)
property_decl  := "property" IDENT ":" tprop [ "with" "monitor" "=" "true" ] NEWLINE                     (* @stage v1.1 — 13.3 *)   (* @check 56 *)
assumption_decl := "assumption" IDENT ":" tprop [ "with" "monitor" "=" "true" ] NEWLINE                  (* @stage v1.1 — 13.3 *)   (* @check 56 *)
tprop          := tprop_implies                                                                          (* @stage v1.1 — beschraenkte Temporallogik, 13.3 *)
tprop_implies  := tprop_or { "implies" tprop_or }
tprop_or       := tprop_and { "or" tprop_and }
tprop_and      := tprop_not { "and" tprop_not }
tprop_not      := "not" tprop_not | tprop_atom
tprop_atom     := "always" "(" tprop ")" | "never" "(" tprop ")"
                | "eventually" "[" duration_lit "]" "(" tprop ")" | "stable" "[" duration_lit "]" "(" tprop ")"
                | "once" "[" duration_lit "]" "(" tprop ")"
                | "(" tprop ")" | cmp_expr   (* Atom: Vergleich, Musterpruefung oder Wert; not/and/or gehoeren zur Eigenschaft, die Bedingungsform a if c else b steht nur in Argumenten *)
scenario_decl  := "scenario" STRING [ "every" duration_lit ] ":" NEWLINE INDENT machine_body DEDENT      (* @stage v1.1 — 13.6 *)   (* @check 26 *)
campaign_decl  := "campaign" IDENT ":" NEWLINE INDENT { campaign_item } DEDENT                            (* @stage v1.1 — 13.7 *)
campaign_item  := "program" STRING NEWLINE | "profile" UPPER_IDENT NEWLINE   (* @check 29 *)
                | "sweep" UPPER_IDENT "=" ( const_expr ".." const_expr "step" const_expr
                                          | "[" const_expr { "," const_expr } "]" ) NEWLINE
                | "repeat" int_lit NEWLINE | "stop_on" ( "fail" | "never" ) NEWLINE
trigger_decl   := "trigger" IDENT [ "node" IDENT ] ":" NEWLINE INDENT "when" guard NEWLINE "then" at_stmt "bound" duration_lit NEWLINE DEDENT   (* @stage v1.2 — 7.5 *)   (* @check 55 *)

(* ---------------------------------------------------------------- Statements *)

block          := NEWLINE INDENT stmt { stmt } DEDENT | simple_stmt NEWLINE
action_block   := block                       (* statisch eingeschraenkt: siehe 5.5 *)   (* @check 8 *)
stmt           := simple_stmt NEWLINE | if_stmt | for_stmt | match_stmt | at_stmt | every_stmt
simple_stmt    := assign | var_decl | job_stmt | arm_stmt | check_stmt | alert_stmt | log_stmt | goto_stmt
                | abort_stmt | return_stmt | send_stmt | pulse_stmt | cancel_stmt
                | measure_stmt | verify_stmt | verdict_stmt | raise_stmt | "break" | expr | "pass"
assign         := lvalue ( "=" | "+=" | "-=" | "*=" | "/=" ) expr
lvalue         := IDENT { "." member | "[" expr "]" | "[" expr "," expr "]" }              (* Variable, Output, Feld, Element *)
var_decl       := [ "pub" ] "var" IDENT [ ":" type ] "=" expr
if_stmt        := "if" expr ":" block { "elif" expr ":" block } [ "else" ":" block ]
for_stmt       := "for" ( IDENT | "(" IDENT "," IDENT ")" ) "in" ( "range" "(" const_expr ")" | expr ) ":" block   (* expr: Array, Stream, samples, map (3.9) *)   (* @check 11 *)
match_stmt     := "match" expr ":" NEWLINE INDENT { "case" case_pattern ":" block } DEDENT   (* @check 19, 51 *)
case_pattern   := UPPER_IDENT [ "(" IDENT { "," IDENT } ")" ] | "_"
                | const_expr [ ".." const_expr ] { "," const_expr [ ".." const_expr ] }       (* Bereiche und Mehrfachwerte *)
at_stmt        := "at" duration_expr ":" action_block   (* @check 21, 28 *)
every_stmt     := "every" duration_expr ":" block   (* @check 27 *)
check_stmt     := "check" expr [ "," STRING ] [ "for" duration_expr ] [ "within" duration_expr ] [ "->" UPPER_IDENT ] [ "req" STRING ]   (* for: 5.6; within: 9.4.5; req: v1.2 *)   (* @check 9, 36, 61 *)
alert_stmt     := "alert" expr "," STRING [ "for" duration_expr ]   (* @check 36 *)
log_stmt       := "log" STRING                                                               (* Inhalt nach format_text *)
send_stmt      := "send" IDENT "," expr   (* @check 20 *)
pulse_stmt     := "pulse" IDENT "=" expr "for" duration_expr   (* @check 21 *)
cancel_stmt    := "cancel" IDENT
measure_stmt   := "measure" IDENT "=" expr
job_stmt       := "job" IDENT "=" IDENT "(" [ args ] ")"                                     (* 4.5 *)   (* @check 44 *)
arm_stmt       := ( "arm" | "disarm" ) IDENT                                                 (* @stage v1.2 — 7.5 *)   (* @check 55 *)
verify_stmt    := "verify" expr "," STRING [ "req" STRING ]
verdict_stmt   := "verdict" ( "pass" | "fail" ) [ STRING ]
raise_stmt     := "raise" IDENT
goto_stmt      := "->" UPPER_IDENT   (* @check 8 *)
abort_stmt     := "abort" [ STRING ]
return_stmt    := "return" expr

(* ---------------------------------------------------------------- Ausdruecke *)

expr           := or_expr [ "if" or_expr "else" expr ]
or_expr        := and_expr { "or" and_expr }
and_expr       := not_expr { "and" not_expr }
not_expr       := "not" not_expr | cmp_expr
cmp_expr       := bitor_expr [ ( "<" | "<=" | ">" | ">=" | "==" | "!=" ) bitor_expr ]   (* in "<" … ">" eines Typs ist ">" kein Vergleich, sondern schliesst den Typ; ein Vergleich dort steht in Klammern *)
                | bitor_expr ( "matches" | "has" ) pattern [ "as" IDENT ]
bitor_expr     := bitxor_expr { "|" bitxor_expr }
bitxor_expr    := bitand_expr { "^" bitand_expr }
bitand_expr    := shift_expr { "&" shift_expr }
shift_expr     := add_expr { ( "<<" | ">>" ) add_expr }   (* @check 24 *)
add_expr       := mul_expr { ( "+" | "-" ) mul_expr }
mul_expr       := unary { ( "*" | "/" | "%" ) unary }
unary          := "-" unary | "~" unary | cast_expr
cast_expr      := postfix [ "as" scalar_type ]   (* @check 24 *)
postfix        := primary { "." member [ "(" [ args ] ")" ] | "[" expr [ ".." expr ] "]" | "[" expr "," expr "]" }   (* @check 24 *)
member         := IDENT | KEYWORD                    (* nach "." ist jedes Wort ein Membername: .as .or .len .state .step .bits .jitter (2.5) *)   (* @check 50 *)
primary        := number [ unit_lit ] | duration_lit | STRING | "true" | "false" | "none" | "default"
                | IDENT [ generic_args ] [ "(" [ args ] ")" ]                                (* Variable, eingebaute Groesse (now, tick, event), Aufruf, Instanziierung clamp[bar](...): IDENT "[" ... "]" "(" ist Instanziierung, sonst Index in postfix *)
                | "(" expr ")" | "(" expr "," expr ")"                                       (* Gruppe; Stuetzstelle einer table, 3.9 *)
                | UPPER_IDENT [ "(" [ args ] ")" ]
                | TYPE_IDENT [ "(" [ args ] ")" ]                                            (* Konstruktor oder Typ mit Methode: CanFrame.decode(b) *)
                | "[" [ expr { "," expr } ] "]"
                | "[" const_expr "]" IDENT "(" [ args ] ")"                                  (* Array von Block-Instanzen, 5.7 *)
generic_args   := "[" generic_arg { "," generic_arg } "]"
generic_arg    := unit_expr | type | const_expr                                              (* Einheit (v1); Konstante (v1.1); Typ (v1.2), 3.12; Klasse nach Namensform *)
args           := arg { "," arg }
arg            := [ IDENT "=" ] expr
number         := int_lit | FLOAT
int_lit        := INT | HEX | BIN | OCT                 (* ganze Zahl in jeder Schreibweise (3.1); INT allein nur, wo die Form Bedeutung hat: Edition, Exponent, Formatbreite *)
duration_lit   := DURATION                      (* Zahl mit genau einem Zeitsuffix ns us ms s min h d; sonst Einheitenliteral (3.3, lexer.md L4.4) *)
duration_expr  := expr                          (* Typ Duration *)
pattern        := STRING                        (* Musterliteral, Inhalt nach pattern_text (8.7) *)   (* @check 18 *)
                | TYPE_IDENT "(" [ IDENT "=" const_expr { "," IDENT "=" const_expr } ] ")"   (* Record-Muster, 8.7 *)
unit_lit       := unit_expr   (* Einheit nach einer Zahl: ohne Leerraum geschrieben und so weit gelesen, wie die Tokens anliegen (lexer.md L4.3); "5 K/min" ist ein Literal, "5 K / min" eine Division *)
unit_expr      := ( unit_term | "1" "/" unit_term ) { ( "*" | "/" ) unit_term }   (* die "1" (dimensionslos) steht nur als Zaehler: 1/s, 1/X *)
unit_term      := ( IDENT | UPPER_IDENT | TYPE_IDENT ) [ "^" INT ]   (* Einheitenname jeder Form (bar, mV, V, Hz, KiB) oder Einheitenvariable, nie ein kontextuelles Terminal (lexer.md L4.3) *)

(* ---------------------------------------------------------------- Typausdruecke *)

type           := scalar_type [ "in" range ] [ "?" | "!" TYPE_IDENT ] | "[" const_expr "]" type | TYPE_IDENT [ "?" | "!" TYPE_IDENT ]   (* nach "<" schliesst das naechste ">" derselben Klammerebene den Typ, siehe cmp_expr *)   (* @check 30, 45, 57 *)
                | "bytes" "<" const_expr ">" | "vec" "<" type "," const_expr ">" | "line" "<" const_expr ">"
                | "stream" "<" elem_type ">" | "samples" "<" type "," const_expr ">" | "table" "<" type "," type ">"
                | "mat" "<" const_expr "," const_expr ">" [ "[" unit_expr "]" ]                   (* 3.11, uniform *)
                | "mat" "[" unit_tuple "," unit_tuple "]" | "vec" "[" unit_tuple "]"               (* 3.11, dimensioniert; v1.1 *)
                | "map" "<" type "," type "," const_expr ">"                                       (* 3.9, v1.1 *)
                | UPPER_IDENT [ "?" | "!" TYPE_IDENT ]                                             (* Typvariable, 3.12; v1.2 *)
scalar_type    := "bool" | int_type [ "[" unit_expr "]" ]                                        (* Einheiten auf Integern: 3.2; v1.1 *)   (* @check 38 *)
                | "float" [ "[" unit_expr "]" ] | "f32" [ "[" unit_expr "]" ] | "f64" [ "[" unit_expr "]" ]
                | "Duration" | "str" "<" const_expr ">"
int_type       := "int" | "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64"      (* i64 ist gleichbedeutend mit int, 3.1 *)
unit_tuple     := UPPER_IDENT | "1" "/" UPPER_IDENT | "(" unit_expr { "," unit_expr } ")"      (* unitvec-Name, sein Kehrwert, oder Literal *)   (* @check 34 *)
elem_type      := "u8" | "bytes" "<" const_expr ">" | "line" "<" const_expr ">" | "Edge" | TYPE_IDENT
                | "capture" "<" type "," const_expr ">"                                            (* 8.9, v1.2; nur hier, nicht als allgemeiner Typ *)
range          := const_expr ".." const_expr
const_expr     := expr                          (* nur Literale, const, param, eingebaute Konstanten wie tick, fn-Aufrufe darauf (11.3) *)   (* @check 35 *)

(* ---------------------------------------------------------------- Teilsprachen in STRING *)

pattern_text   := { TEXT_CHAR | "{{" | "}}" | "{" IDENT ":" pattern_kind "}" | "{" "_" "}" }   (* @start — Musterliteral, 8.7 *)   (* @check 18 *)
pattern_kind   := "int" | "hex" | "float" | "word" | "str" [ "<" INT ">" ]
format_text    := { TEXT_CHAR | "{{" | "}}" | "{" expr [ ":" format_spec ] "}" }                (* @start — Formatstring, 3.9 *)   (* @check 16 *)
format_spec    := "hex" | "." INT | INT                                                          (* {x:hex} {x:.3} {x:08} *)
address_text   := address_segment { "/" address_segment }                                        (* @start — hw()/sim()-Adresse (8.1, 12.9); Bedeutung: Hardware-Konfiguration 8.10 *)
address_segment := ADDR_WORD [ "[" INT ":" INT "]" ]                                              (* [a:b] halboffen, bindet ein Channel-Array: tc[0:16] sind 16 Kanaele *)
```

### 2.4 Bedeutung der Kernkonstrukte (informell)

| Konstrukt | Bedeutung |
|---|---|
| `const` | Compile-Zeit-Konstante. **Nur auf Dateiebene** (2.3, `file`): In Funktions-, Block- und Zustandsrümpfen gibt es allein `var`. Eine Konstante beschreibt eine Eigenschaft des Programms, nicht einen Zwischenwert einer Berechnung; sie oben zu führen hält sie auffindbar und macht Tabellen wiederverwendbar, statt sie in einer Funktion zu verstecken. Ein `var` mit konstantem Initialisierer im Rumpf ist die gleichwertige lokale Form — die Intervallanalyse kennt seinen Wert genauso (3.4). |
| `param` | Ladezeit-Parameter mit Typ und Range; während eines Laufs unveränderlich; im Lauf-Header aufgezeichnet. `profile` bündelt Belegungen. |
| `input`/`output` | Logische Channels mit Typ, Einheit, Bindung (`hw`, `sim`, `none`), Attributen. Outputs müssen `safe` deklarieren. |
| `command` | Puls-Input, ausgelöst von Operator/Dashboard/Netzwerk; erscheint automatisch in der Bedienoberfläche. |
| `machine` | Hierarchische Zustandsmaschine; parameterlos = Singleton, das automatisch läuft; parametrisiert = Template, das per `instance` instanziiert wird. |
| `state` | Zustand; verschachtelbar; genau ein `initial` je Verschachtelungsebene mit Kindern. |
| `enter`/`exit` | Aktionen beim Betreten/Verlassen (eingeschränkt: Zuweisungen, Outputs, `log`). |
| `loop` | Körper, der in jedem Aktivierungs-Tick ausgeführt wird, solange der Zustand aktiv ist; auf Maschinenebene: in jedem Tick (Interlocks). |
| `when`/`after` | Schwache Transitionen; Block endet zwingend mit `-> ZIEL`. |
| `-> ZIEL` | Übergang; in `loop:` als schwache Transition, in `check` als Fault-Ziel. |
| `check` | Kontinuierlich geprüfte Invariante; Scheitern = Fault (stark, sofort). |
| `expect` | Einmalige Prüfung an einer Stelle einer Sequenz. |
| `alert` | Nicht-abbrechender Monitor; Verletzung wird gemeldet, Flanken protokolliert. |
| `abort` | Systemweiter Fault. |
| `fault -> ZIEL` | Fault-Ziel für den umgebenden Zustand bzw. die Maschine. |
| `sequence` | Schrittfolge mit `wait`, `until`, `expect`, `repeat`, `step`; wird in Zustände übersetzt (6). |
| `fn`/`block` | Reine Funktion / zustandsbehafteter Baustein mit `step`. |
| `pub var` | Variable, die andere Maschinen und die Telemetrie mit Unit-Delay lesen. |
| `record`, `enum` mit Varianten, `match` | Wertetypen fester Größe; `layout` für Drahtformate; `match` ist erschöpfend (3.7). |
| `T?` | Optionalwert mit `.valid`/`.or()`; Channel-Werte sind `T?` mit Alter (3.8). |
| `stream<E>` | Ereignisstrom mit Puffer, Konsumenten-Cursor und Zeitstempeln (8.6). |
| `on` | Handler für Stream-Elemente auf Maschinen- oder Zustandsebene, nur im Run-Modus (8.7). |
| `matches` / `has` | Musterabgleich mit typisierten Captures, auch auf Strings und in `fn` (8.7). |
| `send` | Schreiben in einen Ausgabestrom (8.8). |
| `at` / `pulse` / `cancel` | Geplante Ausgaben zu Hardware-Zeitpunkten (7.5). |
| `measure` / `verify` / `verdict` | Beobachtung für Testergebnisse; nie ein Fault (13.5). |
| `scenario` / `campaign` | Simulationsszenarien und Parameter-Sweeps (13.6, 13.7; v1.1). |
| `persist var` | Variable, die Neustarts überlebt; wählt nur den Anfangszustand (5.9; v1.1). |
| `idle` | Schlafzustand mit Wake-Quellen (5.10; v1.1). |
| `signal` / `raise` | Veröffentlichter Puls zwischen Maschinen (5.8). |
| `every d:` | Periodische Teilaktion innerhalb von `loop:` (5.8). |
| `trigger` | Hardware-nahe Reaktionsregel auf einem I/O-Knoten (7.5; v1.2). |
| `mat<R, C>`, `mat[R, C]`, `vec[R]`, `unitvec` | Matrizen fester Größe; dimensionierte Form mit Einheitentupeln (3.11). |
| interner `stream<E> name` | Warteschlange zwischen Maschinen mit Stream-Semantik; ein Schreiber, Sichtbarkeit ab dem nächsten Tick (8.6). |
| `job` | Asynchroner Native-Aufruf; Fertigstellung und Ergebnis sind Inputs (4.5). |
| `T!E` | Ergebnis mit Fehlercode; `.ok`, `.err`, `.or()`, `match OK/ERR` (3.8). |
| `default` | Standardwert eines POD-Typs (3.7). |
| `inout` | In-place-Parameter reiner Funktionen; Zucker für Rückgabe (3.9). |
| `irreversible` | Output ohne Rückweg (z. B. einmalig programmierbare Bits); verlangt `expect` davor und Szenario-Abdeckung (12.7). |
| `driver machine`, `port` | Treiberstufe mit sofortigen Registerzugriffen (15; v1.2). |
| `tunable param` | Zur Laufzeit änderbarer Parameter mit Halte-Semantik, Range erzwungen, aufgezeichnet (8.4; v1.1). |
| `follows` | Explizite Vorrangbeziehung zwischen Maschinen; frische Lesevorgänge im selben Tick (7.2; v1.1). |
| `debounce`, `Suspect` | Entprellte Qualität mit beschränkter Haltedauer (3.5). |
| `check … for d`, `alert … for d` | Bestätigungszeit: löst erst nach d ununterbrochener Verletzung aus (5.6). |
| `reader`, `writer` | Cursor-Bausteine über `bytes<N>` mit `T?`/`bool`-Ergebnissen (3.9, 11.4). |
| `len_field` | Längenpräfixierte Felder in `layout` mit statischer Obergrenze (3.7; v1.1). |
| System-Channels `sys/…` | Plattformschnittstelle: Boot-Grund, Image-Bestätigung, Neustart (12.7; v1.1). |
| `native fn` | Kuratierte native Funktion mit Kostenvertrag `cost`/`total` (4.5). |
| `tick_source` | Bindung des Basis-Ticks an ein Hardware-Ereignis (7.1). |
| `jitter`, `max_slew` | Präzision eines Outputs bzw. Plausibilitätsgrenze eines Inputs (3.5, 7.5). |


### 2.5 Editionen, reservierte Namen und Erweiterbarkeit

**Edition.** `system: language = 1` nennt die Sprachedition des Programms. Brechende Änderungen (neue Schlüsselwörter, geänderte Defaults, engere Regeln) kommen ausschließlich mit einer neuen Edition; der Compiler beherrscht alle Editionen, die er kennt, und der Logik-Hash enthält die Edition. Editionen sind eine Eigenschaft des Frontends: importierte Module tragen ihre eigene Edition und werden auf MIR-Ebene zusammengeführt. Fehlt die Angabe, gilt die neueste dem Compiler bekannte Edition mit Warnung (`takt fmt` trägt sie ein); im Zertifizierungsmodus ist das Fehlen ein Fehler. `takt migrate --edition N` schreibt mechanisch übersetzbare Änderungen um und meldet den Rest mit Position. Vordefinierte Einheiten (3.2), reservierte Wörter, reservierte Membernamen und die Standardbibliothek sind je Edition versioniert.

**Reservierte Wörter.** Neben den Schlüsselwörtern (2.2) sind Wörter reserviert, die heute keine Bedeutung haben, als Bezeichner aber verboten sind (Liste in 2.2). `while` erhält eine eigene Meldung („nicht erlaubt: `for` mit Schranke oder `sequence` mit `until`"). Neue Wörter kommen nur mit einer Edition.

**Reservierte Membernamen.** Die eingebauten Zugriffe — `valid suspect stale age reason or ok err t seq text data len count dropped malformed overflowed free jitter time_warped done result state to to_float as bit bits with_bit wrap_* min max mean rms last transpose inv det solve cholesky decode encode default push append get insert remove clear skip starts_with contains armed fired pre post samples rate remaining truncated reset` — gehören zu den eingebauten Typen (Wrapper, Channels, Streams, Sammlungen, Blöcke, Matrizen, Captures). Records und Enums haben einen eigenen Namensraum: Verboten als Feldnamen sind nur die Zugriffe der Wrapper `valid suspect stale age reason or ok err`, weil auf einem Wrapper (`T?`, `T!E`, Channel, Job-Handle, Trigger-Handle) `x.name` immer den Wrapper meint und Felder des Inhalts erst nach dem Auspacken (Dominanz, 3.8) erreichbar sind. Alle anderen Namen der Liste dürfen Records tragen (`CanFrame.data`, 3.7); Variantennamen sind frei, auch `NONE`, `OK` und `ERR`, weil `match` typgeführt ist. Capture-Namen in Mustern (8.7) sind Feldnamen der Bindung, die daneben `t`, `seq` und den Inhalt `data` bzw. `text` trägt; diese vier sind als Capture-Namen verboten. Für *Record*felder gilt das Verbot nicht, weil die Bindung ein Wrapper ist: Ein Element mit eigenem `seq` bleibt unter `m.data.seq` lesbar. Schlüsselwörter sind als Feldnamen verboten — und ebenso als Variablen-, Parameter- und Blocknamen: Ein Schlüsselwort ist im gesamten Programm kein Bezeichner. Praktisch stolpert man dabei fast immer über dieselben kurzen Wörter, die zugleich naheliegende Bezeichner sind; die Meldung nennt das Wort, die Ausweichnamen sind Geschmackssache:

| Schlüsselwort | typischer Wunsch | bewährter Ausweichname |
|---|---|---|
| `on` | Schaltzustand einer Lampe, eines Relais | `lit`, `active`, `energized` |
| `state` | Feld eines Statusregisters, Phase eines Protokolls | `phase`, `stage`, `status` |
| `unit` | Messgröße eines Sensors (E1.20, IO-Link) | `messgroesse`, `quantity`, `uom` |
| `step` | Schritt einer Sequenz als Datenfeld | `stage`, `index`, `no` |
| `at` | Zeitstempel als Feldname | `t`, `when_ns`, `stamp` |

Neue eingebaute Zugriffe kommen nur mit einer Edition.

**Offene Enums.** `FaultKind`, `BootReason`, `ImageState`, `RebootCmd`, `Quality`, der Wertebereich von `x.reason` und `JobErr` sind *offen*: `match` über sie verlangt `case _`, damit neue Varianten (z. B. `Runtime(Node)`, 12.9) keine erschöpfenden Matches brechen. Nutzer dürfen eigene Enums mit `enum Msg open: …` als offen deklarieren (Nachrichtentypen, die über Firmware-Versionen wachsen); geschlossene Enums bleiben erschöpfend prüfbar (3.7).

**Verdeckung und Additivität.** Nutzerdefinitionen dürfen Namen der Standardbibliothek verdecken (Warnung) — neue Bibliothekseinträge brechen nie ein Programm. Attribute in `with` und Einträge in `system:` sind additiv: unbekannte Namen sind heute ein Fehler, neue Namen morgen keine Änderung bestehender Programme.

**Operator-Metadaten (v1.1).** Die Attribute `label = "…"`, `display = <Einheit gleicher Dimension>`, `group = "…"` und `doc = "…"` sind an Channels, Params, Tunables, Commands, Maschinen und Zuständen erlaubt. Sie sind reine Beobachtung: kein Einfluss auf Semantik, Budget oder Logik-Hash (wohl auf den Hash des Programms); Bedienoberfläche und Report lesen sie.

---

## 3. Typsystem

### 3.1 Grundtypen
- `bool`.
- `int` = i64 (Default; `i64` ist der ausgeschriebene, gleichbedeutende Name); schmale und unsignierte Typen `i8 i16 i32 u8 u16 u32 u64` für Registerwerte, Prüfsummen und Treiberschnittstellen; alle Integer-Typen haben targetunabhängige Semantik. Literale dezimal, `0x`, `0b`, `0o`, mit `_`-Trennzeichen. Die *Darstellung* eines `int` wählt der Compiler aus den bewiesenen Intervallen (32 Bit, wo möglich; Lemma 3.4) — ohne Änderung der Semantik.
- `float` = IEEE-754-Fließkommazahl, eingeschränkt auf endliche Werte; ihre Breite legt das Programm einmal fest (`system: float = f32 | f64`, Default f64, 4.2) — nicht das Target. `f32` und `f64` sind explizite Breiten für gemischte Fälle.
- `Duration`: i64 Nanosekunden; Literale mit `ns us ms s min h d` (exakte ganzzahlige Faktoren; `1.5 s` = 1 500 000 000 ns exakt; nicht darstellbare Literale wie `0.1 ns` sind Fehler). Der Wertebereich reicht für 292 Jahre.
- `enum`: benannte Varianten; Outputs mit Enum-Typ werden über die Hardware-Konfiguration auf Rohwerte abgebildet.
- `[N] T`: Array fester Länge N (Compile-Zeit-Konstante); Index vom Typ `int in 0..N-1` (siehe 3.4). **Ein Index trägt keine Einheit**: Der Typ ist strukturell `int in 0..N-1`, ein `int[slot]` ist dort nicht zulässig. Wer mehrere Nummernräume führt (1-basierte Slot-Nummer, 0-basierter Pufferoffset, Kanalindex), trennt sie mit eigenen Range-Typen und benannten Umrechnungsfunktionen, nicht mit Einheiten (3.2).
- `str<N>`: String mit fester Kapazität; nur Literale und Formatierung (`"{p}"`) mit definierter Trunkierung.
- Records und Summentypen (`enum` mit Varianten), erschöpfendes `match`: 3.7. Optionaltyp `T?`: 3.8.
- `bytes<N>`, `vec<T, N>`, `line<N>`, `table<A, B>`: 3.9. Bit-Operationen und Konversionen: 3.10.
- `mat<R, C>`, `mat<R, C>[U]` und dimensionierte `mat[R, C]`/`vec[R]`: 3.11.
- Einheiten sind auf allen numerischen Skalartypen erlaubt, auch auf Integern (`int[mV]`, `u16[raw]`): 3.2 — mit Ausnahme der Indexposition, siehe oben.
- `map<K, V, N>` (3.9) und `capture<T, N>` (8.9, nur als Stream-Element).
- `stream<E>` (8.6), `samples<T, N>` (8.9) und das eingebaute Record `Edge` (`rising: bool`) sind Channel-Typen; sie kommen nur in Channel-Deklarationen und Maschinen vor.

### 3.2 Physikalische Einheiten
**Modell.** Dimensionen bilden die freie abelsche Gruppe ℤ⁷ über den SI-Basisdimensionen (m, kg, s, A, K, mol, cd) plus Zähldimension 1. Eine *Einheit* ist ein Name mit Dimension und rationalem Skalierungsfaktor zur Basiseinheit (`unit psi = 6894.757293168 Pa` — Faktoren dürfen Floats sein, werden aber zur Compile-Zeit exakt als Rational gespeichert, wo möglich).

**Nominale Semantik (Entscheidung).** `float[bar]` und `float[psi]` sind verschiedene Typen. Es gibt keine implizite Konversion; `p.to(psi)` konvertiert explizit. Multiplikation und Division kombinieren Einheiten (`bar * s`, `K/min`); Addition, Subtraktion und Vergleich verlangen gleiche Einheiten; Potenzen mit ganzzahligem Exponenten erlaubt. Skalare (`float` ohne Einheit) skalieren jede Einheit.

Warum nominal statt automatisch konvertiert: Automatische Konversion fügt unsichtbare Multiplikationen ein (Präzision, Determinismus-Nachweis) und verleitet dazu, Einheitenfehler als Rundungsrauschen zu übersehen. Ein Techniker sieht bei `p_bar.to(psi)` genau, was passiert.

**Vordefinierte Einheiten.** SI-Basis- und abgeleitete Einheiten mit SI-Präfixen (`mV`, `uA`, `kHz`, `mohm`, `um`), dazu `bar`, `psi`, `Hz`, `ohm`, `pct` (dimensionslos, Faktor 0.01), `degC`, `degF` (affin), `Ah`, `Wh` (mit Präfixen), sowie `B` (Byte, dimensionslose Zählgröße) mit binären Präfixen `KiB`, `MiB`. Adressen und Größen tragen `B` (`u32[B]`), projektspezifisch `unit sector = 4 KiB` — die Verwechslung von Sektornummern und Byteadressen ist damit ein Typfehler, und `(n * (1 sector)).to(B)` ist exakt (ganzzahliger Faktor). Das gilt für *Werte*; sobald eine solche Größe als **Array-Index** dient, verliert sie ihre Einheit, weil der Index strukturell `int in 0..N-1` ist (3.1). Ein Puffer, der über mehrere Nummernräume adressiert wird, bekommt seine Sicherheit deshalb aus Range-Typen und benannten Umrechnungen, nicht aus Einheiten. `to(...)` konvertiert zwischen Einheiten gleicher Dimension (Vergleich der Exponentenvektoren), also auch `V/A` nach `mohm`. Eigene Einheiten per `unit`; eine Neudefinition einer vordefinierten Einheit ist ein Fehler.

**Entdimensionieren.** Den Rohwert einer dimensionierten Größe liefert die Division durch ein Einheitenliteral: `round(x / (1 inc/s)) as u32` schreibt einen Zählwert auf den Bus. Das ist nicht der Behelf, sondern *das* vorgesehene Idiom — die Division nennt den Umrechnungsfaktor explizit und sagt damit, in welcher Einheit der Rohwert gilt. Eine benannte Kurzform (`x.raw(U)`) gibt es bewusst nicht: Sie spart vier Zeichen und stellt daneben eine zweite Schreibweise für dieselbe Operation, die jeder Leser als solche erkennen müsste.

**Einheiten auf Integern (v1.1).** `int[mV]`, `i32[mbar]`, `u16[raw]` folgen denselben Regeln: nominal, `*`/`/` kombinieren, `+`/`-`/Vergleich verlangen Gleichheit, Skalare skalieren. `x.to(U2)` ist nur erlaubt, wenn der Konversionsfaktor ganzzahlig ist (`mV -> uV`: ×1000); sonst `x.to_float(U2)` (exakt bis 2^53 bei f64, 2^24 bei f32). Division trunkiert wie 4.1, Overflow ist ein Fault wie 4.1. Damit rechnet ein Regler auf einem Kern ohne FPU einheitensicher und schnell, ohne einen zweiten Zahlentyp (Festkomma) einzuführen; die Skalierung steckt in der Wahl der Einheit.

**Unit-Polymorphie in Funktionen und Blöcken.** `fn clamp[U](x: float[U], lo: float[U], hi: float[U]) -> float[U]`: `U` ist eine in eckigen Klammern deklarierte Einheitenvariable (explizit, damit sie nie mit konkreten Einheiten wie `K` kollidiert; Klammerklassen für Typ- und Konstantenvariablen: 3.12); Inferenz per Unifikation in abelschen Gruppen (Kennedy 1994/1997) — entscheidbar, mit Hauptlösung. Das genügt für die gesamte Standardbibliothek (min, max, abs, clamp, lerp, Filter, PID mit `K[U/V]`-Verstärkungen).

**Affine Einheiten.** `degC` und `degF` sind affine Einheiten über `K`: Werte sind Punkte, Differenzen sind Vektoren.

| Operation | Ergebnis |
|---|---|
| `degC − degC` | `K` |
| `degC ± K` | `degC` |
| `K ± K`, `K · Skalar` | `K` |
| `degC + degC`, `degC · Skalar` | Typfehler mit Erklärung |
| `degC < degC` | erlaubt |
| `x.to(K)` für `degC` | absolute Konversion (+273.15) |

Deltas werden in `K` geschrieben (`PEAK_TEMP - 3 K`), was für Ingenieure lesbar bleibt, weil Kelvin- und Celsius-Differenzen numerisch identisch sind.

### 3.3 Duration und Zeitrechnung
`Duration` ist kein Einheitentyp, sondern ein Integer-Typ: `Duration ± Duration`, `Duration · int`, `Duration / int` (Trunkierung), `Duration / Duration → int`, Vergleiche. Übergang in die Float-Welt explizit: `d.as(s)` → `float[s]`, `d.as(min)` → `float[min]`. Damit sind Rampen exakt formulierbar: `RAMP_RATE * time_in_state.as(min)` → `K`.

Lesbare Zeitgrößen in Maschinen: `now` (Duration seit Start), `time_in_state` (Duration seit Eintritt in den aktuellen Blattzustand), `tick` (T₀).

**Lexikalische Regel.** Eine Zahl, auf die genau ein Zeit-Suffix folgt (`3 s`, `200 ms`, `7 d`), ist ein `Duration`-Literal. Folgt ein zusammengesetzter Einheitenausdruck (`5 K/min`, `9.81 m/s^2`, `0.0005 1/s`), ist es ein Float-Literal mit Einheit. `s`, `min`, `h` sind in `float[…]` und in `.as(…)` gewöhnliche Einheitennamen. Ein Fließkommaliteral in einer reinen Zeiteinheit gibt es deshalb nicht; ein `float[s]` entsteht aus einer Dauer per `(10 ms).as(s)`. Zwischen Zahl und Einheit steht ein Leerzeichen, und der Einheitenausdruck selbst ist ohne Leerraum geschrieben (`K/min`, `m/s^2`), sodass `200 ms * 2` eine Dauer mal zwei ist und `3 s/m` ein Einheitenliteral (Lexer-Spezifikation `grammar/lexer.md`).

### 3.4 Range-Typen und Intervallanalyse
`int in 0..N-1`, `float[bar] in 0..400 bar`. Der Compiler führt eine Intervallabstraktion über alle Ausdrücke:

- Literale, Konstanten, Params (deklarierte Range) und Channels (deklarierte Range) liefern Startintervalle.
- In einer Range `lo..hi U` mit einheitenlosem Literal `lo` gilt die Einheit `U` für beide Grenzen (`0..100 bar`, `-60..200 degC`, `2.0..4.5 V`); ein einheitenloses Literal als Obergrenze oder verschiedene Einheiten an den Grenzen sind Fehler. Das ist die einzige Ausnahme von 3.6.
- Arithmetik propagiert Intervalle (Standard-Intervallarithmetik; Division mit Nullausschluss).
- Zuweisung an eine Variable mit Range: Ist das Intervall des Ausdrucks enthalten → keine Prüfung. Sonst fügt der Compiler einen impliziten Check ein (Scheitern → `RangeFault`) **und warnt** mit Vorschlag (`clamp`, weitere Range, `check` davor).
- Array-Index: Typ `int in 0..N-1` verlangt; ein Index aus `range(N)` erfüllt das trivial; sonst implizite Prüfung mit Warnung.
- Vergleiche verfeinern Intervalle in Zweigen (`if x < 10:` → im Zweig x ∈ (−∞,10)), analog zu Refinement-Typen light.

Die Analyse ist absichtlich einfach (Intervalle, keine Relationen zwischen Variablen): vorhersagbar, schnell, und ihre Grenzen sind für Techniker erklärbar. Eine Relation wie `dt >= tick` zwischen zwei Größen kann sie nicht herleiten; solche Fakten werden über Parameter-Ranges eingebracht (`dt: Duration in tick..1 h`). Bit-Operationen propagieren Breitenschranken (3.10); Indizes in `vec` und Slices sind nur über ihre Range-Deklaration beweisbar (3.9).

**Ranges sind Tick-Rand-Invarianten.** Eine Variable mit deklarierter Range hat diese Range an jedem Tick-Anfang (der Compiler prüft jede Zuweisung, 3.4); Channel- und Param-Ranges werden am Treiberrand erzwungen (3.5, 12.6) bzw. zur Ladezeit validiert (8.4). Die Analyse eines Ticks ist deshalb lokal — Straight-Line-Code plus beschränkte Schleifen — und braucht keine Fixpunktbildung über Ticks hinweg. `for`-Schleifen mit n·|Körper| unterhalb einer Schwelle werden abgerollt analysiert, sonst mit Widening auf die deklarierten Ranges der beschriebenen Variablen; Akkumulatoren ohne Range erhalten implizite Prüfungen mit Warnung. Die Zahl impliziter Prüfungen ist eine Kennzahl im Report, aufgeschlüsselt nach ihrer Ursache — `Declared` (Zuweisung in eine engere Range), `Index` (Array-, `vec`- oder Slice-Zugriff), `Convert` (`as`-Konversion) und `Arith` (Division, Overflow, Shift). Die Summe allein führte in die Irre: Ein Protokollparser sammelt zwangsläufig viele `Index`-Prüfungen, die alle aus derselben fehlenden Schranke folgen, während eine einzige `Declared`-Prüfung in einer Regelschleife mehr wiegt. Die Aufschlüsselung sagt dem Techniker, *welche* Schranke fehlt, nicht nur *dass* eine fehlt. Zertifizierungsprojekte eskalieren die Kennzahl zu Fehlern (10).

**Warnpolitik und Idiom.** Implizite Prüfungen sind Informationen, die pro Datei aggregiert werden; Warnungen im engeren Sinn entstehen nur bei impliziten Prüfungen in `for`-Schleifen und in Aktionsblöcken. Wer Relationen zwischen Größen braucht, benutzt eine **range-typisierte Zwischengröße**: `var dp : float[bar] in 0..50 bar = p_out - p_in` erzwingt per implizitem Check eine bekannte Range für alles Folgende — ein bewusst platzierter, sichtbarer Check statt vieler verstreuter. Ein `assume` gibt es nicht; es wäre unsound.

**Oktagone (v1.1).** Die Analyse wird auf die Oktagon-Domäne (Miné) erweitert: Invarianten der Form `±x ± y <= c` in polynomieller Zeit — genau die Relationen `a < b` und `a - b <= c`, die Steuercode braucht (Differenzen korrelierter Sensoren, Abstände zu Sollwerten). Die Schnittstelle der Analyse (implizite Prüfungen, Warnungen, Kennzahl) bleibt unverändert; nur die Präzision steigt. Ein SMT-Solver bleibt ausgeschlossen (Vorhersagbarkeit, Übersetzungszeit).

**Darstellungsverengung.** Auf 32-Bit-Kernen kostet ein i64 zwei Register: Addition und Vergleich das Doppelte, Multiplikation das Vier- bis Fünffache, Division einen Bibliotheksaufruf. Der Compiler wertet deshalb jeden Ausdruck in der schmalsten Breite aus, deren Wertebereich die bewiesenen Intervalle *aller* Teilausdrücke enthält, und speichert Variablen mit deklarierter Range in der schmalsten passenden Breite. Das ist eine reine Optimierung:

*Lemma (Darstellungsverengung).* Sei e ein Ausdruck vom Typ `int` und I(e') das bewiesene Intervall jedes Teilausdrucks e' von e. Gilt I(e') ⊆ [−2³¹, 2³¹ − 1] für alle e', dann liefert die Auswertung aller Operationen von e im 32-Bit-Zweierkomplement mit Overflow-Erkennung dasselbe Ergebnis — Wert oder Fault — wie die Auswertung in 64 Bit.
*Beweis.* Induktion über den Aufbau von e. Jede Operation hat ein exaktes Ergebnis in I(e') ⊆ i32; also tritt in 32 Bit kein Overflow auf, und der 32-Bit-Wert gleicht dem 64-Bit-Wert. Ein Fault kann in beiden Darstellungen nur durch eine Operation entstehen, deren exaktes Ergebnis außerhalb des jeweiligen Bereichs liegt; das ist durch I(e') ⊆ i32 ⊆ i64 für beide ausgeschlossen. Division und Modulo sind bei gleichen Operanden identisch; Division durch null faultet in beiden Fällen. Der Range-Check der Zuweisung sieht in beiden Fällen denselben Wert. ∎

Teilausdrücke ohne bewiesenes Intervall — und alles, was von ihnen abhängt — bleiben in 64 Bit; gemischte Breiten in einem Ausdruck sind erlaubt. Schleifenindizes aus `range(n)`, Array-Indizes, Zähler mit Range, Literale und Channel-Werte mit Range fallen praktisch immer in 32 Bit. Explizite `i32`/`u32` bleiben eine *semantische* Wahl (Fault an der schmalen Grenze). Ein Performance-Lint (10) meldet `int` ohne Range in Schleifen und Regelpfaden auf 32-Bit-Zielen mit dem Kostenanteil (9.4.3). Satz 9.4.4 bleibt unberührt, weil die Verengung das Ergebnis nach dem Lemma nicht ändert.

### 3.5 Qualität von Input-Channels
Jeder Input trägt pro Tick Qualität ∈ {Good, Suspect, Stale, Bad} und Alter. Stale ⇔ Alter > `max_age`; ohne Angabe ist der Default das Doppelte der kürzesten Periode unter den Maschinen, die den Channel lesen — die Zusicherung „frisch genug" gilt damit für jeden Leser, auch den schnellsten. Ein Channel, den niemand liest, altert nicht. Die Regel gilt unabhängig von der bisherigen Qualität: ein entprellter Kanal (`Suspect`), dessen Treiber ausfällt, wird ebenfalls `Stale`; nur `Bad` bleibt `Bad`. Das Alter einer soeben abgetasteten Größe ist das ihres Treibers, für eine frische Lieferung also null (9.4: `I_k = sample()` steht am Tick-Anfang). `Suspect` entsteht nur durch `debounce` (unten) und gilt für `.valid` als gültig.

**Implizite Validitätsprüfung (Entscheidung).** Wird ein Input-Channel `x` in einem Ausdruck verwendet, gilt an dieser Stelle implizit `check x.valid` (Scheitern → `SensorFault(x)`), **außer** die Verwendung liegt in einem Zweig, der statisch von `x.valid` dominiert wird, oder sie benutzt `x.or(default)`. Der Compiler bestimmt Dominanz durch dieselbe Flussanalyse wie Definite Assignment.

```
loop:
    check tank_p < LIMIT  # implizit: check tank_p.valid
    if lox_temp.valid:
        alert lox_temp > 100 K, "LOX warm"  # explizit abgesichert
    var t = lox_temp.or(90 K)  # Fallback, keine Prüfung
```
Ergebnis: Ohne Mehrarbeit ist der sichere Fall der Default („expected state: sensor valid"); wer Sensorausfälle tolerieren will, sagt es ausdrücklich. Verfügbar: `x.valid`, `x.suspect`, `x.stale`, `x.age`, `x.reason`, `x.or(v)`.

Diese Zugriffe gelten für **jeden** Channel-Typ, nicht nur für Fließkomma: der Wert eines Inputs ist immer `T?` (3.8), unabhängig davon, ob `T` ein Float, ein Integer, ein `bool`, ein Enum oder ein Record ist. `mode.or(MANUAL)` auf einem Enum-Channel und `ready.or(false)` auf einem Bool-Channel sind so gewöhnlich wie `tank_p.or(0 bar)`; der Default-Ausdruck hat den Typ des Channels.

Einzige Ausnahme sind die Beobachtungs-Statements `alert`, `log`, `measure`, `verify`, `verdict`: Dort erzeugt ein ungültiger Channel keinen Fault — der Alert feuert mit dem Zusatz „sensor invalid", `log`/`measure` schreiben `<invalid>`, `verify` zählt den Wert als Verletzung. Beobachtung ist keine Steuerung und darf die Steuerung nie beeinflussen (Entscheidung 14).

Channel-Werte sind damit Optionalwerte `T?` (3.8) mit zusätzlichem Alter; es gibt in der Sprache genau ein Modell für „vielleicht nicht da".

**Rand-Durchsetzung.** Der Treiberrand (12.6) prüft jede Lieferung gegen die deklarierte Range des Channels: Werte außerhalb werden `Bad` mit Grund `OutOfRange`, nicht geklemmt und nicht zu Faults — der implizite Validitäts-Check greift, und das Programm kann mit `.valid`/`.or()` reagieren. Mit `with max_slew = 50 bar/s` wird ein Wert `Bad` mit Grund `Implausible`, wenn seine Änderungsrate gegenüber dem letzten guten Wert die Grenze überschreitet (der erste Wert nach Start oder nach `Bad` gilt als gut). Der Grund ist als `x.reason` in `{Stale, OutOfRange, Implausible, Driver}` lesbar. Ohne diese Durchsetzung wären die deklarierten Ranges keine gültigen Annahmen der Intervallanalyse (3.4).

**Entprellte Qualität.** `with debounce = 3`: Ein Wert außerhalb der Range oder jenseits `max_slew` wird für bis zu drei aufeinanderfolgende Lieferungen als `Suspect` geführt — der letzte gute Wert wird gehalten, `x.suspect` ist lesbar, `.valid` bleibt `true` —, erst danach `Bad` mit dem jeweiligen Grund. Die Haltedauer ist beschränkt (`debounce · Lieferperiode`) und im Programm sichtbar; ein Sicherheitsargument kann sie einrechnen. Ohne `debounce` ist eine Verletzung sofort `Bad` (Entscheidung 18). `Driver` als Grund bedeutet, dass der Treiber selbst degradiert ist (12.6): alle seine Channels sind `Bad`, bis er wieder vertragsgemäß liefert.

### 3.6 Typinferenz
Lokale Inferenz (Hindley-Milner-artig für Einheitenvariablen, sonst bidirektional): Variablen erhalten den Typ ihres Initialisierers; Funktions- und Block-Signaturen sind annotiert (Dokumentationswert für Techniker); Literale ohne Einheit in Einheitenkontext sind Fehler (`p < 300` bei `p: float[bar]` → „meinst du `300 bar`?"). Einzige Ausnahme ist die Untergrenze einer Range (3.4).


### 3.7 Records und Summentypen
```
record CanFrame layout little:
    id   : u32 in 0..0x1FFFFFFF
    dlc  : u8 in 0..8
    data : bytes<8>

record BootStatus:
    version        : int
    sectors_erased : int in 0..255

enum BootMsg:
    BOOT(version: int)
    ERASE(sector: int in 0..255)
    WRITE(addr: int, len: int in 0..4096)
    OTHER
```
- Records sind Wertetypen fester Größe; Zuweisung kopiert; Konstruktion `CanFrame(id = 0x7E0, dlc = 8, data = payload)`, Feldzugriff `f.id`.
- `layout little | big` definiert das Drahtformat; dann existieren `CanFrame.decode(b: bytes<M>) -> CanFrame?` (`none` bei zu kurzem Puffer oder Range-Verletzung eines Feldes) und `f.encode() -> bytes<SIZE>`. Ohne `layout` gibt es keine Byte-Repräsentation.
- Längenpräfixierte Felder (v1.1): `value: bytes<64> with len = len_field` dekodiert ein Feld, dessen Länge in einem vorangehenden Feld steht, mit statischer Obergrenze 64; `decode` liefert `none`, wenn `len_field` die Obergrenze überschreitet. Damit sind TLV-Pakete und Diagnosedaten ohne dynamischen Speicher modellierbar (Cursor-Bausteine: 3.9).
- **Drahtformat-Details** (für Header, Partitionstabellen, Protokollframes und die Übersetzung von C-Structs, 13.9):
  ```
  enum ImageType layout u8: APP = 0x00, DATA = 0x01, BOOT = 0x02  # explizite Diskriminanten mit Breite
  record PartitionEntry layout little, align = 4:
      magic   : u16 = 0x50AA  # Konstantenfeld: decode prueft, encode setzt
      kind    : ImageType
      subtype : u8
      offset  : u32[B]  # Byte-Adresse als Einheit (3.2)
      size    : u32[B]
      label   : [16] u8         # Array fester Laenge
      flags   : u32 with bits:  # Bitfelder mit Positionen
          encrypted : bool at 0
          readonly  : bool at 1
          level     : u8 at 4..7
      _ : [4] u8  # Padding, ignoriert
  ```
  **Zugriff auf Bitfelder.** Ein benanntes Bitfeld ist eine *Sicht* auf sein Trägerfeld, kein eigener Speicher: `p.flags.encrypted` liest es mit dem deklarierten Typ (`bool` bei einer einzelnen Position, sonst der Integer-Typ), `p.flags.level = 5` schreibt genau seine Bits und lässt die übrigen des Trägers unverändert. Beides senkt auf die Bitoperatoren aus 3.10 (`bit`/`bits` beziehungsweise `with_bit` und Maske), kostet also nichts Zusätzliches. Der Träger bleibt daneben als Ganzes lesbar und schreibbar (`p.flags`), und weil der Codec ihn serialisiert, überstehen die Bitfelder `encode`/`decode` ohne eigenes Zutun. Zusammengesetzte Zuweisungen (`+=`) sind auf einem Bitfeld nicht erlaubt — der Rechenschritt gehört sichtbar hin.

Regeln: verschachtelte Records und Arrays fester Länge; `offset = N` für absolute Positionen (sonst fortlaufend); `align` rundet die Gesamtlänge; Bitfelder liegen innerhalb der Breite ihres Trägerfelds und überlappen nicht (statisch geprüft); Konstantenfelder werden bei `decode` geprüft und bei `encode` gesetzt; die Länge eines Records ist statisch. `decode` liefert `none` bei Konstanten-, Range- oder Längenverstoß.
- **`default`.** Jeder POD-Typ hat einen Standardwert: 0 bzw. `0 U`, `false`, die erste Variante, leere `vec`/`bytes`, Records feldweise; `var img : ImageHeader = default` ist damit ohne Literal möglich.
- **Records als Konstanten, Arrays von Records als Gerätetabellen.** Ein Record-Konstruktor mit konstanten Argumenten ist ein `const_expr` (2.3). Statische Gerätebeschreibungen — Sensordefinitionen, PID-Tabellen, Personality-Listen, Registerkarten — stehen deshalb als ein Datenblock da und werden indiziert, statt als Kette von Bedingungen in den Code zu wandern:
  ```
  record SensorDef:
      kind        : u8
      messgroesse : u8
      range_min   : i16
      range_max   : i16

  const SENSORS : [2] SensorDef = [
      SensorDef(kind = 0x00, messgroesse = 0x01, range_min = -400, range_max = 1250),
      SensorDef(kind = 0x01, messgroesse = 0x05, range_min = 0, range_max = 300)]

  fn sensor_kind(idx: int in 0..1) -> u8:
      return SENSORS[idx].kind
  ```
  Das ist der vorgesehene Weg für jede Tabelle, deren Zeilen zur Übersetzungszeit feststehen. Er ersetzt Ketten wie `0x00 if n == 0 else 0x01` in jedem Feld eines Konstruktors: Die Tabelle bleibt als Tabelle lesbar, und ein zusätzlicher Eintrag ist eine Zeile statt eines Eingriffs an mehreren Stellen. Der Index ist wie bei jedem Array `int in 0..N-1` (3.1); für Stützstellen mit Interpolation gibt es stattdessen `table<A, B>` (3.9).
- Varianten mit Feldern werden mit `BOOT(version = 3)` oder positional `BOOT(3)` gebaut; Range-Felder werden bei Konstruktion geprüft (Intervallanalyse, sonst impliziter Check).
- `match` ist **erschöpfend**: Fehlt eine Variante ohne `case _:`, ist das ein Compile-Fehler — die konstruktive Form der Totalität für Summentypen. Speicherbedarf = größte Variante plus Diskriminante.
```
match classify(ev):
    case ERASE(s): measure erase_sector = s
    case WRITE(a, n): measure write_addr = a
    case BOOT(v): verify v >= 3, "bootloader too old: {v}"
    case OTHER: pass
```

### 3.8 Optionaltyp `T?`
Werte sind `none` oder ein `T`; `T` wird bei Zuweisung implizit nach `T?` gehoben. Verfügbar: `.valid`, `.or(d)`. Unbewachte Verwendung fügt `check x.valid` ein (Fault `MissingValue`) — mit derselben Dominanzanalyse wie für Channels (3.5) und mit Warnung, damit explizite Behandlung (`if x.valid:` oder `.or(...)`) der Normalfall bleibt. `T?` entsteht aus Channel-Lesen, `decode`, `vec.get(i)` und Musterabgleich; es ist das einzige Modell für abwesende Werte.

Ein `check x.valid` (bzw. `check r.ok`) gilt für den Rest des Blocks oder Segments als Guard: Scheitert er, verlässt die Ausführung den Block (Fault), sonst ist `x` dominiert.

**Ergebnis mit Fehlercode `T!E`.** C-Code liefert Fehlercodes mit Out-Parametern; `T?` kennt nur Abwesenheit. `T!E` ist derselbe Mechanismus mit Nutzlast (E ein Enum):
```
enum HeaderErr: MAGIC, SIZE, VERSION
fn parse_header(b: bytes<4096>, min_version: u32) -> ImageHeader!HeaderErr:
    var h = ImageHeader.decode(b)
    if not h.valid: return ERR(MAGIC)
    if h.size > 1 MiB: return ERR(SIZE)
    if h.version < min_version: return ERR(VERSION)
    return OK(h)
```
`.ok`, `.err`, `.or(d)`, Dominanzanalyse wie bei `T?` (unbewachte Nutzung: impliziter Check, Fault `MissingValue` mit dem Fehlercode in der Nachricht); `match r: case OK(v): … case ERR(e): …` ist erschöpfend. `T!E` ist ein Typkonstruktor wie `T?`; es braucht keine Typ-Generics und ist die Zielform für C-Schnittstellen, die Fehlercodes mit Out-Parametern zurückgeben (13.9).

### 3.9 Bytes, Vektoren, Strings, Tabellen
- `bytes<N>`: Bytefolge mit Länge `0..N`; `.len`, `.push(x) -> bool` (`false` bei vollem Puffer, kein Fault; als Statement mit Stelle für das Ergebnis, 4.4), `.append(src) -> bool` (hängt eine ganze `bytes<M>` an — **alles oder nichts**: passt sie nicht vollständig, bleibt der Puffer unverändert und das Ergebnis ist `false`, damit nie ein halber Rahmen zurückbleibt; `M` darf von `N` abweichen), `.clear()`, Index `b[i]` lesend **und schreibend** (`b[i] = x`), Slice `b[a..c]` mit `0 <= a <= c <= len` (implizit geprüft, Warnung, sonst `RangeFault`). Das Schreiben trifft eine bereits belegte Stelle (`i < len`) und ändert die Länge nicht; ein Index dahinter ist ein `RangeFault` wie beim Lesen, kein stiller Anhang. Damit ist *Backpatching* schreibbar — ein Platzhalter wird gepusht und später gefüllt, wie es Längenpräfix, CRC am Ende und COBS-Rahmung verlangen.
- `vec<T, N>`: beschränkter Vektor mit denselben Operationen — `push`, `append` (gleiche Elementtypen, Kapazitäten dürfen abweichen), `clear` —, Index ebenfalls lesend und schreibend; `v.get(i) -> T?` ohne Fault, `v[i]` mit implizitem Range-Check.
- `map<K, V, N>` (v1.1): beschränkte assoziative Struktur mit offener Adressierung über ein festes Array. `insert(k, v) -> bool` (`false` bei voll, kein Fault), `get(k) -> V?`, `remove(k) -> bool`, `len`, `for (k, v) in m:` in Slot-Reihenfolge. Schlüssel sind POD mit Gleichheit; der Hash ist FNV-1a über die kanonische Byte-Kodierung des Schlüssels und je Edition festgelegt, Sondierung linear, Entfernen per Rückwärtsverschiebung (keine Grabsteine) — Ergebnisse und Iterationsreihenfolge sind auf allen Zielen identisch (Satz 9.4.4). Kosten O(N) je Operation im Worst Case und so im Budget; Speicher `N · (K + V + 1 Byte)`; in `persist var` erlaubt, wenn K und V POD sind.
- `reader`/`writer` (Standardbibliothek, 11.4; **v1.1**, weil beide Blöcke über `bytes<N>` sind und dafür Konstantenvariablen in Generics brauchen, 3.12): Cursor-Bausteine über `bytes<N>` — `var r = reader(frame.data)`; `r.u8() -> u8?`, `r.u16_le() -> u16?`, `r.take(n) -> bytes<M>?` (M aus dem Zieltyp, `none` bei Unterlauf oder `n > M`), `r.remaining`; `var w = writer(buf)` schreibt in einen deklarierten Puffer `buf : bytes<N>`: `w.u8(x) -> bool`, `w.bytes(b) -> bool`, `w.fmt("… {x} …") -> bool` (`false` bei Überlauf), danach `send tx, buf`; `r.str(n) -> str<M>?` liest Text. Bis v1.1 übernimmt `layout` dieselbe Aufgabe: Ein Record mit `layout` liefert `encode`/`decode` für den festen Teil eines Rahmens (3.7), `push` hängt die Nutzlast an, und das Index-Schreiben füllt Platzhalter nach (Backpatching, oben). Das deckt Kopf, Nutzlast und Prüfsumme ab; was fehlt, ist der laufende Cursor über einer Folge ungleicher Felder.
- **`inout`-Parameter** reiner Funktionen: `fn fill[const N](inout b: bytes<N>, x: u8)` ohne Rückgabetyp ist Zucker für eine Rückgabe (`b = fill(b, x)` an der Aufrufstelle); die Funktion bleibt rein, die Zeigerübergabe übernimmt der Compiler (11.2). Ein Argument darf pro Aufruf nur einmal als `inout` gebunden werden und nicht zugleich als weiteres Argument erscheinen (kein Aliasing, statisch geprüft). Das ist die Zielform für C-Funktionen, die Puffer in place ändern. Sie machen Parsen und Zusammensetzen variabler Nutzlasten total und lesbar, ohne dynamischen Speicher: jede reale Nutzlast hat eine feste Obergrenze (Modbus 253 Byte, CAN-FD 64 Byte).
- `line<N>`: Textzeile bis N Bytes mit `.truncated`-Flag (Elementtyp für zeilengerahmte Streams, 8.6); verhält sich sonst wie `str<N>`.
- `str<N>`: `==`, `!=`, `<` (bytewise), `.len`, `.starts_with(lit)`, `.contains(lit)`, `matches`/`has` (8.7); Formatierung `{x}`, `{x:hex}`, `{x:.3}`, `{x:08}` mit statisch bekannter Höchstlänge und definierter Trunkierung.
- `table<A, B>`: Stützstellenliste `[(x0, y0), (x1, y1), ...]`, statisch auf streng steigende `x` geprüft; `interp(t, x)` ist stückweise linear, an den Rändern geklemmt, total, mit statisch beschränkten Kosten.
```
const OCV : table<float[V], float[pct]> = [(3.0 V, 0 pct), (3.4 V, 10 pct), (3.7 V, 50 pct), (4.2 V, 100 pct)]
var soc = interp(OCV, cell_v.min())
```
Arrays und `samples` bieten `.min() .max() .mean() .rms() .count .last` (Reduktionen mit Kosten O(N)).

**Backpatching: einen Rahmen zusammensetzen.** Kopf per `encode`, Nutzlast per `push`, Länge und Prüfsumme nachträglich — das ist der Normalfall jedes Rahmungscodecs, und er kommt ohne `writer` (v1.1) aus. Der Puffer darf dabei gelesen werden, während er entsteht:

```
record Head layout big:
    start : u8 = 0xCC
    len   : u8  # Platzhalter, wird nachgetragen
    kind  : u8

fn frame(kind: u8, pd: bytes<32>, pdl: int in 0..32) -> bytes<64>:
    var out : bytes<64> = default
    out.append(Head(start = 0xCC, len = 0, kind = kind).encode())
    for i in range(32):  # nur weil `pdl` ein Teilstueck begrenzt
        if i >= pdl:
            break
        out.push(pd[i])

    out[1] = (3 + pdl) as u8           # Laenge nachtragen (Index-Schreiben)
    var csum = checksum(out, 3 + pdl)  # liest den Puffer, der gerade entsteht
    out.push(csum)
    return out
```

Zwei Punkte, die beim Lesen von 4.4 und von den `inout`-Regeln (unten) leicht als Verbot erscheinen, es aber nicht sind:

- `out.append(…)` und `out.push(…)` stehen als Anweisung ohne Ergebnisziel — eine Bindung je Zug ist nicht nötig (4.4). Eine ganze Folge hängt `append` in einem Zug an; die Schleife darunter bleibt nur, weil `pdl` ein *Teilstück* von `pd` begrenzt.
- `checksum(out, …)` liest `out` in einem Ausdruck, und die nächste Zeile schreibt hinein. Das ist erlaubt: `out` ist eine benannte lokale Stelle, kein `inout`-Argument. Die Aliasing-Regel weiter unten gilt für die *Parameterbindung* eines Aufrufs, nicht für die Abfolge von Lesen und Schreiben an derselben Stelle — Ausdrücke sind seiteneffektfrei, also ist die Reihenfolge von Anweisungen ohnehin die geschriebene.

Ist die Länge vorab ausrechenbar, darf man sie natürlich gleich setzen; für COBS-Rahmung und CRC über das Vorherige geht das nicht, und dann ist das Index-Schreiben der vorgesehene Weg.

Die drei Pufferarten unterscheiden sich in Zweck und Herkunft, nicht in der Kapazitätsdisziplin — alle drei sind fest begrenzt und total:

| Typ | Inhalt | Herkunft | `.truncated` |
|---|---|---|---|
| `bytes<N>` | Rohbytes | selbst gefüllt, `decode`/`encode`, Byte-Ströme | nein |
| `str<N>` | Text | Literale, Formatierung, `reader.str(n)` | nein |
| `line<N>` | Text einer Zeile | Elementtyp zeilengerahmter Ströme (8.6) | **ja** — der Rand hat gekürzt |

Nur `line<N>` trägt `.truncated`, weil nur dort ein *anderer* (der Treiberrand) die Länge begrenzt hat und das Programm es sonst nicht merken könnte. Beim selbst gefüllten `bytes`/`str` meldet schon `push` beziehungsweise `w.fmt` den vollen Puffer.


### 3.10 Bit-Operationen und Konversionen
- Operatoren `& | ^ ~ << >>` auf allen Integer-Typen. Der Shift-Betrag hat den Typ `int in 0..width-1` (implizit geprüft, meist statisch bewiesen; sonst `RangeFault`); `>>` ist arithmetisch auf signierten, logisch auf unsignierten Typen. Zusätzlich `rotl(x, n)`, `rotr(x, n)`, `x.bit(i) -> bool`, `x.bits(hi, lo) -> int`, `x.with_bit(i, b)`.
- Konversionen sind explizit: `x as u16` ist range-geprüft (`RangeFault` bei Verlust), `x.wrap_u16()` rechnet modulo 2^16 (total), Aufwärtskonversion `x as int` ist immer fehlerfrei. Gemischte Breiten in einem Ausdruck ohne Konversion sind ein Typfehler.
- Die Intervallanalyse propagiert Breitenschranken durch Bit-Operationen (`x & 0xFF` liegt in `0..255`).


### 3.11 Matrizen fester Größe

**Uniforme Form (v1).** `mat<R, C>` hat Elemente vom Typ `float`; `mat<R, C>[U]` trägt eine einheitliche Einheit `U`. Zeilen- und Spaltenzahl sind Compile-Zeit-Konstanten; eine harte Obergrenze gibt es nicht — ab 16 warnt ein Lint (Kosten n³, Scratch n²·8 Byte), und das Zeitbudget (9.4.3) sowie das Speicherbudget (11.5) entscheiden. Temporärwerte von Matrixausdrücken liegen im statischen Scratch der Maschine, nicht auf dem Stack (11.2).
```
var p : mat<2, 2> = [[1, 0], [0, 1]]
const I2 : mat<2, 2> = [[1, 0], [0, 1]]
const F  : mat<2, 2> = [[1, 0.01], [0, 1]]
const H  : mat<1, 2> = [[1, 0]]
var s : mat<1, 1> = H * p * H.transpose()
var k = p * H.transpose() * s.inv()
p = (I2 - k * H) * p
var e = p[0, 1]
```
- Operatoren: `+`, `-` (gleiche Form), `*` (Matrix·Matrix mit Formprüfung, Matrix·Skalar, Skalar·Matrix), `transpose()`, `det()`, `inv()` (LU mit Spaltenpivotisierung, R = C; singulär → `ArithmeticFault(Singular)`), `solve(A, b)`, `cholesky()` → `mat<R, R>?` (`none`, wenn nicht positiv definit), Elementzugriff `A[i, j]` mit Indizes vom Typ `int in 0..R-1` bzw. `0..C-1`.
- Totalität und Kosten: alle Operationen sind total (nicht-endliche Ergebnisse → `ArithmeticFault(NonFinite)` wie 4.1); Kosten sind statisch O(R·C·K) bzw. O(n³) und gehen klassifiziert in das Budget ein (9.4.3). Determinismus über `libtaktm` (4.2).
- Iterative Verfahren (Newton, Gauss-Seidel) schreibt man als `for` mit statischer Iterationszahl und `break` bei Konvergenz — die Iterationszahl ist Teil des Budgets, wie im Embedded-Bereich üblich.

**Dimensionierte Form (v1.1).** Uniforme Einheiten decken Zustandsschätzer nicht ab: Ein Zustand aus Position und Geschwindigkeit hat Elemente in `m` und `m/s`, seine Kovarianz Elemente in `m^2`, `m^2/s`, `m^2/s^2`. Grundlage der allgemeinen Form ist ein Satz von Hart (*Multidimensional Analysis*, 1995): Jede Matrix, die in dimensionskonsistenter linearer Algebra vorkommt, hat notwendig Elementeinheiten der Form `r_i * c_j` — das äußere Produkt eines Zeilen- und eines Spalten-Einheitenvektors. Kovarianz, Übergangsmatrix, Messmatrix und Kalman-Gewinn erfüllen das alle. Damit ist die Typregel klein und bleibt ein Vergleich von Exponentenvektoren (3.2):
```
Typ:          mat[R, C] mit Einheitentupeln R = (r_1..r_m), C = (c_1..c_n); Element (i, j) hat Einheit r_i * c_j
Vektor:       vec[R] = mat[R, (1)]
Uniform:      mat<m, n>[U] = mat[(U, .., U), (1, .., 1)]                   (Zucker; mat<m, n> = mat<m, n>[1])
Gleichheit:   mat[R, C] == mat[R', C']  gdw  r_i * c_j == r'_i * c'_j fuer alle i, j
Addition:     nur bei Gleichheit
Produkt:      mat[R, C] * mat[R', C'] verlangt c_j * r'_j == k fuer alle j (dieselbe Einheit k);  Ergebnis mat[k * R, C']
Transponiert: mat[C, R]
Inverse:      mat[1/C, 1/R]      (A * A.inv() = mat[R, 1/R] ist die dimensionierte Einheitsmatrix)
Skalar:       s * mat[R, C] = mat[s * R, C]
Element:      A[i, j] mit konstanten Indizes: r_i * c_j; mit variablem Index nur, wenn alle betroffenen Einheiten gleich sind
Literal:      [[e_11, ..], ..] ist typisierbar, wenn die Elementeinheiten ein aeusseres Produkt bilden (sonst Typfehler)
```
Syntax und Beispiel (ein linearer Kalman-Filter, jede Zeile einheitengeprüft):
```
unitvec X = (m, m/s)  # Zustand: Position, Geschwindigkeit
unitvec Z = (m)       # Messung
var x : vec[X] = [0 m, 0 m/s]
var p : mat[X, X] = [[1 m^2, 0 m^2/s], [0 m^2/s, 1 m^2/s^2]]
const F : mat[X, 1/X] = [[1, (10 ms).as(s)], [0 1/s, 1]]  # F_12 = m / (m/s) = s; float[s] aus einer Dauer (3.3)
const H : mat[Z, 1/X] = [[1, (0 s).as(s)]]
const I : mat[X, 1/X] = [[1, (0 s).as(s)], [0 1/s, 1]]
loop:
    x = F * x
    p = F * p * F.transpose() + Q        # Q : mat[X, X]
    var s = H * p * H.transpose() + R    # R : mat[Z, Z]
    var k = p * H.transpose() * s.inv()  # mat[X, 1/Z]
    x = x + k * (z - H * x)              # z : vec[Z]
    p = (I - k * H) * p
```
Ein vertauschtes `H` oder ein `F` mit falscher Zeiteinheit ist ein Compile-Fehler. `1/X` bezeichnet das elementweise Kehrwert-Tupel. Matrizen haben die Breite von `float` (4.2); ihre Skalarprodukte werden als `fma`-Ketten fester Reihenfolge ausgewertet. Die uniforme Form bleibt der Normalfall für Rotationen, Filterkoeffizienten und Geometrie; die dimensionierte Form ist der Normalfall für Zustandsschätzer.


### 3.12 Generics: Klammerklassen und Monomorphisierung
Die eckige Klammer nach `fn`-, `block`- und `native`-Namen enthält Variablen dreier Klassen:
```
fn clamp[U](x: float[U], lo: float[U], hi: float[U]) -> float[U]                 # Einheitenvariable (v1)
fn first[type T: pod, const N](v: vec<T, N>) -> T?                               # Konstantenvariable (v1.1), Typvariable (v1.2)
fn sum[type T: numeric, const N in 1..4096](a: [N] T) -> T
block window_mean[U, const N in 1..1024]()   step(x: float[U]) -> float[U]
```
- Ein bloßer Bezeichner ist eine Einheitenvariable (3.2); `type T` eine Typvariable mit genau einer eingebauten Fähigkeit aus `{pod, eq, ord, numeric, integer, float}` (Default `pod`); `const N` eine Compile-Zeit-Konstante vom Typ `int` mit optionaler Range, die in Typen (`[N] T`, `bytes<N>`, `mat<R, C>`) und in `range(N)` stehen darf. Nutzerdefinierte Fähigkeiten (Traits) gibt es nicht. Stufen: Einheitenvariablen v1, Konstantenvariablen v1.1, weil die Standardbibliothek sie für `bytes<N>` braucht (11.4), Typvariablen v1.2.
- Inferenz ist lokal aus den Argumenten: Jeder Parameter, dessen Typ genau eine noch offene Variable mit Exponent ±1 enthält, bestimmt sie aus dem Argument, in beliebiger Reihenfolge, bis nichts mehr offen ist. Eine Variable, die in keinem Parameter des Aufrufs oder Konstruktors vorkommt (`lowpass[U](tau: Duration)`, ein Rückgabetyp allein), wird explizit angegeben: `lowpass[bar](tau = 50 ms)`; der Compiler nennt diese Schreibweise. Jede Instanziierung ist statisch und wird **monomorphisiert**: eine eigene MIR-Funktion mit eigenem Budget (9.4.3). Es gibt keinen dynamischen Dispatch und keine Laufzeitkosten.
- Der Instanziierungsgraph ist azyklisch (statisch geprüft); die Instanziierungstiefe ist damit endlich, und die Terminierungsargumente (9.4.2, T3) bleiben unverändert.
- Warum kein Trait-System: Es zöge Auflösungsregeln, Dispatch und Fehlermeldungen nach sich, die weder die Bibliothek noch die Zielgruppe brauchen; sechs feste Fähigkeiten decken die Bibliothek ab.

---

## 4. Ausdrücke und Arithmetik

### 4.1 Totalität als Grundsatz
Jede Operation der Sprache ist eine totale Funktion ihrer Argumente oder erzeugt einen Fault. Es gibt keine dritte Möglichkeit.

| Operation | Regel |
|---|---|
| `int` `+ − *` | Ergebnis in i64 → Wert; sonst `ArithmeticFault(Overflow)`. Explizit: `wrapping_add(a, b)`, `saturating_add(a, b)` (Funktionen wie `rotl`, 3.10). |
| `int / %` | Divisor 0 → `ArithmeticFault(DivZero)` (statisch ausgeschlossen, wenn Range 0 nicht enthält). `/` trunkiert gegen 0, `%` hat das Vorzeichen des Dividenden (Rust-Semantik, dokumentiert). |
| `float` `+ − * /` | IEEE-754 round-to-nearest-even; ist das Ergebnis nicht endlich (Inf, NaN) → `ArithmeticFault(NonFinite)`. Damit existieren NaN/Inf in der Sprache nicht. |
| `sqrt`, `log`, `asin` … | Definitionsbereich verletzt → `ArithmeticFault(Domain)`. |
| Konversionen `int → float` | exakt bis 2⁵³ (f64) bzw. 2²⁴ (f32), sonst gerundet (dokumentiert); `float → int` nur explizit `round/floor/ceil` mit Range-Prüfung. |
| `fma(a, b, c)` | korrekt gerundetes fusedMultiplyAdd; nicht-endliches Ergebnis → `ArithmeticFault(NonFinite)` (4.2). |
| Vergleiche | total (keine NaN). |
| Array-Index | 3.4. |
| Channel-Lesen | 3.5. |
| Bit-Operationen | total; Shift-Betrag range-geprüft (3.10). |
| `as`-Konversion | range-geprüft → `RangeFault`; `.wrap_*()` modulo, total (3.10). |
| `match` | erschöpfend, daher total (3.7). |
| Musterabgleich | total, O(Länge), kein Backtracking (8.7). |
| Matrix-Inversion, `solve` | singulär → `ArithmeticFault(Singular)` (3.11). |
| Native Funktion | total per Vertrag `total` (TCB, 4.5); Kosten per Vertrag `cost`. |

Warum nicht saturierend als Default: Saturierung verbirgt Fehler; in einem Hochkonsequenz-System ist „laut und sicher scheitern" richtig, und die Intervallanalyse beweist die meisten Fälle statisch weg, sodass keine Laufzeitkosten entstehen.

### 4.2 Reproduzierbare Fließkomma-Arithmetik
- LLVM mit strikter FP-Semantik: keine Fast-Math-Flags, `contract=off` (keine FMA-Kontraktion; explizites `fma` siehe unten), keine Reassoziation, kein x87 (SSE2, VFP bzw. ARMv8-NEON, RISC-V F/D).
- Eigene Mathematikbibliothek (`libtaktm`), korrekt gerundete Implementierungen (Ansatz CORE-MATH) für `sin cos tan exp log pow sqrt atan2 …`, auf allen Targets identisch kompiliert. Korrekt gerundete Ergebnisse sind eindeutig, also plattformunabhängig — das ist der mathematisch sauberste Weg zu bitidentischen Traces.
- `f32`, `f64` und `float` werden nie implizit gemischt.
- **Breite von `float`.** `system: float = f32 | f64` (Default f64) legt die Breite programmweit fest — für `float`, `float[U]`, einheitenlose Literale im Float-Kontext, `Duration.as(...)` und die Instanzen der Standardbibliothek. Die Festlegung ist Teil des Logik-Hashs; die Simulation rechnet mit derselben Breite. Sim = HW bleibt, weil die Breite eine Programmeigenschaft ist, nicht eine Targeteigenschaft. Auf Zielen ohne f64-Hardware (Cortex-M4F/M7, RV32IMFC nur f32; Cortex-M0/M3, RV32IMAC gar keine FPU) empfiehlt ein Lint `f32` mit dem Kostenanteil der f64-Operationen (10). `libtaktm` liefert korrekt gerundete Varianten für beide Breiten.
- **FPU im IEEE-Modus.** Die Runtime schaltet Flush-to-Zero und Denormals-Are-Zero aus (x86 MXCSR, ARM FPSCR FZ); auf ARMv7 nutzt der Compiler für Fließkomma nur VFP, nicht NEON (NEON rechnet dort ohne Subnormale); ARMv8 und RISC-V sind IEEE-konform. Die Konformitätssuite prüft die Subnormal-Behandlung mit einem Referenzvektor (13.8). Ohne diese Regel wäre Satz 9.4.4 stillschweigend verletzt.
- **`fma`.** Die Kontraktion von `a * b + c` bleibt verboten (was man schreibt, bekommt man); `fma(a, b, c)` ist eine explizite Primitive: IEEE-754-2008 fusedMultiplyAdd, korrekt gerundet, in Hardware wo vorhanden (VFMA, FMADD, FMA3), sonst korrekt gerundete Software — auf allen Zielen bitidentisch. Die Bibliothek (Polynome in `libtaktm`, Skalarprodukte der Matrizen, Regler) nutzt `fma` explizit.
- **Reduktionen** (Summen, Skalarprodukte) haben eine feste Auswertungsreihenfolge (links nach rechts, als `fma`-Kette); Reassoziation bleibt ausgeschlossen. Elementweise Operationen darf LLVM vektorisieren (keine Umordnung von Rundungen), Reduktionen nicht.
- **Subnormale in Zuständen.** Operationen mit subnormalen Operanden kosten auf x86 das Hundertfache; Filterzustände, die lange gegen null abklingen, erreichen sie sicher. Blöcke der Bibliothek halten ihre Zustände deshalb mit einem expliziten, deterministischen Totband (`if abs(y) < 1e-30: y = 0` in der Einheit des Filters) — identisch auf allen Zielen, kein FTZ als Abkürzung.

Folge (Satz 9.4.4): Ein Programm liefert auf Simulator, Linux-Box und MCU bei gleichen Inputs bitidentische Outputs.

### 4.3 Bedingte Ausdrücke, Bool-Logik
`a if c else b` (Python), `and`/`or` mit Kurzschluss (semantisch irrelevant, da keine Seiteneffekte in Ausdrücken, aber wichtig für implizite Validitätsprüfungen: `x.valid and x > 5` prüft `x` nur im dominierten Zweig).

### 4.4 Ausdrücke sind seiteneffektfrei
Ausdrücke lesen nur; Zuweisungen, Channel-Schreiben, `block.step` und `check` sind Statements. Damit ist die Auswertungsreihenfolge innerhalb eines Ausdrucks semantisch irrelevant, und jede Faultquelle hat eine eindeutige Statement-Position (für „which line").

Das gilt auch für die verändernden Methoden der Sammlungen (`push`, `insert`, `remove`, `clear`, 3.9) und für `step`/`reset` einer Blockinstanz (5.7): Sie stehen als Statement, nie in einem Ausdruck. Verschachtelt (`if b.push(x):`) sind sie ein Fehler, weil sonst die Reihenfolge der Teilausdrücke sichtbar würde.

**Das Ergebnisziel ist optional.** Eine verändernde Methode steht als Anweisung für sich; ein Ziel nimmt ihr Ergebnis nur entgegen, wenn es gebraucht wird:

```
b.push(x)           # Ergebnis verworfen — die übliche Form beim Serialisieren
ok = b.push(x)      # Ergebnis in eine vorhandene Stelle
var ok = b.push(x)  # Ergebnis in eine neue Bindung
```

Ein eigener Wegwerf-Name (`_ = b.push(x)`) ist deshalb nicht nötig und nicht vorgesehen; `_` bleibt dem Padding-Feld in `layout` vorbehalten (3.7). Wer eine feste Byte-Folge zusammensetzt, deren Kapazität statisch über dem Bedarf liegt, schreibt die Züge also ohne Bindungen untereinander — siehe das Backpatching-Beispiel in 3.9.


### 4.5 Native Funktionen mit Kostenvertrag
```
native fn fft256(x: [256] float) -> [256] float with cost = {f64: 20000, mem: 4000}, stack = 1024, total
native fn hmac_sha256(key: bytes<32>, msg: bytes<256>) -> bytes<32> with cost = 60000, stack = 512, total
```
- Eine native Funktion ist eine Signatur mit drei Zusagen: `stack` (maximaler Stack-Bedarf in Byte, aus der Konformitätssuite, geht in die Stack-Zusammensetzung 12.3 ein), `cost` (abstrakte Operationen je Klasse, `cost = {i32: 20000, f32: 4000}`; ein einzelner Wert zählt in der Klasse `i32`; geht als Konstante in das Budget 9.4.3 ein) und `total` (keine Panics, Terminierung, bitreproduzierbare Ergebnisse über alle Targets). Die Implementierung liegt in Rust im Runtime-Crate und gehört zur Trusted Computing Base (9.5); sie ist pur (kein Zustand, keine I/O), Argumente und Ergebnisse haben feste Größe, und sie darf für Fließkomma nur `libtaktm`-Arithmetik nutzen, damit Satz 9.4.4 gilt.
- Verwendung wie `fn`; erlaubt in `fn`, `block`, `machine`.
- **Chunk-Natives** für streamfähige Algorithmen führen ihren Zustand als Wert mit: `sha256_init() -> Sha256Ctx`, `sha256_update(ctx, chunk: bytes<4096>) -> Sha256Ctx`, `sha256_final(ctx) -> bytes<32>`. `Sha256Ctx` ist ein opakes Record fester Größe; die Kette über Ticks ist gewöhnlicher Takt-Code (`ctx = sha256_update(ctx, chunk)` je Chunk aus einem Stream), das Budget pro Tick bleibt statisch.
- **Jobs** für atomare Algorithmen, die in keinen Tick passen (Signaturprüfung: ECDSA P-256 braucht auf einem Cortex-M4 30–100 ms; Entschlüsselung; große Transformationen): `native job ecdsa_p256_verify(key: bytes<64>, digest: bytes<32>, sig: bytes<64>) -> bool with cost = 300, stack = 2048, duration = 120 ms, total`. Aufruf als Statement `job v = ecdsa_p256_verify(...)`; `v.done : bool` und `v.result : T!JobErr` sind *Inputs* (Teil von I_k). Semantik: Der Job startet im Tick des Statements (Kosten `cost` = Start, Argumente werden kopiert), läuft in der Runtime außerhalb der Schrittphase in einem vom Tick unterbrechbaren Kontext mit deklariertem `stack` (im `baremetal`-Profil in der Restzeit der Ticks, im `rtos`-Profil als eigener Task), und seine Fertigstellung erscheint als Input-Ereignis wie ein Sensorwert. Satz 9.4.1 gilt (Funktion der Inputs); Satz 9.4.4 gilt für das *Ergebnis* (bitidentisch), nicht für den Fertigstellungs-Tick — der wird aufgezeichnet und in der Simulation aus Aufzeichnung oder Modell (Default: `duration`) genommen. `duration` ist die deklarierte Worst-Case-Dauer und der Anker für `until v.done timeout …`. Je Maschine höchstens K_j gleichzeitige Jobs (statisch, Default 2); ein Fault-Übergang der Maschine bricht ihre laufenden Jobs ab (5.3); ein Job-Handle ist zustandslokal oder Maschinenvariable.
- v1: nur die kuratierte, mitgelieferte Menge (DSP-Kerne, Prüfsummen, Hashes, MACs). **Projekt-Natives** (v1.1): `native fn crc_custom(...) from "crypto.rs" with …` bindet eine Rust-Implementierung des Projekts ein; Pflicht sind die Konformitätstests aus 13.8 (Bit-Gleichheit über Targets, Panic-Freiheit unter Fuzzing, gemessener `stack`) und die Nennung im Lauf-Header, damit die erweiterte TCB sichtbar bleibt. Frei einbindbare Natives ohne diesen Prozess gibt es nicht. Die Konformitätssuite (13.8) prüft Bit-Gleichheit über Targets und Panic-Freiheit unter Fuzzing.
- Kryptographie bleibt damit außerhalb der Sprache: Konstantzeit und Seitenkanalfreiheit sind Eigenschaften der Implementierung, nicht der Steuerlogik.

---

## 5. Maschinen: Zustände, Übergänge, Faults

### 5.1 Struktur
Eine Maschine besteht aus Variablen, einem optionalen maschinenweiten `loop:` (Interlocks und Berechnungen, die in jedem Tick gelten), einem `initial`-Zustand und einem Baum von Zuständen. Blattzustände sind die möglichen Konfigurationen; ein Konfigurationspfad ist die Kette root ⊂ s₁ ⊂ … ⊂ s_n (Blatt). Zustandsnamen sind maschinenweit eindeutig — Übergangsziele sind damit ohne Pfadangabe eindeutig, was für Techniker wichtiger ist als Namensraum-Eleganz.

**Richtlinie für Regelkreise.** Innerhalb einer Maschine gibt es keine Verzögerung: Messung → Regler-Block → Output sind sequentielle Statements desselben Ticks. Zwischen Maschinen beträgt die Latenz einen Basis-Tick (1.4), nicht eine Periode. Regelschleifen derselben Rate (z. B. Clarke/Park → PI → SVPWM einer feldorientierten Regelung) gehören deshalb in eine Maschine; Kaskaden mit unterschiedlichen Raten (Drehzahlregler bei 1 kHz, Stromregler bei 20 kHz) dürfen über Maschinen verteilt sein — die Sollwert-Latenz von einem Basis-Tick (50 µs bei 20 kHz) ist für die äußere Schleife vernachlässigbar. Phasenstarre Abtastung erreicht man über `tick_source` (7.1).

### 5.2 Ausführung eines Ticks (informell, exakt in 9.3)
1. **Körper.** Die `loop:`-Blöcke der aktiven Kette werden von außen nach innen ausgeführt (Maschine → … → Blatt); unmittelbar nach dem `loop:`-Block einer Ebene laufen deren `on`-Handler über das Stream-Fenster (8.7, nur im Modus Run). Ein `check`, der scheitert, oder ein `abort` beendet die Ausführung der Kette sofort (starker Abbruch: kein weiteres Statement des alten Zustands läuft).
2. **Übergänge.** Ohne Fault werden die Transitionen der Kette von außen nach innen und je Zustand in Quelltextreihenfolge ausgewertet; die erste zutreffende wird genommen (Outer-first-Priorität: eine globale Bedingung überstimmt lokalen Fortschritt). Ein `-> ZIEL` innerhalb eines `loop:`-Blocks ist ebenfalls eine schwache Transition und wird bei Erreichen als „gewählt" vorgemerkt; der Rest des Blocks wird nicht mehr ausgeführt.
3. **Wechsel.** `exit`-Blöcke der verlassenen Zustände (innen → außen), Aktionen des Transitionsblocks, `enter`-Blöcke der betretenen Zustände (außen → innen). Der kleinste gemeinsame Vorfahr bleibt aktiv (Standard-Statechart-Semantik).
4. **Entry-Tick-Regel.** Die `loop:`-Blöcke der *neu betretenen* Zustände (unterhalb des kleinsten gemeinsamen Vorfahren; die darüberliegenden Blöcke liefen in diesem Tick bereits) laufen noch im selben Tick im *Entry-Modus*: `check`s wirken, `-> ZIEL` sind wirkungslos. Dadurch werden die Invarianten des neuen Zustands geprüft, *bevor* die Outputs des Ticks committet werden. Kein Block läuft zweimal pro Tick. `on`-Handler laufen im Entry-Modus nicht (das Stream-Fenster ist dort leer), damit ein Übergang auf ein im Eintritts-Tick eingetroffenes Element weder unterdrückt noch das Element verbraucht wird. Zustandslokale Variablen (5.8) werden beim Eintritt neu initialisiert.
5. **Fault-Behandlung.** Ein Fault (aus Schritt 1, 3 oder 4) führt sofort zum Fault-Ziel des innersten Zustands, der eines deklariert (sonst des nächsten äußeren, sonst `FAULTED`). Das Fault-Ziel wird betreten (exit/enter wie oben) und ebenfalls im Entry-Modus ausgeführt; weitere Faults folgen dem Fault-Wald bis zu einem Zustand, dessen Körper nicht mehr scheitert — spätestens `FAULTED`. `FAULTED` führt keinen Nutzercode aus, auch keine `loop:`-Blöcke oder Handler seiner Vorfahren; `abort` und Runtime-Faults sind dort wirkungslos, weil die Outputs bereits sicher sind. Jeder Fault-Übergang verwirft alle geplanten Ausgaben der Maschine (7.5).

Minimale Verweildauer eines Zustands: ein Tick (außer bei Fault). Das ist beabsichtigt: jeder Schritt ist in der Telemetrie sichtbar, und instantane Übergangsketten existieren nicht.

### 5.3 Fault-Wald
- Jeder Zustand s hat ein Fault-Ziel φ(s): explizit (`fault -> X` im Zustand), geerbt vom Elternzustand, sonst von der Maschine, sonst der implizite Zustand `FAULTED`.
- `check e -> X` überschreibt φ für genau diesen Check.
- φ(s) ≠ s ist Pflicht. Der als Fault-Ziel der Maschine deklarierte Zustand (z. B. `SAFE`) erbt φ daher nicht von der Maschine, sondern hat φ = `FAULTED`, sofern er nichts anderes deklariert. Praktische Folge: Interlocks, die im sicheren Zustand nicht mehr gelten sollen, gehören in einen übergeordneten Betriebszustand (Beispiel 14.1, `ARMED`), nicht auf Maschinenebene.
- **Statische Regel:** Der gerichtete Graph {s → φ(s)} ∪ {s → X für `check … -> X` in s} muss azyklisch sein (funktionaler Graph plus Zusatzkanten: Zyklen werden per Tiefensuche gefunden; Fehlermeldung nennt den Zyklus).
- `FAULTED` ist implizit, hat keinen Nutzercode außer der Zuweisung aller von der Maschine besessenen Outputs auf ihre `safe`-Werte, und kann nicht scheitern. Aus `FAULTED` führen nur explizite Transitionen, die der Nutzer auf Maschinenebene deklariert (ein `state FAULTED:` mit `when reset: -> IDLE` ist erlaubt und erweitert den impliziten Zustand um Übergänge). Damit `FAULTED` nie scheitern kann, dürfen die Guards dieser Transitionen keine impliziten Prüfungen enthalten (Channel-Lesen nur unter `.valid` oder mit `.or()`, keine Range-/Arithmetik-Prüfungen); der Compiler erzwingt das (10, Zeile 9).
- `last_fault` (Kind, Nachricht, Quellposition, Tick) ist in jedem Zustand lesbar.
- Ein Fault-Übergang bricht laufende Jobs der Maschine ab (4.5), disarmt ihre Trigger (7.5) und leert die Warteschlangen geplanter Ausgaben (`sched`, 7.5) aller Outputs der Maschine: Ein Safe-Wert darf nie von einem veralteten geplanten Schreibvorgang überschrieben werden.
- Ein explizites `-> FAULTED` ist als Übergangsziel erlaubt (z. B. nach zu vielen Neustartversuchen, Beispiel 14.7).

Fault-Arten: `CheckFailed`, `Expect`, `Timeout`, `SensorFault`, `MissingValue`, `ArithmeticFault(Overflow | DivZero | NonFinite | Domain | Singular)`, `RangeFault`, `StreamOverflow`, `TimingFault`, `ScheduleOverflow`, `Abort`, `Runtime(Overrun | Driver | Watchdog | Hardware | Node)` (`Node` ab v2, 12.9). `Runtime(Driver)` entsteht aus Output-seitigen Vertragsverletzungen eines Treibers (12.6; Input-seitige degradieren stattdessen zu `Bad`), `Runtime(Hardware)` u. a. aus einer anhaltenden Abweichung der Tick-Quelle (7.1). Abort und Runtime-Faults werden in der Abort-Phase zugestellt (5.4). Keine Faults, sondern Alerts sind `PersistReset` (5.9) und `StreamPaused` (5.10).

### 5.4 `abort`
`abort "text"` erzeugt einen Fault der Art `Abort` für die eigene Maschine sofort und merkt ihn für alle anderen Maschinen vor (`raised[m']`, 9.4). Vorgemerkte Abort- und Runtime-Faults werden in der **Abort-Phase** desselben Ticks zugestellt: Nach den planmäßigen Schritten führt jede betroffene Maschine — ob in diesem Tick aktiv oder nicht — ihren Fault-Pfad aus (Wechsel zum Fault-Ziel, Entry-Modus, Fault-Wald). Damit stehen alle Outputs am Commit des Ticks, in dem der Abort ausgelöst wurde, auf den Werten ihrer Fault-Ziele; die Latenz ist unabhängig von den Maschinenperioden. Die Ausführungsreihenfolge bleibt irrelevant, weil `raised` erst nach allen Schritten gelesen wird (Satz 9.4.1).

Auf demselben Weg wirken der Operator-Befehl `abort` (als Input gesampelt, `pending[m]` für alle Maschinen) und Runtime-Faults (7.3, 12.6). **Abort ist idempotent:** Eine Maschine, die ihren Fault-Pfad wegen eines Aborts genommen hat, ignoriert weitere Aborts, bis sie eine normale Transition ausführt (Abort-Latch, 9.3). Ohne diese Regel würde das übliche Muster `if abort_test: abort` in mehreren Maschinen im selben Tick jede Maschine zweimal abortieren und von `SAFE` nach `FAULTED` eskalieren. Runtime-Faults sind bewusst nicht idempotent: Ein Überlauf oder Treiberfehler, der jeden Tick wiederkehrt, eskaliert über den Fault-Wald nach `FAULTED` — laut und sicher. Stream-Überläufe (8.6) sind Sache des jeweiligen Konsumenten und werden bei dessen nächster Aktivierung zugestellt. Die Kosten der Abort-Phase sind statisch: höchstens Σ_m F_m mit dem Fault-Pfad-Budget F_m aus 9.4.3; sie gehen in die Schedulability ein (7.2).

### 5.5 Eingeschränkte Aktionsblöcke
`enter:`, `exit:`, Transitionsaktionen, `at`-Blöcke und `until ... else:`-Blöcke sind Aktionsblöcke. Sie dürfen enthalten: Zuweisungen an Variablen, `persist`-Variablen und Outputs, `send`, `at`, `pulse`, `cancel`, `job`, `log`, `measure`, `verify`, `verdict`, `raise`, `if`/`for`/`match` mit statischen Schranken, `block.reset()`. Verboten: `check`, `expect`, `abort`, `alert`, `wait` und `->` (außer als Abschluss eines Transitionsblocks oder eines `else:`-Blocks). `at`-Blöcke sind weiter eingeschränkt: nur Zuweisungen an skalare Outputs (Bool, Zahl, Enum); ihre rechten Seiten werden zum Planungszeitpunkt ausgewertet. Damit können Aktionsblöcke nicht *absichtlich* scheitern. Implizite Prüfungen — Validität (3.5), Range (3.4), Arithmetik (4.1), `MissingValue` (3.8) — sind in Aktionsblöcken erlaubt und werden wie Arithmetik-Faults behandelt: normaler Fault-Pfad, Terminierung über den Fault-Wald; der Compiler warnt und schlägt `.or(...)` oder `clamp` vor.

### 5.6 Interlocks, Permissives, Alerts
- **Interlock** = `check` im maschinenweiten oder einem übergeordneten `loop:`; gilt in allen darunterliegenden Zuständen.
- **Permissive** = Bedingung in einer `when`-Transition (`when cmd_open and pressure < LIMIT: -> OPEN`). Wird sie ohne Guard betreten, fängt spätestens der Entry-Tick-Check.
- **Alert** = nicht-abbrechender Monitor: `alert cond, "text"`; die Runtime protokolliert Flanken (aktiv/inaktiv) mit Tick und Position; keine Wirkung auf die Steuerung. Ein Alert kann nie einen Fault auslösen — auch nicht über ungültige Sensoren (3.5). **Die Bedingung eines Alerts nennt das zu meldende Ereignis, die eines `check` die einzuhaltende Invariante:** `alert lox_temp > 100 K, "LOX warming"` meldet, *während* es warm ist; `check chamber_p < LIMIT, "overpressure"` faultet, *sobald* der Druck die Grenze erreicht. Beide stehen nebeneinander im selben `loop:` (14.1); die entgegengesetzte Polarität ist beabsichtigt, weil ein Alert ein Ereignis berichtet und ein Check eine Zusicherung erzwingt. Ist ein Input der Bedingung ungültig, gilt der Alert als aktiv und die Meldung trägt den Zusatz „sensor invalid" (3.5) — Beobachtung schweigt nicht, wenn ihr die Grundlage fehlt.
- **Alerts in Schleifen.** Eine Alert-Stelle in einer `for`-Schleife hat je Durchlauf eine eigene Flanke; der Schlüssel ist die Position samt den Indizes der umgebenden Schleifen. Damit meldet `for i in range(16): alert …` jedes betroffene Element einzeln statt nur das erste (14.5). Die Zahl der Flanken bleibt statisch beschränkt, weil jede Schleifenschranke statisch ist (4.3).
- **Bestätigungszeit** = `check p < LIMIT, "overpressure" for 5 ms` — die Reihenfolge ist *Bedingung, Meldung, `for d`*, wie in der Grammatik (2.3, `check_stmt`); `for` steht hinter der Meldung, nicht zwischen ihr und der Bedingung: Der Check scheitert erst, wenn die Bedingung ununterbrochen mindestens 5 ms verletzt ist (Auslöseverzögerung nach IEC 61511). Semantik: ein Zähler pro Check-Stelle, bei Eintritt des Zustands und bei jeder erfüllten Auswertung 0, bei jeder verletzten Auswertung plus Periode der Maschine; Fault bei Erreichen von `d` (9.2). `d` muss ≥ P_m sein und wird wie `after` auf Perioden gerundet (Warnung, 7.1). Zähler von Checks auf Maschinenebene werden nur durch eine erfüllte Auswertung zurückgesetzt, Zähler in Zuständen zusätzlich beim Eintritt. **Steht die Prüfung in einer `for`-Schleife, hat sie je Durchlauf einen eigenen Zähler** (Schlüssel: Stelle plus die Indizes der umgebenden Schleifen), sonst setzten die erfüllten Durchläufe den Zähler der verletzten zurück und die Auslöseverzögerung entschärfte den Check dauerhaft. Dasselbe gilt für die `next`-Zähler von `every` (5.8). Die Zahl der Zähler bleibt statisch beschränkt, weil jede Schleifenschranke statisch ist (4.3). Gleiches für `alert … for d`, dort mit umgekehrter Polarität: der Zähler steigt, solange die Alert-Bedingung *zutrifft*, und der Alert wird bei Erreichen von `d` aktiv. Ohne `for` bleibt das Verhalten sofortig — der Default ist scharf, die Entschärfung sichtbar (Entscheidung 18).
- **Geforderte Latenz** = `check p < LIMIT, "overpressure" within 100 ms` — die Zusage, dass vom Eintreten der Bedingung bis zum sicheren Zustand höchstens 100 ms vergehen. Der Compiler rechnet die Schranke aus Periode, Bestätigungszeit und Fault-Wald (Satz 9.4.5) und lehnt das Programm ab, wenn sie überschritten wird (Prüfung 61). Verglichen wird in **Ticks**: `within 100 ms` bei `T₀ = 1 ms` heißt „in höchstens 100 Ticks" — das ist ohne Kalibrierung exakt entscheidbar. `for` und `within` stehen in dieser Reihenfolge und dürfen zusammen auftreten; die Bestätigungszeit geht dann in die Schranke ein.

**Beobachtung und Steuerung.** Genau fünf Statements sind Beobachtung und können unter keinen Umständen einen Fault erzeugen: `alert`, `log`, `measure`, `verify`, `verdict` (13.5). Genau drei sind Steuerung und erzeugen Faults: `check`, `expect`, `abort`. Ein Test berichtet mit den ersten, eine Sicherheitsfunktion bricht mit den letzten ab; beide dürfen in derselben Sequenz stehen.

### 5.7 Blöcke innerhalb von Maschinen
Ein `block`-Aufruf an einer Stelle des Codes ist eine Instanz (wie ein Operator in synchronen Dataflow-Werkzeugen): `var f = lowpass[bar](tau = 50 ms)` benannt, `if rose(start):` anonym pro Aufrufstelle (die Einheit `bar` steht explizit, weil kein Konstruktorparameter sie bestimmt, 3.12). Jede Instanz darf pro Aktivierungs-Tick höchstens einmal `step` ausführen (statisch geprüft: kein Aufruf in `for`-Schleifen, außer über Arrays von Instanzen `var filters = [8] lowpass[bar](tau = 50 ms)`). Nicht gesteppte Instanzen behalten ihren Zustand.


### 5.8 Ergonomie in Maschinen

| Konstrukt | Syntax | Semantik |
|---|---|---|
| Zustandslokale Variablen | `state RETRY:` / `    var tries : int in 0..3 = 0` | bei jedem Eintritt neu initialisiert; lebt, solange der Zustand aktiv ist; `var` in einer Sequenz wird auf diese Ebene gehoben (6.2) |
| Speicher exklusiver Zustände | — | Zustandslokale Variablen, gehobene Sequenz-Variablen und Captures, Bestätigungs- und `every`-Zähler von Geschwisterzuständen werden im selben Speicher überlagert (11.2); semantikneutral, weil nie zwei Geschwister gleichzeitig aktiv sind und jeder Eintritt neu initialisiert |
| Periodische Teilaktion | `loop:` / `    every 100 ms:` / `        log "..."` | pro Aufrufstelle ein Zähler `next` (Startwert `d`); der Block läuft in Aktivierungen mit `uhr >= next`, danach `next += d`. Die Uhr ist `time_in_state` für eine Stelle in einem Zustand (der Eintritt setzt Zähler und Uhr gemeinsam zurück) und `now` für eine im maschinenweiten `loop:` — dieser Block gehört keinem Zustand, dessen Eintritt ihn neu startete, und verstummte mit `time_in_state` nach dem ersten Zustandswechsel. In einer `for`-Schleife zählt jede Stelle je Durchlauf getrennt (5.6). Folge für die Stelle im maschinenweiten `loop:`: Ihre **Phase überlebt Zustandswechsel und Fault-Pfade**, weil `now` weiterläuft — nach der Rückkehr aus `FAULTED` liegt das Raster unverändert, statt neu zu beginnen. Für Takterzeuger (CAN-SYNC, Abfragezyklen) ist das die gewollte Phasenstarrheit; wer nach jedem Eintritt neu messen will, setzt das `every` in einen Zustand. **Die Kehrseite steht in derselben Regel:** Ein `every` in einem Zustand, der öfter gewechselt wird als seine Periode lang ist, feuert nie — jeder Eintritt setzt `time_in_state` und den Zähler gemeinsam zurück. Ein 200-ms-Polling in einem Zustand, der alle 50 ms verlassen wird, ist stumm, und zwar ohne Diagnose: Ob ein Zustand lange genug aktiv bleibt, ist zur Übersetzungszeit nicht entscheidbar. Wer periodisch arbeiten will, *obwohl* der Zustand wechselt, gehört ins maschinenweite `loop:` |
| Signale | `signal done` (Maschinenebene), `raise done` (Statement), `m.done` (Leser) | veröffentlichter Puls: für Leser im nächsten Basis-Tick genau einen Tick lang `true`; Single-Writer wie `pub var`. **Ein Signal trägt keinen Wert.** Für ein *Ereignis mit Nutzlast* gibt es den internen Strom: `stream<E> ereignis with capacity = 1` — jeder Leser hat seinen eigenen Cursor (8.6), sieht also jedes Element genau einmal, und die Nutzlast ist der Elementtyp. Er beantwortet Kapazität, Überlauf und Lebensdauer bereits; ein Signal mit Nutzlast müsste dieselben Fragen erneut stellen |
| `break` | in `for` | beendet die Schleife; das Budget bleibt die statische obere Schranke |
| Parameter-Defaults | `machine valve_ctrl(..., travel: Duration = 2 s)` | wie `param`; Instanzen dürfen sie weglassen |
| Instanz-Arrays | `instance cells[i in 0..8] = cell_monitor(v = cell_v[i], t = cell_t[i])` | `i` ist Compile-Zeit-Index; `cells[i].state` lesbar; jede Instanz eigener Single-Writer |
| Anforderungsbezug (v1.2) | `check p < LIMIT, "..." req "SR-12"` | Traceability-ID im Report (13.4) |

### 5.9 Persistente Variablen (v1.1)
```
machine bms:
    persist var cycle_count : int in 0..100000 = 0 with min_interval = 10 s
    persist var last_test   : SelftestResult = SelftestResult(passed = false, code = 0, r_int = 0 mohm)
    initial RUN
    state RUN:
        loop: pass
```
- Semantik: Der Anfangszustand s0 enthält die aus dem nichtflüchtigen Speicher geladenen Werte (Schlüssel = Maschine.Variable plus Typ-Hash). Fehlende oder ungültige Werte (Typ-Hash, Range) ergeben den Default plus Alert `PersistReset`. Die Runtime schreibt geänderte Werte asynchron, atomar (Journal) und höchstens alle `min_interval`; das Schreiben ist Beobachtung und liegt außerhalb der Semantik.
- Nur POD-Typen (Skalare, Records, Arrays, Enums); keine Streams, Blöcke oder Optionale. Nur auf Maschinenebene, nicht in Szenarien.
- **Journal-Anforderungen** (Teil der Treiberkonformität 13.8, in der Simulation gegen das Flash-Modell mit Stromausfall-Injektion zu prüfen, 8.11): zwei Slots im Wechsel (ping-pong); jeder Eintrag trägt Sequenznummer und CRC32; ein Schreibvorgang ändert genau einen Slot; beim Start gewinnt der gültige Eintrag mit der höheren Sequenznummer; Schreiben nur mit Sektorgranularität und `min_interval`. Vor `reboot`, `boot_jump` und Deep Sleep (12.7) schreibt die Runtime ausstehende Änderungen synchron.
- Determinismus: Gegeben s0 (im Lauf-Header aufgezeichnet) ist die Trace unverändert eine Funktion der Inputs (9.10).

### 5.10 Schlafzustände (`idle`, v1.1)
```
input  charger : bool         @ hw("gpio/vbus_det") with wake = true
input  button  : stream<Edge> @ hw("gpio/btn")      with max_rate = 50 Hz, wake = true
command wake_up with wake = true

machine field_device:
    initial STANDBY

    state STANDBY idle:
        enter: led = 0
        when button matches Edge(rising = true) as e: -> SELFTEST
        when charger: -> SELFTEST
        after 7 d: -> SELFTEST
```
**Statische Regeln.** Ein `idle`-Zustand und alle seine Vorfahren haben weder `loop:`-Blöcke noch Handler (ein maschinenweiter `loop:` mit Interlocks muss dann in einen Betriebszustand wie `ACTIVE` wandern). Übergänge sind `after` und `when`, deren Guards nur Wake-Quellen (`with wake = true`), Konstanten, Params, Tunables (8.4) und Maschinenvariablen lesen. Interlocks, die im Schlaf gelten sollen, sind Hardware-Wake-Quellen (Komparator, Übertemperaturpin).

**Nicht-Wake-Streams in `idle`.** Eine Maschine in einem `idle`-Zustand hört nicht: Elemente ihrer Nicht-Wake-Streams werden für sie verworfen (Cursor rückt vor, `dropped` zählt, Alert `StreamPaused` beim Verlassen des Zustands), nie als Überlauf-Fault gemeldet. So bleibt der Schritt in `idle` die Identität.

**Systemschlaf.** Das System darf schlafen, wenn alle Maschinen in `idle`-Zuständen sind (gescopte Instanzen in `idle`-Zuständen sind statisch ausgeschlossen, 5.11), keine geplanten Ausgaben ausstehen, keine Faults vorgemerkt sind (`pending`, `raised`), keine Jobs laufen (4.5) und die Fenster aller Wake-Streams leer sind; armierte Trigger handeln autonom auf ihrem Knoten, ihr `fired`-Strom ist Wake-Quelle, wenn er so deklariert ist. Es schläft bis zur frühesten `after`-Frist oder bis ein Wake-Ereignis eintrifft (Wake-Quellen, Operator-Abort, Runtime-Ereignisse wie Watchdog oder Treiberfehler); beim Aufwachen werden `now` und alle `time_in_state` um die übersprungenen Ticks vorgerückt (virtuelle Ticks). Satz 9.9.1 zeigt, dass die Trace mit Schlaf identisch zur Trace ohne Schlaf ist.


### 5.11 Gescopte Instanzen: parallele Komposition ohne Regionen (v1.2)
Parallele Regionen (AND-Zustände) innerhalb einer Maschine bräuchten eine Konfliktsemantik für gemeinsame Variablen, Outputs und Fault-Wälder — genau die Komplexität, die Takt vermeidet. Dasselbe leisten Instanzen, die in einem Zustand deklariert sind und mit ihm leben:
```
state RUNNING:
    instance pump = pump_ctrl(cmd = pump_cmd, out = pump_valve)
    instance fans[i in 0..4] = fan_ctrl(setpoint = fan_sp[i], out = fan_pwm[i])
    instance pid resume = loop_ctrl(setpoint = loop_sp, out = loop_out)  # behaelt seine Konfiguration ueber Deaktivierungen (5.12)
```
Semantik (ASCII):
```
active(inst, k)  = der deklarierende Zustand ist zu Beginn von Tick k in der Konfiguration seiner Maschine
Eintritt des Zustands (switch, Schritt 3 = Initialisierung):
    inst wird initialisiert (Variablen, Bloecke, Timer, Cursor, Zaehler) und betritt initial
    (bzw. den gespeicherten Pfad bei resume); Eintrittsaktionen und loop-Bloecke von inst laufen im selben
    Tick im Modus ENTRY (wie eine Maschine bei Tick 0), unabhaengig von der Periode von inst
Aktiv:  inst wird nach Periode und Phase aktiviert (countdown; bei Aktivierung auf phase gesetzt), steht im Prioritaetsplan nach ihrer scopenden
    Maschine, liest und veroeffentlicht mit Unit-Delay und darf follows mit der scopenden Maschine und mit
    Geschwisterinstanzen deklarieren
Austritt des Zustands (switch, Schritt 2 = exit-Bloecke):
    exit-Bloecke von inst laufen innen -> aussen (wirksam fuer Zustand, pub var, Signale, log);
    danach stehen alle Outputs von inst auf safe; inst kostet keine Schritte mehr; sched, Jobs und Trigger
    von inst werden verworfen; ohne resume wird die Konfiguration verworfen
Faults: eigener Fault-Wald je Instanz; abort erreicht aktive Instanzen in der Abort-Phase; last_fault je Instanz
Beobachtung: Name "maschine.ZUSTAND.inst"; die scopende Maschine liest inst.state und inst.pub_var (Unit-Delay)
```
Statische Regeln: Single-Writer bleibt global — ein Output gehört genau einer Instanz, gescopt oder nicht, auch wenn zwei exklusive Zustände je eine Instanz desselben Templates deklarieren (sie brauchen verschiedene Outputs oder eine Instanz auf höherer Ebene). `idle`-Zustände dürfen keine Instanzen scopen (sie hätten `loop:`-Blöcke). Ein Template darf sich nicht — auch nicht mittelbar — selbst scopen. `persist var` in gescopten Instanzen ist erlaubt (Schlüssel enthält den Scope-Namen).

Wechselwirkungen: Determinismus (9.4.1) bleibt, weil Instanzen Maschinen sind, Unit-Delay gilt und Aktivität eine Funktion der Konfiguration zu Tick-Beginn ist. Budget (9.4.3): Die Kosten einer Konfiguration schließen ihre aktiven Instanzen ein; die Spitzenlast darf über Konfigurationen maximiert werden, weil Instanzen exklusiver Geschwister nie gleichzeitig aktiv sind — eine Verbesserung gegenüber der Summe aller Maschinen (7.2). Speicher (11.2): Zustände exklusiver Instanzen werden überlagert, Output-Latches nicht. Schlaf (9.9): eine inaktive Instanz ist ein Identitätsschritt. Damit ist der frühere Roadmap-Punkt „parallele Regionen" entschieden, nicht offen.

### 5.12 History-Zustände: `resume` (v1.2)
```
state MANUAL resume:  # bei Wiedereintritt wird der zuletzt aktive Kindpfad betreten
    initial COARSE
    state COARSE:
        when refine: -> FINE
    state FINE:
        when coarsen: -> COARSE
```
Semantik (ASCII):
```
saved[s] : gespeicherter Blattpfad unterhalb von s (Teil von Sigma, auf Maschinenebene, nicht ueberlagert; Start: leer)
Austritt von s (switch, Schritt 2):     saved[s] = aktiver Pfad unterhalb von s
Eintritt von s (switch, Schritte 3/4):  Ziel = saved[s], falls vorhanden und der Uebergang s nicht mit einem
                                        benannten Kind adressiert; sonst initial   (tief ueber alle resume-Ebenen)
Fault-Uebergaenge betreten Fault-Ziele immer ueber initial (resume wird ignoriert)
```
Ein Übergang auf ein benanntes Kind (`-> FINE`) ignoriert `resume` für diese Ebene. Der gespeicherte Pfad wird bei Programmstart auf leer gesetzt und nicht persistiert; wer ihn über Neustarts halten will, speichert einen Enum in `persist var` und wählt ihn per Übergang. Gescopte Instanzen mit `resume` (5.11) behalten ihre Konfiguration über Deaktivierungen. Speicher: Tiefe des Teilbaums in Bytes je `resume`-Zustand. Tiefe History mit einem Wort statt flacher History, weil das Wiederaufnehmen einer Betriebsart sonst überrascht („warum COARSE statt FINE?").

---

## 6. Sequenzen als Zucker

### 6.1 Motivation
Abläufe („Ventil öffnen, 150 ms warten, Zündung, Druckaufbau abwarten, LOX öffnen") sind der häufigste Code in Test und Betrieb. Als explizite Zustände geschrieben sind sie korrekt, aber geschwätzig. `sequence` erlaubt lineare Schreibweise; der Compiler erzeugt daraus die Zustände. Sequenzen haben damit *keine* eigene Semantik — jede Frage („was passiert bei Fault?", „wann gilt ein `check`?") wird über die Zustandssemantik beantwortet.

### 6.2 Desugaring (formal)
Eine Sequenz ist eine Liste von Items. Sie wird von links nach rechts in Segmente geteilt; ein Segment endet an jedem `wait`, `until`, an jedem Statement, das ein `->` enthält (auch innerhalb von `if`/`elif`/`else`), und am Ende eines `repeat`-Körpers. Für Segment i entsteht ein Kindzustand S_i des umgebenden Zustands:

| Item | Übersetzung |
|---|---|
| Statement vor der ersten Zeitgrenze des Segments | Teil von `enter:` von S_i (bei Verstoß gegen 5.5, z. B. `check`, siehe unten) |
| `check e [, msg]` an Position p | wird in `loop:` aller Zustände S_j mit j ≥ i aufgenommen — *ab hier kontinuierlich bis zum Ende der Sequenz* |
| `expect e [, msg]` | einmalige Prüfung: `loop:` von S_i erhält `check e` mit Fault-Art `Expect`; ausgeführt genau einmal (Entry-Tick) und dann durch ein Flag deaktiviert |
| `wait d` | `after d: -> S_{i+1}` |
| `until c [timeout d]` | `when c: -> S_{i+1}`; mit Timeout zusätzlich `after d: -> φ(S_i)` mit Fault-Art `Timeout` |
| `repeat n:` | Zählervariable `k_r: int in 0..n = 0`; nach dem letzten Segment des Körpers: `when k_r + 1 < n: k_r += 1; -> S_first_of_body` sonst `-> S_after`; der Rücksprung ist eine schwache Transition, also dauert jede Iteration ≥ 1 Tick |
| `step "name":` | benennt das erste Segment des Körpers für die Telemetrie; sonst reine Gruppierung |
| Statement mit `->` (auch in `if`/`elif`/`else`) | beendet das Segment; jeder Zweig, der mit `->` endet, wird zu einer Transition `when <Bedingung des Zweigs>: <Aktionen des Zweigs>; -> X` von S_i in Quelltextreihenfolge; gibt es einen Zweig ohne `->` (oder keinen `else`), folgt `when true: <dessen Aktionen>; -> S_{i+1}`. Die Bedingungen werden im nächsten Tick (Run-Modus, frische Inputs) ausgewertet; ein bedingter Übergang kostet damit genau einen Tick (5.2). Ein unbedingtes `->` unmittelbar nach `wait`/`until` wird mit deren Transition verschmolzen (kein zusätzlicher Tick, vgl. 6.3) |
| Ende ohne `->` | S_last hält (Sequenz ist abgeschlossen; `done`-Flag lesbar als `<state>.done`) |
| `var x = e` in der Sequenz | wird zur zustandslokalen Variablen des umgebenden Zustands gehoben (5.8) und in `enter:` von S_i zugewiesen; lebt über Zeitgrenzen hinweg |
| `until g as m` | Captures von `m` werden wie gehobene Variablen gespeichert und sind in allen folgenden Segmenten sichtbar; bei Stream-Guards (8.7) gilt das erste passende Element des Fensters als untersucht |
| `until c timeout d -> X` | wie `until`, Timeout führt per schwacher Transition nach `X` statt zum Fault-Ziel |
| `until c timeout d else:` | Timeout führt den Aktionsblock aus; endet er mit `->`, ist das die Transition; sonst geht die Sequenz mit S_{i+1} weiter („weicher Timeout") |
| `measure`, `verify`, `verdict`, `send`, `at`, `job` | Aktionsblock-Statements: Teil von `enter:` des Segments (5.5) |

`check` an Segmentanfängen ist damit kontinuierlich (Ablaufinvariante ab dem Zeitpunkt), `expect` punktuell. Beide Wörter haben in `loop:` und `sequence:` konsistente Bedeutung: „gilt, solange der Scope aktiv ist" bzw. „gilt an dieser Stelle".

### 6.3 Beispiel des Desugarings
```
state IGNITION:
    sequence:
        fuel_main = OPEN
        wait 150 ms
        igniter = true
        until chamber_p > IGNITION_P timeout 500 ms
        lox_main = OPEN
        check chamber_p > IGNITION_P, "flameout"
        wait BURN_DURATION
        -> SHUTDOWN
```
wird zu
```
state IGNITION:
    initial S0
    state S0:
        enter: fuel_main = OPEN
        after 150 ms: -> S1
    state S1:
        enter: igniter = true
        when chamber_p > IGNITION_P: -> S2
        after 500 ms: -> [Fault Timeout → φ(IGNITION)]
    state S2:
        enter: lox_main = OPEN
        loop:
            check chamber_p > IGNITION_P, "flameout"
        after BURN_DURATION: -> SHUTDOWN
```

---

## 7. Zeit und Scheduling

### 7.1 Logische Zeit
Alle Zeitangaben werden zu u64-Tick-Zählern der jeweiligen Maschine: n = ⌈d / P_m⌉; die Darstellung ist u32, wenn die längste Frist der Maschine unter 2³² Aktivierungen liegt (Verengung nach 3.4, semantikneutral). Ist d kein Vielfaches von P_m, warnt der Compiler mit dem effektiven Wert. `after d` feuert im ersten Aktivierungs-Tick mit `time_in_state ≥ d`, nie im Entry-Tick (Mindestverweildauer). Simulation, Replay und Betrieb rechnen mit identischen Zählern.

**Tick-Quelle.** `system: tick_source = hw("tim1/update")` bindet den Basis-Tick an ein Hardware-Ereignis (Timer-Update, PWM-Periode); ohne Angabe erzeugt die Runtime den Tick selbst. `tick` bleibt der nominale Wert der Semantik; die gemessene Periode wird aufgezeichnet. Weicht sie über `tick_tolerance` ab (Default `2 pct for 10 ticks`: erst nach zehn aufeinanderfolgenden Verletzungen, damit Uhrenjitter keinen Fault erzeugt), ist das ein `Runtime(Hardware)`-Fault (Abort-Phase, 5.4). `samples<T, N>`-Kanäle werden auf das Tick-Ereignis ausgerichtet abgetastet (12.3), wodurch z. B. die Strommessung in der PWM-Mitte liegt.

### 7.2 Multirate-Scheduling
- Aktivierung per Abwärtszähler `countdown[m]` (Start `phase_m / T₀`; am Tick-Ende n_m − 1 nach einer Aktivierung, sonst Dekrement): `active(k) = { m | countdown[m] == 0 }`, geprüft in O(|M|) pro Tick; keine Tabelle, keine Hyperperiode zur Laufzeit (9.4).
- Speicher O(|M|); nicht-harmonische Perioden kosten nichts (1.3).
- **`follows` (v1.1).** `machine pwm_mod follows current_ctrl every 50 us:` deklariert eine Vorrangbeziehung: Die `follows`-Kanten müssen einen azyklischen Graphen bilden (Zyklus = Compile-Fehler); die Reihenfolge der Schritte im Tick ist eine topologische Ordnung der Kanten, unter den zulässigen Ordnungen die Prioritätsordnung (kürzere Periode zuerst, dann Deklaration) als statischer Tie-Break. Ein Follower liest `pub var`, Zustand und Signale der gefolgten Maschine *frisch* (Wert nach deren Schritt), wenn sie in diesem Tick aktiv war, sonst aus Ψ_k; die gefolgte Maschine liest den Follower nie frisch. Frische Lesevorgänge gelten nur in der Schrittphase; in der Abort-Phase gilt Ψ_k. Das ist die konkrete Form des Roadmap-Punkts „instantane Kommunikation" — ohne Datenflussanalyse. Empfehlung bleibt: erst Blöcke (5.1), dann `follows`.
- Reihenfolge innerhalb eines Ticks: kürzere Periode zuerst, dann Deklarationsreihenfolge — ohne `follows`-Kanten semantisch irrelevant (Satz 9.4.1), mit `follows` durch die topologische Ordnung festgelegt; praktisch relevant für die Latenz der Output-Commits bei `asap`.
- Budget (Schedulability): `Σ_c (Peak_c + Σ_m F_m,c) · c_target[c] ≤ T₀ − T_IO`, summiert über die Operationsklassen c ∈ {i32, i64, f32, f64, mem, call, native}, mit B_m,c dem statischen Operationsbudget einer Aktivierung je Klasse, F_m,c dem Fault-Pfad-Budget (beide 9.4.3), Σ_m F_m,c den Kosten der Abort-Phase (5.4) und `c_target[c]` der kalibrierten Kostentabelle des Targets (13.8; auf einem Kern ohne FPU liegt `c_target[f64]` zwei Größenordnungen über `c_target[i32]`; auf 32-Bit-Kernen liegt `c_target[i64]` über `c_target[i32]`, auf 64-Bit-Kernen sind beide gleich; Division hat eigene Gewichte, weil sie je nach Kern Hardwarebefehl oder Bibliotheksaufruf ist). `Peak = Σ_m B_m`, denn bei Phase 0 sind in Tick 0 alle Maschinen aktiv (mit gescopten Instanzen, 5.11, ist die Spitze das Maximum über Konfigurationen, weil Instanzen exklusiver Zustände nie gleichzeitig laufen); nur `phase`-Versatz kann die Spitze senken, was der Compiler exakt über die Hyperperiode prüft, wenn H ≤ 10⁶ (bei paarweise teilerfremden Perioden ist die Spitze nach dem Chinesischen Restsatz ohnehin Σ_m B_m). Verletzung ist ein Compile-Fehler mit Vorschlag (Periode erhöhen, Phase verschieben, Fault-Pfade verkleinern).

**Budget je Maschine.** `machine current_ctrl every 50 us with budget = {ram = 2 KiB}` deklariert, was eine Maschine verbrauchen darf. Ohne Deklaration prüft erst die Integration, ob die Summe passt — in einem Projekt mit mehreren Teams fällt die Überschreitung dann auf, wenn sie teuer ist. Mit Deklaration ist das Budget ein **Vertrag**, der lokal und sofort scheitert (Prüfung 62); dieselbe Logik trägt für wiederverwendbare Bausteine. `ram` ist der Speicher der Maschine in Byte, gerechnet wie in `takt size` (11.5, mit Overlay). `wcet` ist vorgesehen, aber erst mit der kalibrierten Kostentabelle entscheidbar (13.8) und meldet bis dahin seine Stufe.

### 7.3 Überlauf zur Laufzeit
Überschreitet ein Tick trotz Budget die Periode (Treiberstörung, Cache-Effekte auf Linux), erzeugt die Runtime `Runtime(Overrun)` als Fault für alle Maschinen im nächsten Tick (Policy konfigurierbar: `fault` (Default) oder `alert` für unkritische Systeme). Der Tick wird nie übersprungen; die logische Zeit bleibt konsistent, die physische Verzögerung wird protokolliert.

### 7.4 Wall-Clock
Nur als Input-Channel (`input wall_time: Duration @ hw("sys/clock")`), damit die Semantik frei von Systemzeit bleibt. Zeitstempel für Recording setzt die Runtime außerhalb der Semantik.


### 7.5 Zeit unterhalb des Ticks

**Zeitgestempelte Eingaben.** Stream-Elemente (8.6) tragen `.t: Duration` mit Hardware-Auflösung, auf die logische Zeitachse abgebildet. `Edge`-Streams liefern Flankenzeitpunkte; Latenzen zwischen Ereignissen sind exakt messbar (`m2.t - m1.t`), unabhängig vom Tick.

**Geplante Ausgaben.**
```
at m.t + BROWNOUT_DELAY:  # absoluter logischer Zeitpunkt; Block: nur Output-Zuweisungen (5.5)
    vbus_en = false
pulse reset_n = false for 20 us  # jetzt setzen, nach 20 us den vorherigen Latch-Wert wiederherstellen
cancel vbus_en                   # ausstehende geplante Schreibvorgaenge dieses Outputs verwerfen
```
Semantik (ASCII, Details in 9.8):
```
Zustand pro Output o:  sched[o] : nach T sortierte Warteschlange der Kapazitaet K_o (Default 4) von (T, value)
                       nur fuer Outputs, die in einem at- oder pulse-Statement vorkommen (statisch bekannt); sonst kein Speicher
exec(at T: o = v):
    T <= now + guard(o)      => (s, FAULT TimingFault(o))        # guard: Treiberlatenz; in der Simulation 0
    len(sched[o]) == K_o     => (s, FAULT ScheduleOverflow(o))
    sonst                    => (s[sched[o] += (T, v)], NORMAL)   # gleiche T: spaetere Anweisung gewinnt
pulse o = v for d            == o = v; at now + d: o = <Latch-Wert von o vor diesem Statement>
```
Die Runtime schreibt `o` zum Hardware-Zeitpunkt `T`; der Latch-Wert von `o` ist ab dem Tick, der `T` enthält, `v` (die Simulation wendet den Wert im Tick `ceil(T / T0)` an). Ein Fault-Übergang der Maschine leert `sched` aller ihrer Outputs (5.3).

**Latenzformel.** Ein Ereignis mit Zeitstempel `t_e` trifft im Basis-Tick k ein (`t_e` in `(t_(k-1), t_k]`), eine Maschine mit Periode P_m sieht es bei ihrer nächsten Aktivierung `t_a >= t_k`, reagiert bis `t_a + W_m` (WCET) und kann eine Ausgabe frühestens für `T >= t_a + W_m + guard` planen:
```
D_min <= P_m + W_m + guard            (worst case; best case W_m + guard)
```
Für die Linux-Box mit P_m = 1 ms, W_m = 100 µs, guard = 50 µs ist D_min ≈ 1,2 ms — die *Präzision* der Ausgabe ist aber die des Hardware-Timers (µs), nicht des Ticks. Ein Sweep von 1,5 ms bis 500 ms in 10-µs-Schritten ist von der Box aus möglich; für kürzere Abstände braucht es einen MCU-Knoten (P_m = 50 µs → D_min ≈ 0,2 ms) oder einen Trigger.

**Präzision.** Jeder Output trägt aus der Hardware-Konfiguration `guard` und `jitter` (im Programm lesbar: `vbus_en.jitter`); beide stammen aus der Konformitätsmessung des Treibers (13.8). Auf einer Linux-Box mit gewöhnlichem GPIO ist `jitter ≈ T0` (die Ausgabe ist dann tick-granular), mit zeitgesteuertem DAQ oder MCU-Knoten liegt er im Mikrosekundenbereich. Regeln: `at` auf einen Output mit `jitter >= T0` erzeugt die Warnung „tick-granular" (10); eine Kampagne, deren Sweep-Schritt für einen in `at` verwendeten Parameter kleiner als `2 * jitter` ist, wird abgelehnt (13.7) — die Messreihe wäre Rauschen.

**Trigger (v1.2): Regeln auf dem I/O-Knoten.**
```
trigger cut_on_erase:
    when dut_log matches "Erasing sector {n:int}"
    then at event.t + 250 us: vbus_en = false
    bound 20 us
```
Ein Trigger ist eine deklarative Reaktionsregel, die der Compiler auf den I/O-Knoten (MCU, FPGA, Ereignismatrix der Peripherie) verlagert:
```
trigger cut_on_erase node io1:
    when dut_log matches "Erasing sector {n:int}"  # nur knotenlokale Inputs/Streams, Muster, Konstanten
    then at event.t + 250 us: vbus_en = false      # nur Output-Zuweisungen auf demselben Knoten
    bound 20 us                                    # vom Knoten garantierte Reaktionszeit

arm cut_on_erase  # Statement, auch in Aktionsbloecken
disarm cut_on_erase
until cut_on_erase.fired as f timeout 2 s  # fired : Eingabestrom mit .t des Feuerns und den Captures (f.n)
check cut_on_erase.armed
```
Semantik: `event` bezeichnet im `then`-Teil das Element, das den `when`-Guard erfüllt hat (mit `.t` und den Captures). Ein Trigger ist eine Funktion des Ereignisstroms seines Knotens ohne eigenen Zustand außer `armed`; er wird mit Ereignisrate ausgewertet, nicht mit dem Tick; seine Ausgabe ist eine geplante Ausgabe mit `guard = bound`. Im Hauptprogramm ist `fired` ein Eingabestrom (Determinismus wie jeder Input; Satz 9.4.4 für das Ergebnis, der Zeitpunkt ist Datum), `armed` ist Zustand der armierenden Maschine, `arm`/`disarm` sind Statements. Ein Fault-Übergang der armierenden Maschine disarmt ihre Trigger (5.3). Kosten: Das Hauptprogramm zahlt `arm`/`disarm` und die Stream-Verarbeitung; der Knoten trägt die deklarierte Auswertungszeit, die in seine Konformität eingeht (13.8). Simulation: der Trigger wird mit `bound` als Latenz emuliert. Statisch geprüft: Guard und Outputs sind knotenlokal (12.9); ein Trigger ohne `node` liegt auf dem Hauptknoten. Grammatik: `trigger_decl`, `arm_stmt`.

---

## 8. Channels, Geräte, Simulation, Profile, Commands

### 8.1 Deklaration
```
input  tank_p     : float[bar] in 0..100 bar @ hw("daq1/ai0")      with max_age = 5 ms
input  tcs        : [16] float[degC]         @ hw("daq1/tc[0:16]") with max_age = 100 ms
output fuel_main  : ValveCmd                 @ hw("plc1/do0")      with safe = CLOSED
output heater_pwm : float in 0..1            @ hw("ctrl/pwm0")     with safe = 0
```
- `hw("adresse")` bindet an einen Kanal der Hardware-Konfiguration (Gerät/Kanal; Kalibrierung, Rohtyp und Enum-Abbildung stehen dort, nicht im Programm).
- `sim("adresse")` an einem **Output** speist im Simulations-Build den Input, der an derselben Adresse mit `hw` gebunden ist.
- `none` erlaubt Deklaration ohne Bindung (Compile-Fehler bei Nutzung im Hardware-Build).
- Outputs verlangen `safe`; die Runtime schreibt diesen Wert bei `FAULTED`, bei Stopp, bei Runtime-Faults und — über die Geräte-Konfiguration — die I/O-Hardware selbst bei Heartbeat-Verlust.
- Weitere Attribute: `rate` für oversampelte Kanäle (8.9), `max_rate`/`capacity`/`framing`/`overflow` für Streams (8.6, 8.8), `wake` für Wake-Quellen (5.10), `max_slew` als Plausibilitätsgrenze (3.5), `jitter` als *Anforderung* an die Präzision eines Outputs (7.5): Der Compiler prüft sie gegen den gemessenen Wert der Hardware-Konfiguration (13.8); ist der gemessene Jitter größer, ist das ein Compile-Fehler. `o.jitter` im Programm liest den gemessenen Wert.
- Deklarierte Ranges werden am Treiberrand erzwungen: Werte außerhalb sind `Bad` mit Grund `OutOfRange` (3.5, 12.6).

### 8.2 Große Systeme
`import channels from "site1.hw"` erzeugt typisierte Channel-Symbole aus der Hardware-Konfiguration (Ergebnis der Autodiscovery). Das Programm bindet nur, was es liest oder schreibt; alle übrigen Kanäle — auch Streams — zeichnet die Runtime auf, ohne dass sie im Programm erscheinen. Channel-Arrays und `for`-Schleifen decken homogene Gruppen (Thermoelement-Bänke) ab.

### 8.3 Simulation und HIL
- **Sim-Build:** Für jeden `hw`-Input — auch Streams — muss ein `sim`-Output existieren (Compile-Fehler „unsimulated input tank_p", mit Vorschlag `output tank_p_sim: float[bar] @ sim("daq1/ai0")`). Simulierte Stream-Inputs werden von Modell- oder Szenario-Maschinen per `send` gespeist; das Element erhält als `.t` die logische Zeit des Commits.
- **Plant-Modelle** sind gewöhnliche Maschinen (z. B. `machine tank_model every 1 ms`), die `sim`-Outputs schreiben und `hw`-Outputs des Steuerprogramms lesen (Unit-Delay). Sie unterliegen denselben Garantien und laufen mit derselben logischen Zeit; im Hardware-Build werden sie nicht gelinkt.
- **HIL-Umschaltung:** ausschließlich über die Bindung. Steuerlogik bleibt byteidentisch, der Programm-Hash der Logik ist in beiden Builds gleich (Bindungen sind Metadaten).
- **Sim-Ausführung** ist schneller als Echtzeit (keine Wartezeit zwischen Ticks) und liefert denselben Trace wie Echtzeitbetrieb.

### 8.4 Parameter und Profile
```
param PEAK_TEMP   : float[degC] in 20..150 degC = 85 degC
param CYCLE_COUNT : int in 1..1000 = 20
profile QUAL:
    PEAK_TEMP   = 120 degC
    CYCLE_COUNT = 50
```
- `param` ist zur Compile-Zeit unbekannt, aber typ- und range-geprüft; die Intervallanalyse nutzt die Range.
- Profile werden beim Laden gewählt (`takt run test.takt --profile QUAL`), gegen Ranges/Einheiten validiert und mit dem Programm-Hash im Lauf-Header aufgezeichnet.
- Während eines Laufs sind Params konstant (Determinismus). Änderung = neuer Lauf.

**Tunables (v1.1).** `tunable param KP : float[pct/bar] in 0..10 pct/bar = 0.5 pct/bar` ist ein Parameter, der während eines Laufs geändert werden darf — semantisch ein Input mit Halte-Semantik: Der Wert in Tick k ist der zuletzt akzeptierte Wert (Start: Default). Änderungen sind Inputs (Teil von I_k), werden gegen Range und Einheit validiert, als *Satz* atomar an einer Tick-Grenze übernommen und mit Tick aufgezeichnet; `takt replay` reproduziert sie exakt (Satz 9.4.1 unverändert, der Lauf-Header enthält die Startwerte). Tunables haben keine Qualität und kein Alter und sind nie `Bad`; ihre Range bleibt Annahme der Intervallanalyse, weil das Setzen sie erzwingt. Sie sind überall erlaubt, wo Ausdrücke stehen (auch in Guards und `after`, dort jeden Tick neu gelesen), nicht aber, wo Compile-Zeit-Konstanten verlangt sind (Array-Größen, Kapazitäten, `repeat`). Die Bedienoberfläche erzeugt automatisch Regler mit Range; `takt tune --save PROFILE` schreibt den aktuellen Satz als Profil.

### 8.5 Commands und Operator-Interaktion
`command start`, `command confirm_arm`, `command wake_up with wake = true`. Ein Command ist ein Puls-Input (ein Tick wahr), mit Berechtigung („Commander") in der Runtime, in der Bedienoberfläche automatisch als Schaltfläche sichtbar; mit `wake = true` ist er Wake-Quelle (5.10). Sequenzen mit menschlicher Bestätigung: `until confirm_arm` (ohne Timeout; der Compiler warnt „unbegrenztes Warten", was hier beabsichtigt ist).

Setpoints vom Operator sind Inputs (`input setpoint: float[bar] @ hw("ui/setpoint")`), keine Sonderkonstrukte.


### 8.6 Ereignisströme (`stream<E>`)

**Motivation.** Logs, Busse, Zähler und Kommandokanäle liefern *Folgen* von Elementen mit eigenen Zeitstempeln, asynchron zum Tick. Das Modell muss (1) nichts verlieren, solange der Konsument mithält, (2) Überlauf als definiertes Ereignis behandeln, (3) pro Aktivierung nur beschränkte Arbeit erzeugen und (4) mehrere Konsumenten mit unterschiedlichen Perioden zulassen.

**Deklaration.**
```
input  dut_log : stream<line<256>> @ hw("uart0/rx")  with max_rate = 2000 Hz, framing = lines, overflow = fault
input  can_rx  : stream<CanFrame>  @ hw("can0/rx")   with max_rate = 5000 Hz
input  edges   : stream<Edge>      @ hw("gpio/cap0") with max_rate = 1 kHz
```
Elementtypen: `u8`, `bytes<N>` (Frames fester Höchstlänge), `line<N>` (3.9), Records mit `layout` (3.7), das eingebaute Record `Edge` (`rising: bool`). Die Bindung eines Elements ist ein **Wrapper** wie `T?` und `T!E` (2.5): Ihr gehören `.t: Duration` (Hardware-Zeitstempel) und `.seq: int`, der Inhalt steht unter `.data` — bei `line<N>` unter `.text`, weil dort Text steht und nicht Bytes. Felder eines Record-Elements sind erst über den Inhalt erreichbar (`f.data.id`), genau wie beim Wrapper „Felder des Inhalts erst nach dem Auspacken". Damit verdeckt kein Metadatum ein Protokollfeld: Ein Rahmen mit eigenem `seq` — in Protokollen der Normalfall — bleibt unter `f.data.seq` lesbar, während `f.seq` die Nummer im Strom nennt.

Attribute: `max_rate` (Pflicht; Elemente pro Sekunde, Einheit `Hz`), `capacity` (Pufferplätze; Default `2 * ceil(max_rate * P_max)`, P_max = größte Periode eines Konsumenten), `capacity_bytes` und `expect_len` (Byte-Ring, unten), `framing` (`raw | lines | cobs | length_prefixed(u16_le) | fixed(N)`; Rahmung ist ein beschränkter Transducer im Treiber), `overflow` (`fault` Default | `drop_oldest`).

**Semantik (ASCII, formal in 9.6).**
```
Zustand pro Stream s:     buf[s] : beschraenkte FIFO (Kapazitaet CAP) von (seq, t, value)
Zustand pro Konsument m:  cur[s, m] : int                    # naechste noch nicht konsumierte seq

tick(k):
  D = vom Treiber gelieferte Elemente, |D| <= MAXPT = ceil(max_rate * T0), t streng steigend
  if len(buf) + len(D) > CAP:
      overflow == fault        => FAULT StreamOverflow(s) fuer jeden nicht-idle Konsumenten bei dessen
                                  naechster Aktivierung; die ueberzaehligen Elemente werden verworfen
      overflow == drop_oldest  => aelteste Elemente verwerfen, s.dropped += n, Alert
  buf.append(D)

activation(m):
  W = [e in buf | e.seq >= cur[s, m]]      # Fenster; |W| <= CAP; im Entry-Modus leer
  examined = -1                             # Konstrukte (8.7) untersuchen Elemente von W in seq-Reihenfolge
  ...                                       # und setzen examined = e.seq
  if examined >= 0: cur[s, m] = examined + 1

end_of_tick:
  entferne e aus buf mit e.seq < min ueber alle Konsumenten von cur[s, m]
```
Elemente, die der Rand nicht dekodieren kann (Record-Streams mit `layout`: zu kurz oder Range-Verletzung eines Feldes), werden gezählt (`s.malformed`), verworfen und per Alert gemeldet. Die Regel **„untersucht heißt konsumiert"** ist die einzige Konsumregel: Ein Konstrukt, das ein Element ansieht, konsumiert es und alle davor. Was niemand ansieht, bleibt für die nächste Aktivierung erhalten. **Der Cursor gehört der Maschine, nicht dem Strom** (`cur[s, m]`, 9.6): Jeder Leser hat seinen eigenen, und mehrere Maschinen sehen unabhängig voneinander *jedes* Element — konsumieren heißt „für diese Maschine erledigt", nicht „aus dem Puffer entfernt". Ein Element verlässt den Puffer erst, wenn alle Konsumenten daran vorbei sind (Eviction über das Minimum der Cursor, 9.6). Ein Strom, den mehrere Maschinen verschieden auslegen, braucht deshalb *keinen* vorgeschalteten Verteiler mit veröffentlichten Feldern; jede liest ihn direkt. Das Fenster W ist pro Aktivierung fest; verschachtelte Zustände sehen dasselbe W. `s.count` (Elemente in W), `s.dropped`, `s.overflowed` lesen ohne zu untersuchen; `s.skip()` untersucht alles (verwirft das Fenster).

**Statische Prüfung.** `MAXPT * n_m <= CAP` für jeden Konsumenten m (in seiner Periode können nicht mehr Elemente eintreffen, als der Puffer fasst); sonst Compile-Fehler mit Vorschlag für `capacity`.

**Byte-Ring für Elemente variabler Länge.** Ein `stream<line<256>>` mit `capacity = 64` würde mit Slots fester Größe 16 KB belegen, obwohl Zeilen typisch 30–80 Byte lang sind. Elemente variabler Länge (`line<N>`, `bytes<N>`) liegen deshalb in einem Byte-Ring der Größe `capacity_bytes`, dazu ein Deskriptor-Ring mit `capacity` Einträgen `(seq, t, offset, len)`; die FIFO ist damit zweidimensional beschränkt (Elemente und Bytes), Fenster und Cursor bleiben unverändert. Der Default `capacity_bytes = capacity * N` ist unverändert sicher. Wer Speicher sparen will, deklariert `with expect_len = 80`: dann ist der Default `capacity * expect_len`, und der Compiler warnt einmal, dass Lemma 9.6.1 nun die Annahme „mittlere Länge ≤ `expect_len` über `capacity` Elemente" braucht; ein Überlauf bleibt der definierte Fault oder `drop_oldest` — nie ein undefinierter Zustand. Streams fester Elementgröße (`u8`, Records, `Edge`) behalten Slots fester Größe, weil sie dort optimal sind.

**Garantien.** `buf` und `cur` sind beschränkt (Σ bleibt endlich dimensioniert); D ist Teil von I_k; Überlauf ist ein Fault; `.t` ist Datum. **Budget:** pro Aktivierung höchstens CAP Elemente; jedes Konstrukt über W kostet `CAP * (cost_match + cost_body)`.

**Interne Streams (Warteschlangen zwischen Maschinen).** Ein Stream ohne Hardware-Bindung ist eine Warteschlange mit denselben Regeln:
```
stream<UpdateMsg> update_q with capacity = 16  # Dateiebene; genau ein Schreiber (Single-Writer, statisch)

machine receiver:  # Schreiber
    initial RUN
    state RUN:
        on can_rx as f:
            send update_q, UpdateMsg(kind = CHUNK, data = f.data.data)

machine flasher:  # Leser, Cursor je Konsument
    initial RUN
    state RUN:
        on update_q as m:
            log "update chunk {m.seq}"
```
Elemente, die in Tick k gesendet werden, sind für Leser ab Tick k+1 sichtbar (Unit-Delay wie Ψ; die Ordnungsunabhängigkeit aus Satz 9.4.1 bleibt); `.t` ist die logische Sendezeit, `seq` läuft je Stream; ein `follows`-Leser (7.2) sieht die Elemente desselben Ticks frisch. Überlauf trifft den Schreiber (`send` → `StreamOverflow`, wie bei Ausgabeströmen 8.8). Budget und Speicher wie oben (Byte-Ring bei variabler Länge). Damit wird „Task → Maschine, Queue → interner Stream" zur mechanischen Übersetzungsregel (13.9).

### 8.7 Muster, Captures, Handler

**Motivation.** Logzeilen und Textprotokolle brauchen Erkennung mit Werteextraktion. Reguläre Ausdrücke wären mächtig, aber kryptisch und — mit Backtracking — nicht linear. Die Lösung sind **typisierte Muster**: Sie beschreiben eine reguläre Sprache und werden zur Compile-Zeit in einen deterministischen endlichen Automaten übersetzt; Matching ist pro Zeichen O(1), speicherfest und total.

**Musterliterale.**
```
"Erasing sector {n:int}"
"Boot v{major:int}.{minor:int} ({build:word})"
"Recovery: {outcome:word} after {dt:float} ms"
"{_}CRC mismatch{_}"  # {_} = beliebiger Text, wird verworfen
```

| Art | Zeichenklasse | Ergebnistyp |
|---|---|---|
| `int` | `[+-]?[0-9]{1,19}` | `int` (Überlauf → kein Match) |
| `hex` | `(0x)?[0-9a-fA-F]{1,16}` | `int` |
| `float` | `[+-]?[0-9]+(.[0-9]+)?([eE][+-]?[0-9]+)?` | `float` (nicht endlich → kein Match) |
| `word` | `[A-Za-z0-9_]{1,64}` | `str<64>` |
| `str` / `str<N>` | beliebige Zeichen, höchstens N | `str<N>` |
| `_` | wie `str`, ohne Bindung | — |

Escapes `{{`, `}}`. Zwei Platzhalter dürfen nicht direkt aufeinander folgen. Auf `int`, `hex`, `float`, `word` muss ein Literal folgen (oder das Ende), dessen erstes Zeichen nicht zur Klasse gehört — sonst Compile-Fehler „mehrdeutiges Muster". `str` und `_` enden beim ersten Vorkommen des folgenden Literals (leftmost-shortest). Damit ist jede Capture-Grenze eindeutig, und Matching plus Extraktion ist ein einziger Vorwärtsdurchlauf.

Mathematisch beschreibt ein Muster `l0 c1 l1 ... ck lk` die Sprache `l0 . C(c1) . l1 . ... . C(ck) . lk`, ein Produkt regulärer Sprachen, also regulär; der DFA entsteht per Thompson-Konstruktion und Teilmengenkonstruktion zur Compile-Zeit, seine Größe ist durch die Musterlänge beschränkt. Kein Backtracking, keine Rekursion.

**Record-Muster.** Für Streams von Records: `CanFrame(id = 0x7E8)` matcht Elemente, deren aufgeführte Felder den konstanten Ausdrücken gleichen (endliche Konjunktion von Gleichheiten, total).

**Operatoren.**
```
line matches P            # ganze Zeile (Bool)
line has P                # Teilzeichenkette: Kurzform fuer  line matches "{_}" + P + "{_}"
line matches P as m       # Bindung: m.n, m.outcome, ... nur im dominierten Zweig sichtbar (Capture-Namen unterliegen 2.5)
```
`matches`/`has` sind auf `str<N>`, `line<N>`, Record-Werten und in Stream-Konstrukten erlaubt, auch in `fn`. Bei Stream-Elementen trägt die Bindung neben den Captures `m.t`, `m.seq` und den Inhalt `m.data` bzw. `m.text` (8.6). Weil die Bindung ein Wrapper ist und kein Abbild, steht der Inhalt als *ein* Wert zur Verfügung: `parse_frame(m.data)` gibt ihn an eine reine Funktion weiter, ohne ihn Feld für Feld zu kopieren.

**Handler.**
```
state UPDATING:
    on dut_log matches "Erasing sector {n:int}" as m:
        measure erase_sector = m.n
    on dut_log has "CRC mismatch" as ev:
        verdict fail "CRC error during update: {ev.text}"
    on dut_log as ev:  # Catch-all
        log "{ev.t}: {ev.text}"
```
Dispatch pro Aktivierung im Modus Run (formal in 9.7):
```
dispatch(state, W):
  for e in W in seq-Reihenfolge:
      for h in handlers(state) in Quelltextreihenfolge:
          if matches(h.pattern, e) as b:
              (v, out) = exec(h.body with b, v, RUN)
              examined = e.seq
              if out == GOTO q or out == FAULT f: return out     # Rest von W bleibt fuer den Folgezustand
              break                                              # erster passender Handler gewinnt
      examined = e.seq                                           # auch unpassende Elemente gelten als untersucht
  return NORMAL
```
Handler laufen unmittelbar nach dem `loop:`-Block ihrer Ebene, Vorfahren vor Nachfahren; jede Ebene sieht das vollständige Fenster W. Ein Übergang aus einem Handler stoppt die Verarbeitung; die restlichen Elemente bleiben im Puffer und werden vom Folgezustand im nächsten Tick verarbeitet — Ereignisse gehen über Zustandsgrenzen hinweg nie verloren.

**Sequenzen und Übergänge.**
`until s as e timeout d` und `when s as e:` (ohne Muster) treffen das *nächste* Element des Fensters — für Streams ohne Textmuster (Rohbytes, Chunks, Records ohne Feldbedingung).
```
until dut_log matches "Boot v{major:int}.{minor:int}" as m timeout 2 s
until dut_log has "Update complete" timeout 30 s else:
    verdict fail "no completion message"
    -> RECOVER
when can_rx matches CanFrame(id = 0x7E8) as f: -> GOT_RESPONSE
```
Ein Stream-Guard sucht das erste passende Element in W und setzt `examined` auf dessen `seq`; Elemente danach bleiben unkonsumiert. `for ev in s:` iteriert über W (beschränkt durch CAP), `break` erlaubt.

**Garantien und Budget.** Matching ist total und O(Zeilenlänge); Captures sind beschränkte Werte; Handler-Körper sind gewöhnliche Statements. Budget pro Zustand mit Handlern: `CAP * (max_len + max_h cost(h.body))`.

### 8.8 Ausgabeströme
```
output dut_tx : stream<u8> @ hw("uart0/tx") with max_rate = 11520 Hz, capacity = 256
send dut_tx, "UPDATE {size} {crc:hex}\n"  # formatiert in festen Puffer (Hoechstlaenge statisch bekannt)
send dut_tx, frame.encode()               # bytes<N>
```
Ein Ausgabestrom hat einen Sendepuffer (`capacity`, Default 256 Bytes), den der Treiber mit `max_rate` leert. Der freie Platz `tx.free` wird zu Tick-Beginn als Input gesampelt (Determinismus über den Input, wie bei jedem Sensor; die Simulation leert exakt `max_rate * T0` Bytes pro Tick). Statisch prüft der Compiler, dass die Summe der Höchstlängen aller in einer Aktivierung erreichbaren `send`-Statements `capacity` nicht übersteigt; zur Laufzeit ist `send` mit `len > tx.free` ein `FAULT StreamOverflow(tx)` (oder Alert bei `overflow = drop`). Gesendet wird beim Commit des Ticks in Sendereihenfolge.

**Ein Pin und ein Byte werden gemeinsam committet, aber nicht gemeinsam wirksam.** `commit(L)` schreibt Latches und Sendepuffer im selben Schritt (9.4); was danach geschieht, unterscheidet sich: Der Latch eines Pins liegt am Ende des Commits an der Leitung, die Bytes des Sendepuffers laufen mit `max_rate` hinaus. Wer in einem Tick `dc = false`, `send tx, cmd` und `dc = true` schreibt, bekommt darum *nicht* die Reihenfolge, die der Quelltext nahelegt — der Pin steht am Tickende auf `true`, während das Byte noch in der FIFO liegt.

Das ist kein Mangel des Commits, sondern die Grenze eines synchronen Modells an einem asynchronen Bus: Ein Tick ist die kleinste Einheit, in der die Sprache Reihenfolge zusagt, und ein SPI-Transfer ist kürzer als einer. Ein Treiber, der Steuerleitungen zu einzelnen Bytes moduliert, braucht darum je Phase einen Tick (oder einen Bus-Treiber, der die Leitung selbst führt — das ist die Aufgabe von `port`, 15). Der Compiler warnt nicht: Welche Leitung zu welchem Byte gehört, steht nicht im Typsystem.

### 8.9 Oversampelte Kanäle, Register, Capture-Fenster
```
input  i_dut : samples<float[A], 100> @ hw("daq1/ai2") with rate = 100 kHz  # 100 Samples je 1-ms-Tick
check i_dut.max() < 2 A
for x in i_dut:  # beschraenkt durch 100
    alert x > 1.5 A, "current spike {x}"
```
`samples<T, N>` liefert pro Basis-Tick ein beschränktes Array; Reduktionen `.min() .max() .mean() .rms() .count .last`; fehlende Samples ergeben Qualität `Stale`, ein Sample außerhalb der deklarierten Range macht das ganze Tick-Array `Bad` (Grund `OutOfRange`, konservativ). Die Abtastung ist auf das Tick-Ereignis ausgerichtet (7.1). Budget O(N).

Einfache Register sind Skalarkanäle, die der Treiber pollt (`input reg_status : u8 @ hw("i2c1/0x36/0x0C") with max_age = 100 ms`); Transaktionen (Schreiben, dann Lesen) laufen über ein Stream-Paar mit Records und `layout`. Die Sprache bleibt frei von Bus-Semantik; der Treiber gehört zur Trusted Computing Base.

`capture<T, N>` (v1.2): vom Treiber um ein Ereignis herum aufgezeichnetes Fenster (Pre-/Post-Trigger), geliefert als Element eines Streams — für Einschalt- und Einbruchskurven der Versorgung. Die Armierung folgt dem Kommando+Status-Muster (8.11), es gibt keine neue Semantik:
```
input  vbus_wave : stream<capture<float[V], 4096>> @ hw("daq1/cap0")     with max_rate = 10 Hz, capacity = 2
output cap_arm   : CaptureCmd                      @ hw("daq1/cap0/arm") with safe = NONE  # NONE, ARM(pre = 1024, level = 3.0 V, edge = FALLING)
on vbus_wave as w:
    measure dip_min = w.samples.min()
    measure dip_at = w.t
```
Ein Capture-Element trägt `.t` (Triggerzeitpunkt), `.pre` und `.post` (Anzahl Samples vor und nach dem Trigger, `pre + post <= N`), `.samples : [N] T` (gültig `0..pre+post-1`), `.rate` und die Reduktionen aus 3.9. Speicher: Byte-Ring nach 8.6 mit Elementgröße `N · sizeof(T)`; Budget O(N) je Verarbeitung.


### 8.10 Hardware-Konfiguration und Geräteprofile (v1.1)
Bus-Mappings — Modbus-Registertabellen, CANopen-PDO/SDO-Zuordnungen, EtherCAT-Prozessabbilder, DMX-Universen, DAQ-Kanalbänke — sind Bindungen zwischen Geräten und logischen Channels und gehören in die Hardware-Konfiguration, nicht in die Steuerlogik. Die Konfiguration ist deklarativ und wird von der Autodiscovery erzeugt oder aus Geräteprofilen abgeleitet:

| Feld | Bedeutung |
|---|---|
| Gerät | Treibertyp, Adresse, Heartbeat, Zykluszeit |
| Channel | Adresse (`"modbus1/40001"`, `"can0/pdo/0x181/0"`), Richtung, Rohtyp, Skalierung/Kalibrierung (linear oder Tabelle), Einheit, Range, `safe`-Wert (Outputs), Rate/`max_rate`, Rahmung |
| Messwerte | `guard`, `jitter` je Output, Abtastlatenz je Input — aus der Konformitätsmessung (13.8) |
| Speicher und Stack | `ram`, `flash`, ggf. `iram`; Stack-Reserven für Runtime, Treiber, ISRs, RTOS (11.5, 12.3) |
| Topologie (v2) | Knoten, Verbindungen, `hops` (12.9) |
| Formatversion | Schemaversion der Konfiguration (11.3) |
| Herkunft | Geräteprofil (EDS für CANopen, ESI für EtherCAT, Modbus-Registertabelle, DMX-Kanalplan) |

Das Programm sieht nur logische Channels: `import channels from "site1.hw"` (8.2) macht sie typisiert sichtbar; die Bindung im Programm (`@ hw("…")`) nennt die Adresse symbolisch. Transaktionen (SDO-Zugriff, Modbus-Lesen mit Antwort) sind mit den vorhandenen Konstrukten ausdrückbar: `send` plus `until rx matches ModbusResp(tid = req.tid) as r timeout 200 ms`; ein `request`-Zucker wäre protokollspezifisch (Korrelation über Transaktions-IDs oder Index/Subindex) und bringt keine neue Garantie. Die Standardbibliothek liefert Protokollpakete: Records mit `layout` für Modbus-, CANopen- und DMX-Frames sowie Hilfsfunktionen für CRC und Codierung (11.4).


### 8.11 Geräte mit Kommando und Status
Sektorlöschung, Programmierung, DMA-Transfers, Kalibrierungen sind Transaktionen, keine Werte. Das Muster braucht keine neue Semantik, aber eine Konvention und ein Simulationsmodell:
```
output flash_cmd    : FlashCmd            @ hw("flash/cmd")    with safe = NONE  # NONE, ERASE(sector), PROGRAM(addr, len), READ(addr, len)
output flash_data   : stream<u8>          @ hw("flash/tx")     with max_rate = 4 MHz, capacity = 4096
input  flash_status : FlashStatus         @ hw("flash/status") with max_age = 10 ms  # IDLE, BUSY, DONE, ERROR(code)
input  flash_rx     : stream<bytes<4096>> @ hw("flash/rx")     with max_rate = 200 Hz, capacity = 2
sequence:
    flash_cmd = ERASE(sector = n)
    until flash_status == DONE timeout 200 ms -> FLASH_FAULT
    flash_cmd = NONE
```
Der Treiber (TCB) führt das Kommando aus; das Programm sieht nur Zustände — genau wie beim Ventil. Die Bibliothek der Simulationsmodelle liefert ein **Flash-Modell** als Maschine (Sektorzeiten, Busy-Verhalten, Rücklesen) mit **Stromausfall-Injektion**: ein Parameter `CUT_AT_BYTE` bricht einen Programmiervorgang mitten im Sektor ab und lässt den Rest unbestimmt. Ein `campaign`-Sweep darüber prüft die Stromausfallsicherheit eines Schreibpfads oder des `persist`-Journals (5.9) deterministisch und reproduzierbar — dieselbe Methodik wie die Versorgungsunterbrechung in Beispiel 14.6.

---

## 9. Formale Semantik

### 9.0 Notation
Alle Regeln sind in ASCII geschrieben und dienen zugleich als Struktur des Referenzinterpreters (13.1):
```
eval(e, s)            -> value | FAULT f           # Auswertung eines Ausdrucks im Zustand s
exec(stmt, s, mode)   -> (s', out)                 # out in {NORMAL, GOTO q, FAULT f}; mode in {RUN, ENTRY}
s[x := v]             Zustand s mit aktualisierter Komponente x
W, cur, buf           Stream-Fenster, Cursor, Puffer (9.6);  sched[o]  geplante Ausgaben (9.8)
```
Mathematische Zeichen (Σ, ⟦ ⟧, ∎) erscheinen nur in der Prosa der Beweise.

### 9.1 Objekte
- Basis-Tick T₀, Maschinen M mit Perioden n_m, Phasen φ_m, Hyperperiode H.
- Für Maschine m: Zustandsbaum (Q_m, ⊂), Konfigurationen C_m (Ketten von root zum Blatt), Variablen V_m (Maschinen- und zustandslokale, endliche Wertebereiche), Block-Zustände B_m, Timer τ_m: aktive Zustände → ℕ (u64), `every`-Zähler, Fault-Register, Stream-Cursor cur[s, m], vorgemerkte Faults pending[m] (von außen: Operator-Abort, Runtime, Stream-Überlauf) und raised[m] (von anderen Maschinen per `abort`), Abort-Latch abort_latched[m], Aktivierungszähler countdown[m], Bestätigungszähler viol[site, index] je `check … for d` (index = Indizes der umgebenden `for`-Schleifen, 5.6), frische Veröffentlichung fresh[m] für `follows`-Leser (7.2), Tunable-Werte (8.4), laufende Jobs (höchstens K_j, 4.5), gespeicherte Blattpfade saved[s] für `resume`-Zustände (5.12), Aktivität und Zustand gescopter Instanzen (5.11), `armed`-Flags der Trigger (7.5) sowie die Zähler und Ringpuffer der `property`-Monitore (13.3). In der verteilten Ausführung (12.9) wird Ψ zu einem Verlauf der Tiefe `max_hops`. Interne Streams (8.6) haben `buf` und Cursor wie Eingabeströme; ihre Lieferung D_k ist die Sendemenge des vorigen Ticks.
- Globaler Zustand Σ = ∏_m (C_m × V_m × B_m × τ_m × cur_m × pending_m) × ∏_s buf[s] × ∏_o sched[o] × Ψ × L, mit Ψ dem Snapshot veröffentlichter Größen (pub-Variablen, Zustände, Signale) und L dem Output-Latch samt Sendepuffern. Alle Komponenten haben statisch feste Höchstgröße (CAP je Stream, K_o je Output).
- Input-Snapshot I ∈ ∏_c (Val_c × {Good, Suspect, Stale, Bad} × ℕ) × Commands × {abort?} × ∏_s D_s × ∏_o free[o] × Tunable-Änderungen × Job-Fertigstellungen (`done`, `result`), mit D_s der Stream-Lieferung des Ticks (|D_s| ≤ MAXPT_s) und free[o] dem freien Sendepuffer.
- Werte: 𝔹, ℤ₆₄ (und schmale/unsignierte Integer), 𝔽 = endliche IEEE-754-Zahlen in der Programmbreite (`system: float`, 4.2), Enums und Summentypen, Records, Arrays, `bytes`/`vec`/`str`/`line` mit fester Kapazität, `T?`, `Duration` = ℤ₆₄, Stream-Elemente (seq, t, value).

### 9.2 Ausdrücke und Statements

Auswertung von Ausdrücken ist eine totale Funktion `eval(e, s) -> value | FAULT f`, definiert durch strukturelle Rekursion; Operatoren sind totale Funktionen mit Fault-Ergebnis nach 4.1 und 3.10; Channel-Lesen liefert den Wert oder `FAULT SensorFault`, `T?`-Lesen den Wert oder `FAULT MissingValue`, sofern nicht durch `valid` dominiert (statisch entschieden); `matches` liefert `true`/`false` und bindet Captures im dominierten Zweig.

Statements haben Big-Step-Semantik mit Ausgang `out` in `{NORMAL, GOTO q, FAULT f}` (intern zusätzlich `BREAK`, das nur `for` erzeugt und konsumiert) in einem Modus `mode` in `{RUN, ENTRY}`:
```
exec(x = e, s, m)           = let r = eval(e, s) in
                              r is FAULT f ? (s, FAULT f) : (s[x := r], NORMAL)
exec(check e, s, m)         = let r = eval(e, s) in
                              r is FAULT f ? (s, FAULT f) : r == true ? (s, NORMAL) : (s, FAULT CheckFailed)
exec(check e for d, s, m)   = let r = eval(e, s) in
                              r is FAULT f ? (s, FAULT f)
                              : r == true ? (s[viol[site, i] := 0], NORMAL)
                              : viol[site, i] + P_m >= d ? (s[viol[site, i] := 0], FAULT CheckFailed)
                              : (s[viol[site, i] += P_m], NORMAL)               # i = Indizes der umgebenden for-Schleifen (5.6);
                                                                                # viol[site, *] wird bei Zustandseintritt 0
exec(-> q, s, RUN)          = (s, GOTO q)
exec(-> q, s, ENTRY)        = (s, NORMAL)                                      # Entry-Tick-Regel
exec(abort, s, m)           = (s[raised[m'] := Abort fuer alle m' != self], FAULT Abort)   # andere Maschinen: Abort-Phase (9.4)
exec(s1; s2, s, m)          = let (s', out) = exec(s1, s, m) in
                              out == NORMAL ? exec(s2, s', m) : (s', out)
exec(if e: b1 else: b2, s, m) = let r = eval(e, s) in
                              r is FAULT f ? (s, FAULT f) : r ? exec(b1, s, m) : exec(b2, s, m)
exec(for i in range(n): b, s, m) = exec(b[i:=0]; ...; b[i:=n-1], s, m)        # n statisch; BREAK beendet die Folge
exec(for x in W: b, s, m)   = exec(b[x:=e_1]; ...; b[x:=e_k], s, m)           # k = len(W) <= CAP; jedes e_j gilt als untersucht
exec(match e: cases, s, m)  = let r = eval(e, s) in
                              r is FAULT f ? (s, FAULT f) : exec(body(case fuer variant(r)) mit Feldern gebunden, s, m)
exec(send o, e, s, m)       = let b = eval(e, s) in  b is FAULT f ? (s, FAULT f)
                              : len(b) <= s.free[o] ? (s[txbuf[o] += b, free[o] -= len(b)], NORMAL)
                              : (s, FAULT StreamOverflow(o))       # interne Streams: free = CAP - len(buf) - len(sendlist), Zustand statt Input (8.6)
exec(at T: b, s, m)         = 9.8
exec(pulse o = v for d, s, m) = exec(o = v; at now + d: o = latch(o), s, m)   # latch(o): Wert vor dem Statement
exec(cancel o, s, m)        = (s[sched[o] := []], NORMAL)
exec(raise sig, s, m)       = (s[pending_signal[sig] := true], NORMAL)
exec(job v = f(args), s, m) = len(jobs[m]) < K_j ? (s[jobs[m] += start(f, eval(args))], NORMAL) : (s, FAULT JobOverflow)
                              # done/result von v sind ab dem Fertigstellungs-Tick Inputs (4.5)
exec(every d: b, s, m)      = time_in_state >= next ? exec(b, s[next += d], m) : (s, NORMAL)
exec(break, s, m)           = (s, BREAK)
exec(alert e, msg, s, m)    = (s + Alert(aktiv = eval(e) == true oder FAULT), NORMAL)  # Bedingung nennt das Ereignis (5.6); FAULT -> aktiv mit Zusatz (3.5)
exec(log | measure | verify | verdict ..., s, m) = (s + Beobachtung, NORMAL)   # nie FAULT (5.6)
```
Der Fault-Wert trägt Art, Nachricht (formatiert in festen Puffer), Quellposition und Tick.

### 9.3 Schritt einer Maschine

Sei `C` die aktive Kette, `loop(st)` der `loop:`-Block und `handlers(st)` die Handler der Ebene `st`, `trans(C)` die Transitionsliste in Outer-first-Reihenfolge, `fault_target(C, f)` das Fault-Ziel nach 5.3 (Check-spezifisch oder Zustand), `switch(C, q, v)` der Konfigurationswechsel in fester Reihenfolge — Ziel unterhalb eines `resume`-Zustands ist der gespeicherte Pfad (5.12); gescopte Instanzen der verlassenen Zustände werden in Schritt 2 deaktiviert, die der betretenen in Schritt 3 initialisiert (5.11) —: (1) neue Konfiguration setzen, (2) exit-Blöcke der verlassenen Zustände innen → außen ausführen — ihre zustandslokalen Variablen sind dabei noch gültig; erst danach ist überlagerter Speicher der verlassenen Geschwister frei (11.2) —, (3) zustandslokale Variablen, Timer, `every`-Zähler und Bestätigungszähler `viol` der betretenen Zustände initialisieren, (4) enter-Blöcke außen → innen ausführen; scheitert eine Aktion, werden die restlichen übersprungen und der Ausgang ist FAULT — der Fault wird dann mit der bereits neuen Konfiguration behandelt; sonst NORMAL und `entered(C_old, C_new)` die Folge der neu betretenen Zustände unterhalb des kleinsten gemeinsamen Vorfahren, von außen nach innen.

```
step_m(C, v, I, Psi):
  W = windows(m)                                           # 9.6; im Modus ENTRY leer
  out = NORMAL
  if leaf(C) != FAULTED:                                   # FAULTED: kein Nutzercode, keine vorgemerkten Faults (5.3, B6)
      if pending[m] != none and deliverable(pending[m], C):  # Operator-Abort, Runtime-Fault, StreamOverflow (9.6)
          out = pending[m]; pending[m] = none
      if out == NORMAL: (v, out) = exec_chain(C, v, RUN)
  if out == NORMAL:                                        # in FAULTED: nur die dort deklarierten Uebergaenge (Guards ohne implizite Pruefungen)
      for t in trans(C):                                   # Outer-first, dann Quelltextreihenfolge
          g = eval_guard(t, v)                             # true | false | FAULT f; bindet Captures (8.7)
          if g is FAULT f: out = FAULT f; break
          if g == true:
              (v, out) = exec(actions(t), v, ENTRY)
              if out == NORMAL: out = GOTO target(t)
              break
  return resolve_m(C, v, out)

resolve_m(C, v, out):                                      # Uebergaenge und Fault-Wald; auch von der Abort-Phase genutzt (9.4)
  while out != NORMAL:
      if out is GOTO q:
          abort_latched[m] = false                         # eine normale Transition loest den Abort-Latch
          C_old = C; (C, v, out) = switch(C, q, v)
          if out == NORMAL: (v, out) = exec_chain(entered(C_old, C), v, ENTRY)
      else:                                                # out is FAULT f
          if kind(f) == Abort:
              if abort_latched[m]: out = NORMAL; continue  # Abort ist idempotent, bis eine normale Transition erfolgt (5.4)
              abort_latched[m] = true
          v.last_fault = f; cancel_all_scheduled(m); cancel_jobs(m); disarm_triggers(m)   # 5.3
          C_old = C; (C, v, out) = switch(C, fault_target(C, f), v)
          if leaf(C) == FAULTED: out = NORMAL              # FAULTED fuehrt nichts aus
          elif out == NORMAL: (v, out) = exec_chain(entered(C_old, C), v, ENTRY)
  return (C, v)

exec_chain(states, v, mode):                               # je Ebene: loop-Block, dann Handler
  for st in states (aussen -> innen):
      (v, out) = exec(loop(st), v, mode);           if out != NORMAL: return (v, out)
      if mode == RUN: (v, out) = dispatch(st, W, v); if out != NORMAL: return (v, out)     # 9.7
  return (v, NORMAL)
```
**Lemma 9.3.1 (Terminierung des Maschinenschritts).** Die `while`-Schleife in `resolve_m` terminiert nach höchstens 1 + d Iterationen, wobei d die Tiefe des Fault-Waldes ist; das gilt für jeden Aufruf, auch aus der Abort-Phase (9.4).
*Beweis.* Im Modus ENTRY ist der einzige Nicht-NORMAL-Ausgang FAULT (Regel für `->`; Handler laufen dort nicht). Nach einer GOTO-Iteration folgen also nur FAULT-Iterationen. Jede FAULT-Iteration wechselt entlang einer Kante des Fault-Waldes, der azyklisch und endlich ist (statische Regel 5.3); `FAULTED` führt keinen Code aus und beendet die Schleife. Jede Iteration führt endlich viele Statements aus (9.4.3). ∎

### 9.4 System-Tick und Sätze
```
tick(k):
  I_k = sample()                          # Skalare (Wert, Qualitaet, Alter), Commands, abort-Flag (-> pending[m] = Abort fuer alle m),
                                          # Stream-Lieferungen D_k (9.6), free[o]; davor Rand-Selbstpruefungen (12.6):
                                          # Input-seitige Vertragsverletzung -> alle Channels des Treibers Bad (Grund Driver); Range-Verstoss -> Bad/Suspect;
                                          # Output-seitige Verletzung -> pending[m] = Runtime(Driver) fuer die Besitzer der Outputs (12.6)
  deliver(D_k)                            # 9.6: Puffer fuellen, Ueberlauf-Faults in pending[m] vormerken
  A = active(k)                           # countdown[m] == 0 (7.2); Reihenfolge: topologisch nach follows, sonst Prioritaet (statisch)
  for m in A:
      (C_m, v_m) = step_m(C_m, v_m, I_k, Psi_k)   # Lesevorgaenge: fresh[m'] fuer gefolgte, bereits gelaufene m'; sonst Psi_k
      fresh[m] = publish_m(v_m)                    # nur fuer Follower in dieser Schrittphase sichtbar
  abort_phase():                          # 5.4: Abort- und Runtime-Faults wirken im selben Tick fuer alle Maschinen
      for m in alle Maschinen (statische Reihenfolge):
          f = raised[m] if raised[m] != none
              else (pending[m] if m not in A and kind(pending[m]) in {Abort, Runtime} else none)
          raised[m] = none
          if f != none and leaf(C_m) != FAULTED:
              if f == pending[m]: pending[m] = none
              (C_m, v_m) = resolve_m(C_m, v_m, FAULT f)   # 9.3; Kosten <= F_m (9.4.3)
  advance_cursors()                       # 9.6: cur[s, m] = examined + 1; Eviction
  Psi_{k+1} = publish(sigma)              # pub var, Zustaende, Signale (pending_signal -> einen Tick sichtbar)
  apply_scheduled(k)                      # 9.8: geplante Ausgaben mit T im Tick k in den Latch
  commit(L)                               # Outputs und Sendepuffer; asap oder boundary
  fuer alle m: countdown[m] = (m in A) ? n_m - 1 : countdown[m] - 1      # Start: phase_m / T0
  if sleep_allowed(): sleep()             # 9.9
```
**Satz 9.4.1 (Determinismus).** (σ_{k+1}, O_k) ist eine Funktion von (σ_k, I_k).
*Beweis.* Jedes step_m ist eine Funktion seiner Argumente (9.2, 9.3 enthalten keine Wahl außer der ersten zutreffenden Transition bzw. des ersten passenden Handlers, beides durch die statische Reihenfolge festgelegt). step_m liest nur σ_m, I_k, Ψ_k und die Stream-Puffer (nur lesend, über den eigenen Cursor) und schreibt nur σ_m, die Latches und Sendepuffer der von m besessenen Outputs (Single-Writer, statisch) und den eigenen Cursor. Ψ_k ist ein Snapshot vom Tick-Anfang; Cursor-Fortschritt und Eviction geschehen erst nach allen Schritten. `abort` schreibt nur `raised[m']`, das kein Schritt desselben Ticks liest; die Abort-Phase läuft danach in statischer Reihenfolge, und jeder Fault-Pfad liest nur den eigenen Zustand und Ψ_k. Mit `follows` liest ein Schritt zusätzlich `fresh[m']` gefolgter Maschinen; diese Kanten bilden einen azyklischen Graphen, ihre Reihenfolge ist statisch (topologische Ordnung mit festem Tie-Break), und die gefolgte Maschine liest den Follower nie frisch. Also kommutieren die step_m paarweise bis auf die `follows`-Kanten, deren Ordnung fest ist, und das Ergebnis ist eine Funktion von (σ_k, I_k). ∎

**Satz 9.4.2 (Totalität / Crash-Freiheit).** Für jedes vom Compiler akzeptierte Programm und jeden Input-Strom (I_k)_{k∈ℕ} existiert die unendliche Trace (σ_k, O_k) und enthält keinen undefinierten Zustand.
*Beweis.* Strukturelle Induktion über Statements mit den statischen Garantien: (T1) Typ-Soundness der Ausdruckssprache (Progress + Preservation, Standard für eine erststufige Sprache ohne Referenzen); (T2) Totalität aller Primitive (4.1, 3.4, 3.5, 3.11) und nativer Funktionen per Vertrag `total` (4.5, TCB); (T3) Terminierung jedes Statements (`for` mit statischer Schranke, kein rekursiver Aufruf: Aufrufgraph azyklisch, Block-`step` höchstens einmal pro Instanz und Tick); (T4) Lemma 9.3.1; (T5) Σ hat statisch feste Größe (mit Overlay exklusiver Zustandsspeicher und statischem Scratch, 11.2), keine Allokationsprimitive; Stack-Tiefe des Programms = längster Pfad im azyklischen Aufrufgraphen plus die deklarierten `stack`-Verträge nativer Funktionen als Blattkosten (4.5); Reserven für Runtime, ISRs und RTOS kommen aus dem Profil und werden gemessen, nicht bewiesen (12.3, 13.8); (T6) Definite Assignment, auch für zustandslokale Variablen bei jedem Eintritt; (T7) keine Exceptions — alle Fehler sind Werte `out`; (T8) eine statische Ausführungsreihenfolge, keine gemeinsam beschriebenen Speicherstellen; (T9) Streams: Fenster durch CAP beschränkt, Handler-Dispatch und `for x in W` sind beschränkte Schleifen, Überlauf ist ein Fault (9.6, 9.7); (T10) geplante Ausgaben: `sched` durch K_o beschränkt, `at` total (9.8); (T11) `match` erschöpfend, Musterabgleich total (3.7, 8.7); (T12) `idle`-Zustände sind ohne Übergang Identitätsschritte (5.10, 9.9); (T13) `BREAK` wird von `for` konsumiert und verlässt nie ein Statement; (T14) die Abort-Phase führt höchstens |M| Fault-Pfade aus, jeden nach Lemma 9.3.1 endlich; (T15) deklarierte Channel-Ranges gelten am Tick-Anfang, weil der Rand sie erzwingt (3.5, 12.6); (T16) Jobs: höchstens K_j je Maschine, Start total (`JobOverflow` ist ein Fault), Argumente fester Größe, Fertigstellung ist ein Input — die Schrittfunktion bleibt total und beschränkt (4.5, 9.2). ∎

**Satz 9.4.3 (Statisches Kostenbudget).** Die Kosten sind Vektoren über den Operationsklassen c ∈ {i32, i64, f32, f64, mem, call, native}: `N(s) ∈ ℕ^7`, `cost(e)` zählt jede Operation in ihrer Klasse (eine Integer-Operation nach ihrer gewählten Darstellung in `i32` oder `i64` (3.4), eine f64-Multiplikation in `f64`, ein Array-Zugriff in `mem`, ein Aufruf in `call`, eine native Funktion mit ihrem deklarierten Vektor in `native`). Das Zeitbudget ist `Σ_c N_c · c_target[c]` mit der kalibrierten Tabelle des Targets (13.8). Mit dem Kostenmodell
```
N(x = e) = cost(e)      N(check e) = cost(e) + 1      N(-> q) = 1
N(s₁; s₂) = N(s₁) + N(s₂)      N(if e s₁ s₂) = cost(e) + max(N(s₁), N(s₂))
N(for i < n do s) = 1 + n·N(s)      N(f(args)) = Σ cost(args) + N(body f)
N(dispatch(st))   = CAP · (max_len + max_h N(body h))        N(for x in W: b) = 1 + CAP · N(b)
N(match e: cases) = cost(e) + max_c N(body c)                N(send o, e)     = cost(e) + len_max(e)
N(at T: b)        = cost(T) + N(b) + 1                       N(every d: b)    = 1 + N(b)
N(interp(t, x))   = O(n) mit statischem n                    N(x matches P)   = max_len(x)
N(native f(args)) = Σ cost(args) + cost_f                    N(A * B)         = R·C·K,  N(A.inv()) = c_inv · n³
```
(alle Gleichungen klassenweise gelesen) sei F_m = max über Konfigurationen C und Fault-Arten f von Σ_{Fault-Pfad ab C} (N(exit/enter) + N(bodies)) das **Fault-Pfad-Budget** (Kosten von `resolve_m(C, v, FAULT f)`). Dann ist B_m = max_C [ N(bodies(C)) + max_t N(actions(t)) ] + F_m eine obere Schranke der pro Aktivierung von m ausgeführten abstrakten Operationen, und Σ_m F_m eine obere Schranke der Abort-Phase eines Ticks; beide sind endlich und zur Compile-Zeit berechenbar. *Beweis:* Induktion über die Syntax; die Maxima laufen komponentenweise über endliche Mengen; Lemma 9.3.1 begrenzt die Fault-Pfade; native Funktionen tragen ihre deklarierten Kosten bei. Die Klassifizierung ändert nichts an der Endlichkeit, nur an der Genauigkeit der Zeitschranke. ∎

**Bemerkung (Loop-Sprache).** Innerhalb eines Ticks ist Takt eine Loop-Sprache im Sinne von Meyer & Ritchie (1967): alle Funktionen sind primitiv-rekursiv, Terminierung ist syntaktisch garantiert. Turing-Vollständigkeit über die Zeit bleibt erhalten (unbeschränkte Input-Ströme). Mit endlichen Datentypen ist das System sogar ein endlicher Transduktor; Eigenschaften über step sind prinzipiell entscheidbar und praktisch per k-Induktion/BMC prüfbar (13.3).

**Satz 9.4.4 (Sim = HW).** Zwei Ausführungen desselben Programms (gleicher Logik-Hash) mit gleichen Input-Strömen und Params liefern auf beliebigen unterstützten Targets bitidentische Output-Ströme.
*Begründung.* Die Semantik (9.2–9.4, 9.6–9.10) enthält keine targetabhängige Größe: feste Integer-Breiten, strikte IEEE-Arithmetik, korrekt gerundete Bibliotheksfunktionen (4.2), logische Zeit (7.1), Zeitstempel, `free[o]` und Job-Fertigstellungen als Daten des Input-Stroms (Job-*Ergebnisse* sind bitidentisch, Fertigstellungs-Ticks werden aufgezeichnet, 4.5), die Breite von `float` als Programmeigenschaft (`system: float`, Teil des Logik-Hashs), FPUs im IEEE-Modus und `fma` als korrekt gerundete Primitive (4.2); die Darstellungsverengung ändert nach Lemma 3.4 kein Ergebnis. Der Compiler darf nur semantikerhaltende Optimierungen anwenden (kein Fast-Math). Verbleibendes Restrisiko ist Compiler-Fehlverhalten, das durch differentielles Testen gegen den Referenzinterpreter (13.1) abgedeckt wird. ∎

**Satz 9.4.5 (Safe-State-Latenz).** Für jede Prüfstelle `s` in Maschine `m` und jeden Output `o`, den `m` schreibt, ist die Zahl der Ticks von der Verletzung der Bedingung bis zum Commit des `safe`-Werts nach oben beschränkt durch

```
D_safe(s, o)  =  n_m  +  ceil(d / P_m)  +  depth(φ(q))  +  D_commit
                 └detect┘ └──confirm──┘  └─Fault-Wald─┘   └0 asap, 1 boundary┘
```

mit `n_m` der Periode in Basis-Ticks (1.3), `d` der Bestätigungszeit aus `for d` (5.6, sonst 0), `q` dem Zustand der Prüfstelle und `depth` der Länge des längsten Pfades im Fault-Wald ab `φ(q)` bis zu einem Zustand, dessen Körper nicht mehr scheitert (5.3).

*Begründung.* `D_detect`: Die Bedingung kann unmittelbar nach einer Aktivierung von `m` wahr werden; die nächste Auswertung liegt `n_m` Basis-Ticks später (9.4, Aktivierungsregel). `D_confirm`: Der Zähler `viol[site]` steigt je verletzter Auswertung um `P_m` und löst bei Erreichen von `d` aus (5.6), also nach `ceil(d / P_m)` Aktivierungen. `D_fault`: Jeder Schritt im Fault-Wald ist ein Tick, weil das Fault-Ziel im Entry-Modus ausgeführt wird und ein erneutes Scheitern den nächsten Fault auslöst (5.3); Prüfung 9 hält den Wald azyklisch, die Kette endet spätestens bei `FAULTED`. `D_commit`: Bei `boundary` steht der Wert einen Tick bis zum Commit; bei `asap` **null**, denn die Entry-Tick-Regel (5.2, Punkt 4) führt die `check`s des neu betretenen Zustands noch vor dem Commit aus — der unsichere Wert wird nie committet. ∎

**Was der Satz nicht sagt.** Die Schranke ist in *Ticks* exakt. Ihre Umrechnung in Zeit (`D_safe · T₀`) setzt voraus, dass jeder Tick eingehalten wird — das prüft die Schedulability (7.2) mit der kalibrierten Kostentabelle (13.8). Sie gilt für die Logik, nicht für den Aktor: `guard(o)` und `jitter(o)` des Treibers (7.5) kommen aus der Konformitätsmessung und sind zu addieren. Und sie ist eine **obere** Schranke; der Normalfall ist kürzer.

**Auszahlung.** `takt latency` gibt die Schranke je Output aus, mit Aufschlüsselung nach den vier Summanden — für IEC 61508 und ISO 26262 ist das die Fault Tolerant Time Interval beziehungsweise Process Safety Time, die sonst geschätzt und per Messung plausibilisiert wird. `check c, "…" within d` deklariert sie als Anforderung; Prüfung 61 vergleicht sie mit der gerechneten Tickzahl und schlägt fehl, bevor das Programm läuft.

### 9.5 Was die Sätze nicht abdecken (Trusted Computing Base)
Compiler und LLVM, `libtaktm`, native Funktionen (4.5), Runtime, Treiber (einschließlich Zeitstempelung, Rahmung, geplanter Ausgaben, NVM-Journal), OS, Hardware. Der Treiberrand ist defensiv (12.6): Vertragsverletzungen werden zu Faults, Werte außerhalb deklarierter Ranges zu `Bad` — das macht Treiber nicht korrekt, aber ihre Fehler laut. Die Sprache garantiert, dass *Programme* keine undefinierten Zustände erreichen; die Plattform muss ihrerseits Watchdog, Heartbeat und Safe-State der I/O-Hardware liefern (12.4). Die physische Präzision geplanter Ausgaben (`guard`, Jitter) ist Treibereigenschaft und wird in der Hardware-Konfiguration dokumentiert.

### 9.6 Ereignisströme
```
buf[s]     : FIFO der Kapazitaet CAP_s Elemente und CAPB_s Bytes von (seq, t, value), seq streng steigend
             (interne Streams: D_k[s] = die im Tick k-1 per send geschriebenen Elemente; Ueberlauf faultet den Schreiber beim send)
             (CAPB_s = CAP_s * N fuer feste Elementgroesse; sonst capacity_bytes, 8.6)
cur[s, m]  : int fuer jeden statisch bekannten Konsumenten m
deliver(D_k):
  for s: if len(buf[s]) + len(D_k[s]) > CAP_s or bytes(buf[s]) + bytes(D_k[s]) > CAPB_s:
             overflow(s) == fault:       pending[m] = FAULT StreamOverflow(s) fuer jeden Konsumenten m; D_k[s] gekuerzt
             overflow(s) == drop_oldest: aelteste entfernen, dropped[s] += n, Alert
         buf[s].append(D_k[s])
windows(m):        W[s] = [e in buf[s] | e.seq >= cur[s, m]]   fuer jeden Stream s, den m liest; leer im Modus ENTRY
advance_cursors(): cur[s, m] = examined[s, m] + 1 falls examined[s, m] >= 0
                   idle(C_m) und s nicht Wake-Quelle: cur[s, m] = max seq in buf[s] + 1, dropped[s, m] += len(W[s])   (5.10)
eviction:          entferne e aus buf[s] mit e.seq < min_m cur[s, m]
deliverable(FAULT StreamOverflow(s), C) = not idle(C) or s ist Wake-Quelle;  deliverable(FAULT Abort, C) = not abort_latched[m]
deliverable(anderer Fault, C) = true
pending[m] haelt hoechstens einen Fault; treffen mehrere in einem Tick ein, gilt die Prioritaet Abort > Runtime > StreamOverflow
```
**Lemma 9.6.1.** |W[s]| ≤ CAP_s und bytes(W[s]) ≤ CAPB_s; `deliver`, `windows`, `advance_cursors` sind Funktionen von (Σ, I_k); die statischen Bedingungen `MAXPT_s · n_m ≤ CAP_s` und `MAXPT_s · n_m · N ≤ CAPB_s` garantieren, dass ein mithaltender Konsument nie einen Überlauf erzeugt; mit `expect_len < N` gilt die Byte-Bedingung nur unter der deklarierten Längenannahme (8.6), und der Überlauf bleibt ein definierter Fault. *Beweis.* Ein Konsument der Periode n_m·T₀ untersucht bei jeder Aktivierung sein gesamtes Fenster, falls er ein Fenster-Konstrukt enthält; zwischen zwei Aktivierungen treffen höchstens MAXPT_s · n_m Elemente ein. ∎

### 9.7 Handler-Dispatch
```
dispatch(st, W, v):
  for s in streams(st) in Deklarationsreihenfolge:        # Streams, auf die st Handler deklariert
    for e in W[s] in seq-Reihenfolge:
      for h in handlers(st, s):
          b = match(h.pattern, e)                  # Bindungen oder none; total, O(len)
          if b != none:
              (v, out) = exec(body(h) with b, v, RUN); examined[s] = e.seq
              if out != NORMAL: return (v, out)
              break
      examined[s] = e.seq
  return (v, NORMAL)
```
Handler verschiedener Ebenen sehen dasselbe W (Fenster ist pro Aktivierung fest); `examined` ist das Maximum über alle Konstrukte der Aktivierung. Terminierung: |W| ≤ CAP, Handlerzahl statisch.

### 9.8 Geplante Ausgaben
```
exec(at T: b, s, m):
  (s', out) = eval_writes(b, s)                         # b enthaelt nur Output-Zuweisungen; rechte Seiten jetzt ausgewertet, Schreibvorgaenge gesammelt
  if out is FAULT f: return (s, FAULT f)
  for (o, v) in writes(b):
      if T <= now + guard(o):        return (s, FAULT TimingFault(o))
      if len(sched[o]) == K_o:       return (s, FAULT ScheduleOverflow(o))
      s'.sched[o].insert((T, v))                        # sortiert nach T; gleiche T: spaetere Anweisung gewinnt
  return (s', NORMAL)
apply_scheduled(k): for o, for (T, v) in sched[o] with T in tick k: L[o] = v; entferne (T, v)
cancel_all_scheduled(m): sched[o] = [] fuer alle o von m
```
`sched` ist beschränkt (K_o), alle Operationen sind Funktionen von (Σ, I); die physische Ausführung zum Zeitpunkt T ist Treiberaufgabe; die Simulation wendet im Tick `ceil(T / T0)` an.

### 9.9 Schlaf
```
sleep_allowed():  alle Maschinen in idle-Zustaenden and alle sched[o] leer and alle Wake-Stream-Fenster leer and kein pending[m] and kein raised[m] and keine laufenden Jobs
sleep():          d = min(naechste after-Frist ueber alle Maschinen, Weckereignis)   # Wake-Quellen, Operator-Abort, Runtime-Ereignisse
                  n = d / T0;  fuer jede Maschine: time_in_state += n*T0, every-Zaehler unveraendert;  now += n*T0
```
**Satz 9.9.1 (Schlaf ist unsichtbar).** Die Trace mit Schlaf ist identisch zur Trace ohne Schlaf.
*Beweis.* In einem `idle`-Zustand ist `step_m` die Identität auf (C, v, B), solange keine Transition feuert: Es gibt keine `loop:`-Blöcke und keine Handler (statisch, 5.10); Nicht-Wake-Streams werden ohne Fault verworfen (9.6); `when`-Guards hängen nur von Wake-Quellen ab, die während des Schlafs definitionsgemäß nicht feuern (das erste Feuern beendet den Schlaf); `after` feuert frühestens bei der frühesten Frist, die das Ende des Schlafs ist. Die übersprungenen Ticks wären also leere Schritte gewesen; `now` und die Timer werden exakt um sie vorgerückt. ∎

### 9.10 Persistenz
`persist`-Variablen sind gewöhnliche Komponenten von V_m; s0 = Defaults, überschrieben durch geladene, validierte Werte. Damit ist Satz 9.4.1 unverändert eine Aussage über (s0, (I_k)); das asynchrone Schreiben durch die Runtime liegt außerhalb der Semantik (Beobachtung).

---

## 10. Statische Analysen — das Compiler-Gate

„If it compiles" bedeutet: alle folgenden Prüfungen sind bestanden. Fehlermeldungen nennen Position, Ursache und einen konkreten Vorschlag; Warnungen sind per Projektkonfiguration zu Fehlern eskalierbar (empfohlen für Zertifizierungsprojekte).

| # | Analyse | Fehler / Warnung |
|---|---|---|
| 1 | Tokenizer/Parser (Einrückung, Grammatik) | F |
| 2 | Namensauflösung, Modul-Linking, Namenskonventionen | F / W |
| 3 | Typ- und Einheiteninferenz, affine Regeln, Duration-Regeln | F |
| 4 | Intervallanalyse: Ranges, Division, Overflow, Index; implizite Prüfungen | W bei jeder eingefügten Laufzeitprüfung (außer i64-Overflow) |
| 5 | Validitäts-Dominanz für Channel-Lesen | — (impliziter Check ist Default) |
| 6 | Definite Assignment | F |
| 7 | Single-Writer für Outputs und `pub var`; Channel-Richtung; jede `hw`-Adresse höchstens einmal gebunden | F |
| 8 | Maschinen: eindeutige Zustandsnamen, `initial` vorhanden, Ziele existieren, Transitionsblöcke enden mit `->`, Aktionsblock-Einschränkungen (5.5) | F |
| 9 | Fault-Wald azyklisch; `FAULTED` erreichbar; Guards der Transitionen aus `FAULTED` ohne implizite Prüfungen | F |
| 10 | Erreichbarkeit von Zuständen; tote Transitionen (Guard statisch false); Zustände ohne Ausgang außer `FAULTED`/`DONE`-artige | W |
| 11 | Terminierung: `for`-Schranken konstant; Aufrufgraph azyklisch; Block-`step` ≤ 1 pro Instanz und Tick | F |
| 12 | Kostenbudget je Maschine (9.4.3) und Schedulability (7.2); Stack-Schranke | F mit Vorschlag |
| 13 | Simulation: jeder `hw`-Input hat im Sim-Build eine `sim`-Quelle | F |
| 14 | Zeit: `after`/`wait` nicht Vielfaches der Periode; `until` ohne Timeout | W |
| 15 | Outputs ohne Schreiber in irgendeinem Zustand; Inputs, die nie gelesen werden | W |
| 16 | Format-Strings: Platzhalter existieren, Typen formatierbar, Puffergröße | F |
| 17 | Streams: `MAXPT * n_m <= CAP` je Konsument; Konsumentenmenge statisch; Elementtyp zulässig; `wake`-Attribute | F |
| 18 | Muster: Wohlgeformtheit, Mehrdeutigkeitsregel (8.7), Capture-Typen; Record-Muster nur mit konstanten Feldwerten | F |
| 19 | `match` erschöpfend; Variantenfelder vollständig gebunden | F |
| 20 | `send`: statische Summe der Höchstlängen je Aktivierung ≤ `capacity` | F |
| 21 | `at`/`pulse`: Block enthält nur Zuweisungen an skalare Outputs; `T` vom Typ Duration; K_o | F |
| 22 | `idle`: keine `loop:`/Handler in Zustand und Vorfahren; Guards nur über Wake-Quellen, Konstanten, Params, Maschinenvariablen | F |
| 23 | `persist`: POD-Typ, Maschinenebene, nicht in Szenarien; Typ-Hash stabil | F |
| 24 | Shift-Beträge, `as`-Konversionen, `vec`-Indizes, Slices: Range beweisbar oder impliziter Check | W |
| 25 | Zustandslokale und gehobene Variablen (Sequenz-`var`, Captures): Definite Assignment je Eintritt | F |
| 26 | Szenarien schreiben nur `sim`-Outputs; Single-Writer gegenüber Modellmaschinen | F |
| 27 | `every d:` nur in `loop:`; `on`-Handler nur auf deklarierten Streams | F |
| 28 | `at` auf einen Output mit gemessenem `jitter >= T0` („tick-granular", 7.5); deklarierte `jitter`-Anforderung größer als gemessener Wert erfüllt, sonst Fehler (8.1) | W / F |
| 29 | Kampagne: Sweep-Schritt eines in `at` verwendeten Parameters ≥ `2 * jitter` (13.7) | F |
| 30 | `mat`: Formprüfung bei `+ - *`, `inv`/`solve` nur quadratisch, R, C ≤ 16, Indizes in Range (3.11) | F |
| 31 | Native Funktionen: nur die kuratierte Menge (v1), `cost`, `stack` und `total` deklariert, Argumente fester Größe (4.5) | F |
| 32 | Schedulability mit Abort-Phase, klassenweise: `Σ_c (Peak_c + Σ F_m,c) · c_target[c] ≤ T₀ − T_IO` (7.2); `tick_source` existiert in der Hardware-Konfiguration | F |
| 33 | `follows`: Kanten azyklisch; Follower liest nur gefolgte Maschinen frisch; keine frischen Lesevorgänge in Aktionsblöcken der Abort-Phase (7.2). **Warnung**, wenn ein Follower eine gefolgte Größe liest, die auch auf seinem Fault-Pfad vorkommt: Dort gilt Ψ_k statt des frischen Werts, die Bedeutung wechselt also still zwischen Schritt- und Abort-Phase (5.4) | F / W |
| 34 | Dimensionierte Matrizen: Einheitentupel passen (Produkt, Addition, Inverse), Literale bilden ein äußeres Produkt, variable Indizes nur bei gleichen Einheiten (3.11) | F |
| 35 | `tunable param` nicht in Compile-Zeit-Konstanten (Array-Größen, Kapazitäten, `repeat`) (8.4) | F |
| 36 | `check … for d` / `alert … for d`: `d >= P_m`; Rundung auf Perioden mit effektivem Wert (5.6) | W |
| 37 | `len_field`: verweist auf ein vorangehendes Integer-Feld; statische Obergrenze des Feldes (3.7) | F |
| 38 | Einheiten auf Integern: `.to()` nur bei ganzzahligem Faktor, sonst `.to_float()` (3.2) | F |
| 39 | Speicherbudget aller `baremetal`- und `rtos`-Profile: `takt size` (11.5) ≤ `ram`/`flash`/`iram` der Hardware-Konfiguration, aufgeschlüsselt; auf Targets mit XIP-Flash zusätzlich RAM-Residenz von Code und tick-gelesenen Konstanten (12.3); `debounce`-Haltedauer im Report | F |
| 40 | Performance-Lint: `int` ohne Range in Schleifen und Regelpfaden auf 32-Bit-Zielen, mit Kostenanteil (3.4) | W |
| 41 | Performance-Lint: `float = f64` auf Zielen ohne f64-Hardware, mit Kostenanteil der f64-Operationen; Vorschlag `system: float = f32` (4.2) | W |
| 42 | Lint: Matrizen über 16×16 (Kosten n³, Scratch n²·8 Byte) (3.11); `expect_len < N` schwächt Lemma 9.6.1 zur deklarierten Annahme (8.6) | W |
| 43 | Interne Streams: genau ein Schreiber; `send` nur aus dessen Maschine; Kapazitäten wie 17 mit MAXPT = statische Höchstzahl von `send` je Aktivierung des Schreibers (8.6) | F |
| 44 | Jobs: höchstens K_j je Maschine, Argumente fester Größe, Handle zustandslokal oder Maschinenvariable; `duration` deklariert (4.5) | F |
| 45 | `T!E`: Dominanz durch `.ok` oder `check r.ok`; `match` über `OK`/`ERR` erschöpfend (3.8) | F |
| 46 | `layout`: Offsets überlappen nicht, Bitfelder innerhalb ihres Trägerfelds, Konstantenfelder passen zur Breite, Diskriminanten eindeutig (3.7) | F |
| 47 | `inout`: kein Aliasing (ein Argument höchstens einmal, nicht zugleich als weiteres Argument) (3.9) | F |
| 48 | `irreversible`-Outputs: Zuweisung nur in Sequenzen unmittelbar nach `expect`; Szenario-Abdeckung Pflicht (12.7) | F |
| 49 | Edition fehlt (2.5) | W (Zertifizierungsmodus: F) |
| 50 | Reservierte Wörter als Bezeichner; reservierte Membernamen als Feld- oder Variantennamen (2.5) | F |
| 51 | `match` über offene Enums ohne `case _` (2.5) | F |
| 52 | Generics: genau eine Fähigkeit je Typvariable; Instanziierungsgraph azyklisch; `const`-Variablen nur in Typen und `range` (3.12) | F |
| 53 | Gescopte Instanzen: Single-Writer global; keine Instanzen in `idle`-Zuständen; kein (mittelbares) Selbst-Scoping (5.11) | F |
| 54 | `resume` nur an Zuständen mit Kindern; Fault-Ziele werden über `initial` betreten (5.12) | F |
| 55 | Trigger: Guard und Outputs knotenlokal; `arm`/`disarm` nur aus der deklarierenden Maschine (7.5) | F |
| 56 | `property`: nur lesend, alle Zeitoperatoren beschränkt (13.3) | F |
| 57 | `map`: Schlüssel POD mit Gleichheit (3.9) | F |
| 58 | Knoten (12.9): `follows` knotenlokal; `hops` aus der Topologie berechenbar; Knotentick Vielfaches von `system.tick` | F |
| 59 | Treiberstufe (15, v1.2): Ein gepolltes Gerät läuft zwischen zwei Ticks nicht über — `fifo_depth[d] / byte_rate[d] >= P_m + jitter[tick_source] + wcet_poll[d]`, alle vier Größen aus der Hardware-Konfiguration (8.10) beziehungsweise der Konformitätsmessung (13.8). Fehlt eine, ist die Prüfung nicht entscheidbar: Sie verlangt die Messung oder die ausdrückliche Freigabe `with polling = unchecked`, die im Lauf-Header erscheint. Verletzung nennt drei Auswege: Periode senken, Gerät in die TCB geben (12.6), oder DMA statt Polling | F |
| 60 | Channel-Bindung gegen die Hardware-Konfiguration (8.10): Einheit, Skalierung und Range eines `@ hw(…)`-Channels stimmen mit der Konfiguration überein. Ein Programm, das `float[bar]` bindet, während die Konfiguration `psi` führt, ist sonst unentdeckt — die Einheitenrechnung aus 3.2 endet am Channel-Rand. Fehlt die Konfiguration, entfällt die Prüfung (keine Eingabe, kein Urteil) | F |
| 61 | `check c, "…" within d`: Die gerechnete Safe-State-Latenz (Satz 9.4.5) hält die geforderte Frist ein. Verglichen wird in Ticks (`d / T₀`); die Meldung nennt die Aufschlüsselung nach Erkennung, Bestätigung, Fault-Pfad und Commit, weil die Zahl sonst nicht zu verbessern ist | F |
| 62 | `machine … with budget = {ram = …}`: Der gerechnete Speicher der Maschine (11.5, mit Overlay) liegt im deklarierten Budget. `wcet` braucht die Kalibrierung (13.8) und meldet bis dahin seine Stufe | F |
| 63 | Zwei Lints ohne eigene Syntax: (a) `alert` und `check` mit **derselben** Bedingung im selben Block — die Polaritaet ist entgegengesetzt gemeint (5.6), und weil beide Zeilen gleich aussehen, faellt die Verwechslung sonst niemandem auf; (b) ein `profile`, das einen `param` nicht nennt — er nimmt still seinen Default, und das ist beim Lesen nicht von der Absicht zu unterscheiden (4.6) | W |

Die Kombination aus 4, 8, 9, 11, 17–22 und 30–32 ist die konstruktive Form der Sätze in Abschnitt 9.

---

## 11. Compiler-Architektur und Codegen

### 11.1 Komponenten (Rust-Workspace)
```
takt-syntax       Grammatik als Datei (Quelle für Parser, Formatter, Fuzzer, 2.3), Tokenizer (INDENT/DEDENT), Parser, AST, Formatter
takt-sema         Namen, Typen, Einheiten, Intervalle, Flussanalysen, Maschinen-Wohlgeformtheit, Muster → DFA, Prüfungen 1–58 (10)
takt-mir          Mittlere IR: Maschinen als Structs + step-Funktionen, Desugaring von Sequenzen, Kostenmodell, Schedule-Zähler, Speicherbudget (11.5)
takt-interp       Referenzinterpreter über MIR = ausführbare Semantik (9.x); Compile-Zeit-Auswertung (11.3); Orakel-Rahmen (13.9)
takt-llvm         MIR → LLVM IR (inkwell), Targets x86-64, aarch64, thumbv7em, riscv32imac
libtaktm          korrekt gerundete Mathematik, für alle Targets aus derselben Quelle
takt-rt-core      Tick-Schleife, Prozessabbild, Streams, Fault-Wald, Abort-Phase, Zähler, `sched`, Jobs, Recording-Schnittstelle — `no_std`, ohne Allokation
takt-rt-linux     Profilaufsatz `linux_rt` (12.2): Tick-Thread, Treiber-Threads, Telemetrie, Recording, Web-API
takt-rt-baremetal Profilaufsatz `baremetal` (12.3): Tick per Timer-ISR, Watchdog, NVM-Journal, Schlaf
takt-rt-rtos      Profilaufsatz `rtos` (12.8): Takt als höchstpriore Aufgabe unter einem RTOS
takt-rt-boot      Profilaufsatz `boot` (12.8): minimale Runtime für Startprogramme, RAM-Log statt Recorder
takt-hal          Treiber-Traits (Skalar, Stream, geplante Ausgabe, Flash-Gerät), Rand-Selbstprüfungen (12.6), Simulationstreiber — die Sim/HW-Umschaltung (8.3) ist ein Treiberwechsel
takt-native       kuratierte native Funktionen (4.5): DSP-Kerne, Prüfsummen, Hashes, MACs; Konformitätstests
takt-stdlib       Standardbibliothek in Takt selbst (11.4): Blöcke, Funktionen, Protokollpakete, Simulationsmodelle
takt-conformance  Testkorpus, Golden-Traces, Kalibrierung (`c_target`, `guard`, `jitter`), Subnormal-Vektoren, `takt bench` (13.8)
takt-import-c     C-Frontend mit Klassifikation, Abbildungsregeln und Orakel-Modus (13.9; v1.1)
takt-cli          check | sim | run | replay | test | campaign | driver-test | prove | fmt | size | latency | graph | bench | tune | migrate | import-c | mir | parse | tokens
takt-lsp          Editor-Integration, Live-Zustandsanzeige über Telemetrie
```

`mir`, `parse` und `tokens` sind die Entwicklerstufen: Sie geben das Zwischenergebnis einer einzelnen Schicht aus (Tokenstrom, Syntaxbaum als S-Expression, MIR als Text oder Datei). Sie gehören nicht zum Arbeitsablauf eines Anwenders, sind aber die Schnittstelle, an der die differenziellen Tests gegen die Referenzwerkzeuge in `grammar/` ansetzen (13.1).

### 11.2 Lowering einer Maschine (Skizze)
```
struct hotfire_state { conf: [u8; DEPTH], t_in_state: [u64; DEPTH], vars…, state_vars…, blocks…, every_next…, cur: [u64; STREAMS], pending: Option<Fault>, last_fault: Fault, pc: u32 }
fn hotfire_step(s: &mut hotfire_state, i: &Inputs, psi: &Published, o: &mut OutLatch)
```
- Konfiguration als Pfad-Array fester Tiefe; Blattzustand = Enum-Diskriminante; `switch` über die Blätter.
- `loop:`-Körper als Straight-Line-Code; `check` → Vergleich + bedingter Sprung in den Fault-Trampolin der Maschine; `->` → Setzen der Goto-Vormerkung + Sprung ans Kettenende.
- Timer als u64-Tick-Zähler pro aktiver Ebene. Handler-Dispatch als Schleife über das Fenster mit vorkompilierten DFA-Tabellen; `sched`-Warteschlangen als feste Arrays im Runtime-Anteil des Outputs.
- Outputs in einen Latch-Struct; Commit durch die Runtime.
- Optionale Instrumentierung: vor jedem Statement `s.pc = LINE` (ein Store; auf MCU abschaltbar).
- LLVM-Flags: `-O2`, strikte FP (`-fno-fast-math`, `contract=off`), keine Speculation über Fault-Pfade hinweg; die Fault-Pfade sind `cold`.
- **Große Werte.** Werte über einer Schwelle (Default 64 Byte: Records, Arrays, Matrizen, `bytes<N>`, `line<N>`) werden intern per Zeiger übergeben und als Ergebnis in vom Aufrufer bereitgestellten Speicher geschrieben. Kopien entstehen nur bei Zuweisung, und nur dann als echte Kopie, wenn Ziel und ein Operand dieselbe Variable sind (`P = f(P)`); sonst wird direkt in das Ziel geschrieben. Das ist korrekt, weil die Sprache keine Referenzen hat: Jedes Objekt ist eine eigene benannte Speicherstelle, Überlappung ist ein syntaktischer Test. Die Wertsemantik der Sprache bleibt unangetastet.
- **Statischer Scratch.** Temporärwerte eines Ausdrucks liegen nicht auf dem Stack, sondern in einem statischen Scratch je Maschine, dessen Größe der Compiler als Maximum der gleichzeitig lebenden Temporärwerte über alle Statements berechnet und in `.bss` reserviert; die Slots werden über Statements hinweg wiederverwendet. Operatoren mit Zuweisung (`+=`, `*=`) und `transpose()` als Zugriffsmuster reduzieren Temporärwerte weiter.
- **Overlay exklusiver Zustandsspeicher.** Zustandslokale Variablen, gehobene Sequenz-Variablen, Captures, Bestätigungs- und `every`-Zähler von Geschwisterzuständen teilen sich denselben Speicher, rekursiv entlang des Zustandsbaums: Bedarf(Zustand) = eigene Variablen + max über Kinder. Semantik unverändert (Definite Assignment je Eintritt, T6); `persist`- und Maschinenvariablen werden nicht überlagert.
- Ψ ist ein Doppelpuffer nur für `pub var` und Zustände, die eine andere Maschine oder ein Szenario liest (statisch bekannt); nur aufgezeichnete Größen existieren einfach. `publish` ist ein Zeigertausch, `fresh[m]` (7.2) ein Zeiger auf den Schreibpuffer der Maschine — O(1) pro Tick statt O(Größe aller `pub var`). Nur im Programm genutzte Channels liegen im Prozessabbild; reine Aufzeichnungs-Channels (8.2) laufen daran vorbei in den Recorder.
- Instrumentierungsstufen: `--trace = statements` (ein Store pro Statement, Default auf der Box), `--trace = states` (nur Zustandswechsel, Default auf MCUs), `--trace = off`; das Laufzeitprofil (12.8) setzt den Default, der Lauf-Header hält die Stufe fest.
- **DFA-Tabellen.** Bytes, die alle Muster eines Handler-Blocks gleich behandeln, bilden Alphabetklassen (typisch 10–25 statt 256 Spalten; dazu eine 256-Byte-Klassenabbildung je Block); alle Muster der Handler eines Zustands werden zu einem Produkt-DFA vereinigt (Priorität nach Quelltextreihenfolge, wie 8.7 verlangt), sodass jedes Element einmal durchlaufen wird. Beides ist semantikneutral; die Tabellen werden in `takt size` ausgewiesen und liegen auf Zielen mit XIP-Flash im RAM (12.3).

### 11.3 Sim-Build vs. HW-Build
Gleiche MIR für die Logik; Bindungstabelle und Linkmenge (Plant-Modelle) unterscheiden sich.

**Compile-Zeit-Auswertung.** `const`-Ausdrücke dürfen reine `fn` mit beschränkten Schleifen aufrufen (CRC-Tabellen, Stützstellen); der Compiler wertet sie mit dem Referenzinterpreter (13.1) aus, damit Compile-Zeit- und Laufzeitsemantik identisch sind — auch für Fließkomma (`system: float`, 4.2).

**Versionierte Formate.** MIR-Serialisierung, Aufzeichnung (12.5), Hardware-Konfiguration (8.10), Konformitätsbericht (13.8) und Lauf-Header tragen Formatversion, Edition und Compiler-Version; Leser akzeptieren ältere Versionen ihres Formats, Schreiber schreiben die neueste. Die Bytecode-VM (v2) definiert sich als Verbraucher der versionierten MIR — „gleiche MIR, gleiche Semantik" ist damit ein Vertrag.

**Reproduzierbare Builds.** Gleiche Quelle plus gleiche Toolchain-Version ergeben bitidentische Binaries: feste LLVM-Flags, keine Zeitstempel oder Pfade im Binary, deterministische Symbolordnung; der Hash des Binaries steht im Image-Header. Secure-Boot-Signaturpipelines und die Zertifizierung (13.4) setzen das voraus. Der Logik-Hash (MIR ohne Bindungen) identifiziert das Programm in Lauf-Headern.


### 11.4 Standardbibliothek (Auszug, mit Einheitenvariablen)
```
fn clamp[U](x, lo, hi) -> float[U]          fn lerp[U](a, b, t) -> float[U]
fn map_range[U, V](x, x0, x1, y0, y1) -> float[V]      fn deadband[U](x, w) -> float[U]
fn interp[U, V](t: table<U, V>, x) -> float[V]         fn crc16[const N](b: bytes<N>) -> u16 / crc32 / sum8
block lowpass[U](tau)               step(x, dt) -> float[U]
block hysteresis[U](lo, hi)         step(x) -> bool
block debounce(d)                   step(x: bool, dt) -> bool
block rising() / falling()          step(x: bool) -> bool
block integrate[U](limit)           step(x: float[U], dt) -> float[U*s]      # Coulomb-Zaehlung: A*s -> .to(mAh)
block rate[U]()                     step(x, dt) -> float[U/s]
block window_min[U, const N]() / window_max / window_mean / window_rms
block rate_limiter[U](max_rate)     step(target, dt) -> float[U]
block pid[O, E](kp, ki, kd, lo, hi) step(err, dt) -> float[O]
block stopwatch()                   start() / stop() / elapsed -> Duration
block pulse_counter()               step(edges: stream) -> int
block cross_check[U](tol)           step(a, b) -> float[U]?                    # none bei |a - b| > tol
block vote2oo3[U](tol)              step(a, b, c) -> float[U]?                 # Median, wenn zwei Werte innerhalb tol liegen
block hold_last[U](max_hold)        step(x: float[U]?, dt) -> float[U]?        # letzten guten Wert begrenzt halten
block reader[const N](b: bytes<N>)  u8() -> u8? / u16_le() / u32_le() / take(n) -> bytes<M>? / remaining   (3.9)
block writer[const N](buf: bytes<N>) u8(x) -> bool / u16_le(x) / bytes(b) -> bool   (schreibt in buf; 3.9)
block pid_i[O, E](...) / lowpass_i[U](tau)   Integer-Varianten fuer Kerne ohne FPU (3.2; v1.1)
native fn sha256_init / sha256_update(ctx, chunk) / sha256_final      Chunk-Natives mit opakem Sha256Ctx (4.5)
native job ecdsa_p256_verify / rsa3072_verify / aes_gcm_decrypt        Jobs mit duration (4.5)
machine flash_model(sectors, t_erase, t_program, CUT_AT_BYTE)          Simulationsmodell (Maschine mit sim-Outputs) mit Stromausfall-Injektion (8.11)
fn solve(A, b) / inv / det / cholesky / transpose   (3.11; Einheitsmatrizen als Literale)
fn fma[U, V](a: float[U], b: float[V], c: float[U*V]) -> float[U*V]      korrekt gerundet, bitidentisch (4.2)
fn sin_fast / cos_fast / exp_fast / atan2_fast      deterministische Naeherungen mit dokumentierter absoluter Fehlerschranke (4.2)
# Filter- und Reglerbloecke halten abklingende Zustaende mit einem deterministischen Totband (4.2)
native fn fft256 / crc32c / sha256 / hmac_sha256 ...  with cost = ..., stack = ..., total (4.5)
```
Alle Blöcke deklarieren `dt: Duration in tick..1 h`, damit `dt.as(s) > 0` ein Intervall-Fakt ist (3.4).


### 11.5 `takt size`: Speicherbudget
`takt size` berechnet den Speicherbedarf statisch und aufgeschlüsselt: Maschinenzustände (mit Overlay), Streams (Byte- und Deskriptor-Ringe), Prozessabbild und Ψ, `sched`-Warteschlangen, Scratch je Maschine, DFA-Tabellen, tick-gelesene Konstanten, Stack (Programm exakt, Natives per Vertrag, Reserven je Profil, 12.3), Runtime-Reserven, Flash (Code, Konstanten, `persist`-Journal). Jeder Posten trägt seine **Herkunft**: `exakt` (aus der MIR gerechnet), `Vertrag` (aus einer Deklaration übernommen, etwa der `stack`-Vertrag einer nativen Funktion, 4.5), `gemessen` (aus dem erzeugten Objekt gelesen) oder `offen` (die Eingabe fehlt noch, etwa eine Profilreserve ohne Kalibrierung). Summiert werden die exakten, die Vertrags- und die gemessenen Posten; eine Summe, die Geschätztes einrechnet, wäre schlechter als keine, weil ihr niemand ansieht, welchem Teil er trauen kann.

**Warum `gemessen` eine eigene Herkunft ist.** Der Posten *Flash (Code)* ist aus der MIR nicht ableitbar — wie viele Bytes eine Maschine wird, entscheidet der Codegen —, aber er steht auch in keiner Deklaration. Nach dem Linken ist er dennoch **exakt**: Die Sektionsgrößen des Objekts sind keine Schätzung. Ihn als `offen` zu führen wäre darum irreführend, und ihn `exakt` zu nennen verwischte, dass er aus einem anderen Schritt stammt als der Rest. Die Unterscheidung hat eine praktische Folge: `takt size` vor dem Link zeigt den Posten als `offen` und lässt ihn aus der Summe; mit einem Objekt zeigt es ihn als `gemessen` und rechnet ihn ein. Dasselbe gilt für die Konstanten, die der Link in `.rodata` legt. Von der Kalibrierung (13.8) unterscheidet sich das: Eine Profilreserve braucht Hardware und einen Lauf unter Last, eine Sektionsgröße nur den Linker. Für `baremetal`- und `rtos`-Profile ist die Summe gegen `ram`, `flash` und ggf. `iram` der Hardware-Konfiguration (8.10) zu prüfen, sobald diese vorliegt; Überschreitung ist ein Compile-Fehler, der die größten Posten mit Vorschlägen nennt (`expect_len`, kleinere `capacity`, `float = f32`, Matrizengröße). Das ist die konstruktive Form von „Speicherbudgets vor dem Kompilieren kennen": Der Compiler kennt sie.

---

## 12. Runtime

### 12.1 Gemeinsame Struktur
```
loop:
    wait_for_tick_boundary()
    sample_inputs()          # aus Treiber-Puffern: Skalare (Qualität, Alter), Stream-Lieferungen mit Zeitstempeln, free[o]
    validate_and_bound()     # 12.6: Treiberverträge prüfen (-> Runtime(Driver)), Ranges und max_slew erzwingen (-> Bad)
    deliver_streams()
    run_scheduled_machines()  # active(k) per countdown (7.2)
    abort_phase()             # 5.4 / 9.4
    advance_cursors()
    publish()
    apply_scheduled()        # geplante Ausgaben dieses Ticks in den Latch; Treiber schreibt zum Hardware-Zeitpunkt
    commit_outputs()         # asap: sofort; boundary: am nächsten Tick-Anfang; Sendepuffer an Treiber
    record_and_telemeter()   # außerhalb der Semantik, nie blockierend; persist-Journal
    kick_watchdog()
    maybe_sleep()            # 9.9
```

### 12.2 Linux-Box (Cortex-A / x86-64)
- PREEMPT_RT; Tick-Thread `SCHED_FIFO` auf isoliertem Core (`isolcpus`, `nohz_full`), `mlockall`, vorab berührte Seiten, periodischer Timer mit absoluten Deadlines (`clock_nanosleep` TIMER_ABSTIME).
- Treiber (EtherCAT, Modbus, CAN, DAQ-Karten, Custom) in eigenen Threads; Austausch über lock-freie Doppelpuffer (Prozessabbild); keine Syscalls und keine Allokation im Tick-Thread.
- Telemetrie und Recording über SPSC-Ringpuffer an einen Aufzeichnungsthread; Überlauf zählt und meldet, beeinflusst die Steuerung nicht.
- Overrun-Erkennung per Zeitstempel am Tick-Ende (7.3).
- Web-API für Dashboards, Commands mit Berechtigung, Live-Zustandspfad und `pc` je Maschine.
- Stream-Treiber liefern zeitgestempelte Elemente (UART mit Hardware-Zeitstempel je Zeile, CAN-Controller-Zeitstempel, Capture-Timer für Flanken); geplante Ausgaben gehen mit absolutem Zeitpunkt an den Treiber (Timer-Compare, DAQ-Ausgabeplan). `guard` je Output stammt aus der Hardware-Konfiguration.
- Tick-Quelle (7.1): freilaufender Timer oder ein Hardware-Ereignis eines DAQ-/Feldbus-Zyklus (EtherCAT-Distributed-Clock); Periodenabweichung wird gemessen und gegen `tick_tolerance` geprüft.

### 12.3 MCU (Cortex-M, RISC-V; `no_std`)
- Tick aus Hardware-Timer-Interrupt; die ISR setzt ein Flag und sampelt ggf. zeitkritische Inputs; die Hauptschleife führt den Tick aus. Ist das Flag beim nächsten Interrupt noch gesetzt → `Runtime(Overrun)`.
- Gesamter Zustand statisch in `.bss`; kein Heap. **Stack-Zusammensetzung:** Programmanteil exakt aus LLVM-Stack-Usage und azyklischem Aufrufgraphen; native Funktionen mit ihrem `stack`-Vertrag (4.5) als Blattkosten; Runtime und Treiber, ISRs (verschachtelt, auf Cortex-M auf dem Hauptstack) und im `rtos`-Profil der eigene Task-Stack als Reserven je Profil aus der Hardware-Konfiguration, gemessen in 13.8; Gesamt = Programm + Σ Reserven + Marge, vom Compiler geprüft (11.5) und im Linker-Skript reserviert; die **Marge steht wie die Reserven in der Hardware-Konfiguration** (8.10) und ist damit je Target und Projekt wählbar, nicht ein fester Anteil der Sprache — ein Prozentsatz, der für jede Zielklasse und jede Risikoklasse zugleich gälte, wäre entweder zu knapp oder Verschwendung, und er hätte keine Herkunft im Sinne von 11.5. Als Verteidigung in der Tiefe liegt unter dem Stack ein Schutzbereich (MPU-Region ohne Zugriff oder Kanarienwort mit Prüfung am Tick-Ende); eine Verletzung ist `Runtime(Hardware)` und beweist einen TCB-Fehler, nicht einen Programmfehler.
- **Speicherschutz.** Auf Zielen mit MPU liegen Programmzustand, Runtime-Zustand und Treiberpuffer in getrennten Regionen; ein Treiber schreibt nur in seine Puffer. Das Programm braucht keinen Schutz (es kann per Konstruktion nicht außerhalb seiner Objekte schreiben); der Schutz wendet sich gegen die TCB und verwandelt stille Korruption des Programmzustands in einen definierten Fault `Runtime(Hardware)` mit Angabe der Region.
- HAL-Treiber (SPI/I²C/UART/CAN/ADC/PWM) als Rust-Traits mit typsicheren Kanaladressen.
- Hardware-Watchdog; Outputs auf `safe` bei Reset-Ursache Watchdog vor Neustart des Programms.
- WCET-Nachweis: statisches Budget (9.4.3) × kalibrierte Kosten, Verifikation per Zyklenzähler (DWT) in HIL-Läufen; optional externe WCET-Analyse auf dem Binärcode, die durch die schleifenbeschränkte, rekursionsfreie Struktur unproblematisch ist.
- Telemetrie über UART/CAN/Ethernet mit reduziertem Umfang (Zustandspfad, Faults, `pub var`).
- `persist`: journalisierte NVM-Schreibvorgänge (Flash-Sektorwechsel, CRC, Typ-Hash), Rate durch `min_interval` begrenzt.
- Tick-Quelle (7.1): Timer-Update oder PWM-Periode als Tick-Interrupt; `samples`-Kanäle per ADC-DMA in einem auf das Tick-Ereignis ausgerichteten Fenster (Strommessung in der PWM-Mitte); Abweichung der Periode → `Runtime(Hardware)`.
- Schlaf: Systemschlaf nach 9.9 als Low-Power-Modus (WFI/STOP) mit Wake-Quellen als Interrupts; Timer-Capture liefert Zeitstempel für `Edge`-Streams; geplante Ausgaben über Compare-Kanäle.
- **Targets mit XIP-Flash und Cache-Stall (Profilfamilie `xip_flash`).** Auf vielen Ein-Chip-Systemen läuft Code über einen Instruktions-Cache direkt aus dem Flash, und jeder Flash-Schreib- oder Löschzugriff deaktiviert diesen Cache. (1) *RAM-Residenz:* Code, der aus dem Flash läuft, steht während eines Schreibzugriffs still (Sektorlöschung: zweistellige Millisekunden). Tick-Interrupt, Runtime-Hauptschleife und der übersetzte Takt-Code liegen deshalb im Instruktions-RAM, alle im Tick berührten Daten im Daten-RAM — auch *Konstanten*, die der Tick liest: DFA-Tabellen, `const`-Tabellen (`table<A, B>`), Einheitentabellen, `safe`-Werte (Kopie beim Start; `takt size` weist sie aus); das Profil prüft `takt size` statisch gegen die RAM-Größen (10, Zeile 39). (2) *`persist` außerhalb des Ticks:* Das NVM-Journal schreibt eine niedrig priorisierte Aufgabe, während der Tick aus dem RAM weiterläuft — sonst erzeugt jedes Speichern einen `Runtime(Overrun)`. (3) *Schlaf:* Ein Schlafmodus, der den RAM erhält, entspricht `idle` (virtuelle Ticks, Satz 9.9.1); ein Tiefschlaf ohne RAM-Erhalt beendet den Lauf und startet ihn mit `boot_reason = DEEP_SLEEP_WAKE` neu (12.7). (4) *Numerik:* Kerne ohne FPU rechnen mit `int[U]` (3.2) oder mit `system: float = f32` (4.2); das typisierte Kostenmodell (9.4.3) macht jede f64-Operation mit ihrem Kostenanteil sichtbar. (5) *Zeitgeber:* Hardware-Timer als `tick_source`, Timer-Compare-Einheiten für geplante Ausgaben (`jitter` im Mikrosekundenbereich auf Timer-fähigen Pins), ADC-DMA für `samples`; Ereignis-Matrizen der Peripherie (Trigger ohne CPU) eignen sich für einfache Trigger (7.5, v1.2). (6) *Funk:* nur im RTOS-Profil (12.8).

### 12.4 Sicherheitsmechanismen außerhalb der Sprache
Heartbeat vom Tick-Thread zu jedem I/O-Gerät; Geräte setzen Outputs bei Heartbeat-Verlust auf konfigurierte Safe-Werte (identisch zu den `safe`-Deklarationen; der Compiler exportiert sie in die Hardware-Konfiguration) und verwerfen dabei ausstehende geplante Ausgaben. Damit ist der Verlust des Steuerrechners selbst kein unsicherer Zustand — das schließt die Lücke, die kein Sprachbeweis schließen kann.

### 12.5 Recording und Replay
Ein Lauf zeichnet auf: Logik-Hash, Bindungen, Laufzeitprofil (12.8), Profil und Parametervektor, s0 der `persist`-Variablen, alle Input-Snapshots einschließlich Stream-Elementen mit Zeitstempeln und `free[o]`, Commands, Tunable-Änderungen mit Tick (8.4), Job-Fertigstellungen mit Tick und Ergebnis (4.5), Trigger-Ereignisse (7.5); das Format ist versioniert (11.3). `takt replay` führt dasselbe Binär im Sim-Modus mit den aufgezeichneten Inputs aus und vergleicht Outputs und Zustandspfade; Abweichung = Fehler in Runtime oder Treiber, nie in der Logik (Satz 9.4.4).


### 12.6 Defensiver Treiberrand

Vor dem Bilden des Prozessabbilds I_k prüft die Runtime jede Lieferung eines Treibers gegen dessen Vertrag. Leitsatz: **Inputs degradieren, Outputs faulten.** Eine Lieferung, der nicht zu trauen ist, wird zu `Bad` — die Programme entscheiden über ihre Checks und `.or()`, was das bedeutet, und die Erholung ist automatisch; nur wenn das Stellen selbst scheitert, ist das ein Fault für die Besitzer der betroffenen Outputs.

| # | Prüfung | Bei Verletzung |
|---|---|---|
| 1 | Zeitstempel innerhalb `(t_(k-1), t_k]` | innerhalb der Toleranz (Konfiguration, Default 1 Tick): auf das Fenster geklemmt, `s.time_warped` gezählt, Alert; darüber hinaus wie 2 |
| 2 | Zeitstempel und `seq` streng steigend, `seq` lückenlos, `len(D) <= MAXPT` je Stream, Qualitätsflags konsistent (`Bad` ohne Wert, Alter monoton) | alle Input-Channels des Treibers `Bad` mit Grund `Driver`, Alert `DriverDegraded`; Erholung, sobald der Treiber wieder vertragsgemäß liefert |
| 3 | Werte innerhalb deklarierter Ranges (3.5) | Qualität `Bad`, Grund `OutOfRange` (mit `debounce`: zunächst `Suspect`) |
| 4 | `max_slew` (3.5) | Qualität `Bad`, Grund `Implausible` (mit `debounce`: zunächst `Suspect`) |
| 5 | Record-Streams: `decode` erfolgreich (8.6) | Element verworfen, `s.malformed`, Alert |
| 6 | Output-Seite: Schreibvorgang bestätigt, Heartbeat zum Gerät intakt, Sendepuffer vom Treiber nicht überfahren (`free[o] <= capacity`) | `Runtime(Driver)` für die Besitzer der betroffenen Outputs; die Geräte stehen bereits auf `safe` (12.4) |
| 7 | Tick-Periode innerhalb `tick_tolerance` für die konfigurierte Zahl aufeinanderfolgender Ticks (7.1) | `Runtime(Hardware)` für alle Maschinen |

`Runtime(Driver)` wird den Besitzern der betroffenen Outputs vorgemerkt, `Runtime(Hardware)` allen Maschinen; beide wirken im selben Tick — bei aktiven Maschinen zu Beginn ihres Schritts, bei den übrigen in der Abort-Phase (5.4, 9.4). Ein Treiber, der auf der Output-Seite in jedem Tick versagt, eskaliert seine Besitzer über den Fault-Wald nach `FAULTED`; ein Treiber, der nur liefert, stört ohne steuernde Nutzer nichts. Diese Prüfungen machen Treiber nicht korrekt, aber jede Verletzung ihrer Verträge sichtbar — der Übergang von „stille Korruption" zu „definierte Qualität oder definierter Fault" ist der Kern des defensiven Rands. Treiber-Crates enthalten kein `unsafe` außerhalb geprüfter HAL-Schichten und liefern Fuzzing-Ziele für Rahmung und Zeitstempelung; ihre Konformität misst 13.8.

### 12.7 Plattformschnittstelle (System-Channels, v1.1)
Firmware-Validierung, Golden-Image-Fallback und koordinierte Neustarts sind Aufgaben der Plattform (Startstufe, Runtime). Die Sprache braucht dafür nur Sichtbarkeit und Hebel — beides sind gewöhnliche Channels:
```
input  boot_reason   : BootReason @ hw("sys/boot_reason")  # POWER_ON, WATCHDOG, SOFTWARE, DEEP_SLEEP_WAKE, TRIAL
input  image_state   : ImageState @ hw("sys/image_state")  # CONFIRMED, TRIAL
input  reset_count   : int        @ hw("sys/reset_count")
output image_confirm : bool       @ hw("sys/image_confirm") with safe = false
output reboot        : RebootCmd  @ hw("sys/reboot")        with safe = NONE  # NONE, RESTART, DEEP_SLEEP
```
Die Enums `BootReason`, `ImageState` und `RebootCmd` sind vordefiniert; welche Channels eine Plattform anbietet, steht in ihrer Hardware-Konfiguration (8.10).
- **Muster TRIAL → SELFTEST → CONFIRM.** Nach einem Update startet die Plattform das neue Image im Zustand TRIAL; das Programm läuft seinen Selbsttest; erreicht er PASS, setzt es `image_confirm = true`, und die Plattform markiert das Image als gut. Erreicht das Programm vorher `FAULTED` oder greift der Watchdog, bootet die Plattform das vorherige Image (Beispiel 14.7).
- **Neustart aus sicherem Zustand.** `reboot = RESTART` wird nach dem Commit des Ticks ausgeführt, nachdem alle Outputs auf `safe` stehen.
- **Tiefschlaf.** `reboot = DEEP_SLEEP` beendet den Lauf (kein virtueller Tick; Satz 9.9.1 gilt nur für RAM-erhaltenden Schlaf); der nächste Lauf beginnt mit `boot_reason = DEEP_SLEEP_WAKE` und s0 aus `persist` (5.9). Schemaänderungen über Firmware-Grenzen sind durch den Typ-Hash abgedeckt.
- **Start-Channels.**
  ```
  input  efuse           : EfuseBlock @ hw("sys/efuse")                                             # Schluessel-Hashes/-Werte, min_version, Secure-Boot-Flags (vordefiniertes Record)
  input  image_confirmed : [2] bool   @ hw("sys/image_confirmed")                                   # je Slot: vom Anwendungsimage bestaetigt
  output boot_jump       : u8         @ hw("sys/jump")       with safe = NONE                       # Sprung in den Slot; beendet den Lauf nach dem Commit
  output efuse_burn      : EfuseCmd   @ hw("sys/efuse_burn") with safe = NONE, irreversible = true  # einmalig programmierbare Bits
  ```
  Vor `boot_jump`, `reboot` und Deep Sleep schreibt die Runtime ausstehende `persist`-Änderungen synchron (5.9), danach stehen alle Outputs auf `safe`.
- **Irreversible Outputs** (`irreversible = true`): Der Compiler verlangt, dass jede Zuweisung in einer Sequenz unmittelbar auf ein `expect` folgt, das die Voraussetzung prüft, und dass mindestens ein Szenario die Zuweisung abdeckt (13.2); der Lauf-Header nennt alle irreversiblen Outputs.

### 12.8 Laufzeitprofile (v1.1)
`system: target = <profil>` wählt Runtime, Kostentabelle und Plattformregeln; das Profil steht im Lauf-Header.

| Profil | Umgebung | Zeitgarantie |
|---|---|---|
| `linux_rt` | PREEMPT_RT-Box (12.2) | empirisch (Konformitätsmessung), Überschreitung ist Fault |
| `baremetal` | `no_std` auf MCU (12.3) | statisch (Budget × kalibrierte Tabelle) plus Messung |
| `rtos` | Takt als höchstpriore Aufgabe unter einem RTOS (z. B. wenn ein Funkstack ein bestimmtes Betriebssystem verlangt) | empirisch: Tick aus Hardware-Timer-ISR im RAM, Task-Benachrichtigung; Jitter durch Funk-ISRs und kritische Abschnitte (zweistelliger Mikrosekundenbereich, zu messen); bei T₀ ≥ 1 ms und WCET ≪ T₀ tragfähig; Überschreitung ist `Runtime(Overrun)` |
| `boot` | Startprogramme, die vor jeder anderen Software laufen: minimale Runtime (Tick aus einem Timer, UART-Stream, Flash-Gerät nach 8.11, Watchdog, Zeitbasis), kein Recorder außer einem kompakten RAM-Log für Replay, `--trace = off`, Code und Konstanten im RAM, Flash-Budget gegen die Startpartition (11.5) | statisch wie `baremetal`; ein periodischer Tick von 1 ms genügt (30 Sequenzschritte kosten 30 ms, 1 MB Hashen bei 4 KB je Tick 250 ms); ein freilaufender Tick-Modus wurde erwogen und verworfen — zwei Zeitmodelle für Millisekunden Gewinn |

Im `rtos`-Profil gehören das RTOS und die Funktreiber zur Trusted Computing Base; die Sprachgarantien (Sätze 9.4.x) gelten unverändert für das Programm, die Zeitgarantie wird gemessen statt bewiesen. Funkdaten treten als Streams (`stream<bytes<N>>`, Rahmung im Treiber) in das Prozessabbild ein. Bare-Metal-Funk (Funkstack in `no_std` mit eigenem Heap und Zeitgebern) wird nicht unterstützt, weil er nicht deterministisch ist. Für sicherheitsrelevante Produkte wird die Zwei-Chip-Architektur empfohlen: Takt auf einem dedizierten MCU, Funk auf einem Kommunikations-Koprozessor über UART/SPI-Streams — der Funkteil liegt dann vollständig außerhalb der TCB.

**Zielklassen.** Quer zu den Profilen stehen die Zielklassen, die Numerik und Erwartung an die Geschwindigkeit bestimmen:

| Klasse | Beispiele | Register | FPU | Empfehlung `system:` | Erwartung gegenüber C (nach 3.4, 4.2) |
|---|---|---|---|---|---|
| 64-Bit Linux | x86-64, aarch64 (Cortex-A-Klasse) | 64 | f32 + f64, FMA | Defaults (`float = f64`), `linux_rt` | Grundrechenarten, Zustandsmaschinen, DFA gleichauf; Transzendente mit `libtaktm` etwas teurer; implizite Prüfungen im einstelligen Prozentbereich |
| 32-Bit mit f32-FPU | Cortex-M4F, Cortex-M7, RV32IMFC | 32 | f32, FMA; f64 Software | `float = f32`, Ranges deklarieren, `baremetal` | mit `float = f32` und Verengung gleichauf; f64 nur in bewusst gewählten Pfaden |
| 32-Bit ohne FPU | Cortex-M0+/M3, RV32IMAC | 32 | keine | `float = f32` oder `int[U]` (3.2), Ranges deklarieren, `baremetal` oder `rtos` | Integer-Pfade gleichauf; Fließkomma ist Software wie in C — die Kostentabelle macht es sichtbar |
| 32-Bit mit f64-FPU | Cortex-A7/A9 (Linux 32 Bit), Cortex-R | 32 | f32 + f64, FMA; VFP/NEON | `float = f64` möglich; NEON nur für Integer (4.2) | wie 64-Bit für Fließkomma; i64 nur mit Verengung günstig |

Instrumentierungs-Defaults (11.2): `statements` in `linux_rt`, `states` in `baremetal` und `rtos`. Tick-Untergrenze und `T_IO` werden je Zielklasse gemessen (13.8), nicht angenommen.


### 12.9 Verteilte Ausführung (v2): Regeln, die schon heute gelten
```
node io1 @ hw("ethercat/1") with tick = 1 ms  # Knotentick = Vielfaches von system.tick
machine current_ctrl node io1 every 50 us:    # Platzierung; ohne Angabe: Hauptknoten
    initial RUN
    state RUN:
        loop: pass
input  i_u : float[A] @ hw("io1/ai0")  # Adresse nennt den Knoten (wie heute)
```
1. **Eine logische Zeitbasis.** Jeder Knoten hat einen Tick, der ein Vielfaches von `system.tick` ist; die Uhren sind synchronisiert (PTP, Feldbus-Distributed-Clocks), die Abweichung ist gegen `tick_tolerance` geprüft; Verletzung → `Runtime(Node)` für die Maschinen des Knotens. Alternative unabhängiger Uhren je Knoten wurde verworfen, weil sie den globalen Tick und mit ihm Satz 9.4.1 zerstört.
2. **Verzögerung = Hops.** `hops(a, b)` ist die statische Pfadlänge zwischen Knoten in der Topologie der Hardware-Konfiguration (0 für denselben Knoten, sonst ≥ 1). Jeder Lesevorgang über Knotengrenzen — `pub var`, Zustand, Signal, Channel, Stream-Element — sieht den Wert von vor `hops` Basis-Ticks: Ψ wird zu einem Verlauf der Tiefe `max_hops` (Speicher: `max_hops` × Größe der knotenübergreifend gelesenen Größen, statisch). Das ist die Verallgemeinerung des Unit-Delays (Entscheidung 6); eine Kausalitätsanalyse über das Netz bleibt ausgeschlossen.
3. **Outputs auf fremden Knoten** sind erlaubt; der Schreibvorgang wirkt `hops` Ticks nach dem Commit; `at` auf einen fremden Output verlangt `T >= now + guard + hops · tick`. Die `safe`-Logik des Geräts (12.4) ist unverändert.
4. **`follows`** gilt nur innerhalb eines Knotens.
5. **Abort und Runtime-Faults** erreichen andere Knoten als `pending` im Tick `k + hops` und wirken dort in der Abort-Phase; der Operator-Abort wird auf allen Knoten im selben logischen Tick gesampelt, solange die Verbindung steht.
6. **Verbindungsverlust.** Alle Channels des verlorenen Knotens werden `Bad` mit Grund `Node` (Inputs degradieren), die Besitzer von Outputs auf dem Knoten erhalten `Runtime(Node)` (Outputs faulten) — die Regel aus 12.6, auf Knoten angewandt; das Gerät geht über seinen Heartbeat in den sicheren Zustand.
7. **Jobs** laufen auf dem Knoten ihrer Maschine; **Trigger** sind Knotenprogramme (7.5).
8. **Aufzeichnung** je Knoten mit globaler Tick-Nummer; `takt replay` führt zusammen. Logik-Hash je Knoten und Gesamt-Hash; das Deployment ist atomar über alle Knoten (alle oder keiner).
9. **Simulation.** Hops sind Verzögerungen im Simulator; Satz 9.4.4 gilt mit Hops als Teil der Semantik.

Warum diese Regeln schon heute gelten: v1-Programme laufen auf dem Hauptknoten mit `hops = 0` überall, Channel-Adressen sind bereits knotenpräfixiert, und `follows` ist bereits knotenlokal, weil es nur einen Knoten gibt. Nichts an einem v1-Programm muss sich ändern, wenn v2 kommt.

---

## 13. Verifikation, Test, Zertifizierung

### 13.1 Referenzinterpreter und differentielles Testen
Der Interpreter über MIR ist die ausführbare Fassung von Abschnitt 9. Jede Compiler-Version wird gegen ihn getestet: (a) alle Beispiel- und Kundenprogramme mit aufgezeichneten Input-Strömen, (b) zufällig generierte wohlgeformte Programme (Grammatik-Fuzzer), (c) Mutationstests der Fault-Pfade. Übereinstimmung muss bitgenau sein.

### 13.2 Coverage aus der Simulation
Zustands-, Transitions- und Check-Auslöse-Coverage werden pro Sim-Lauf gesammelt; unbesuchte Zustände und nie geprüfte `check`s werden berichtet. Für Testkampagnen ist das die natürliche Definition von „Sequenz vollständig getestet".

### 13.3 Eigenschaftsprüfung (v1.1)

```
property no_ignition_without_fuel: always(igniter implies fuel_main == OPEN)
property abort_recovers: always(hotfire.state == SAFE implies eventually[10 s](hotfire.state == IDLE))
property no_chatter: always(valve == OPEN implies stable[50 ms](valve == OPEN))
property armed_before_fire: always(igniter implies once[1 s](armed))
```
**Sprache.** Eine beschränkte Temporallogik über endlichen Traces: `always(φ)`, `never(φ)`, `eventually[d](φ)`, `stable[d](φ)`, `once[d](φ)`, `implies`, `and`, `or`, `not`. Unbeschränktes `eventually` gibt es nicht — es wäre auf endlichen Traces nicht überwachbar und für k-Induktion unhandlich; reine Invarianten (`always` allein) reichten nicht für Reaktionseigenschaften.

**Semantik** über der Folge der Tick-Rand-Snapshots (committete Outputs, Ψ, Zustände, Inputs des Ticks): `always(φ)` gilt an jedem Tick; `eventually[d](φ)` an Tick k gilt, wenn φ an einem Tick in `[k, k + d/T0]` gilt; `stable[d](φ)` an k, wenn φ an allen Ticks in `[k, k + d/T0]` gilt; `once[d](φ)` an k, wenn φ an einem Tick in `[k − d/T0, k]` galt. Alle Operatoren sind mit Ringpuffern der Länge `d/T0` oder Zählern überwachbar (O(1) je Tick und Operator). Eine Eigenschaft liest nur (Unit-Delay wie Szenarien), schreibt nie und kann keinen Fault auslösen — sie ist Beobachtung im Sinne von 5.6; eine Verletzung ist ein FAIL-Befund (13.5).

**Verwendung.** Standard: Monitor in der Simulation (`takt test`) und Beweisziel für `takt prove` (k-Induktion/BMC über die Schrittfunktion; Zukunftsoperatoren werden zu Zählern in Σ, Export der MIR als Lustre-Knoten oder eigene SMT-Kodierung). Optional auf Hardware als Laufzeitmonitor (`with monitor = true`), dann mit Kosten im Budget (Klassen `i32`/`mem`). Durch Unit-Delay und endliche Zustände ist die Kodierung einfach; unerreichbare Zustände und tote Transitionen sind statisch erkennbar.

**Umgebungsannahmen (`assumption`).** Ohne sie liefert der Modellprüfer physikalisch unmögliche Gegenbeispiele — „Tankdruck springt in einem Tick von 0 bar auf 400 bar" ist formal zulässig und praktisch wertlos; nach dem dritten solchen Befund schaltet ein Team das Werkzeug ab.

```
assumption slew_is_physical: always(abs(tank_p - tank_p.prev) < 5 bar) with monitor = true
```

Eine Annahme beschränkt die **Beweisverpflichtung**, nicht die Typsicherheit — das ist der Unterschied zum abgelehnten `assume` der Intervallanalyse (3.4), das die Laufzeitgarantie unterhöhlen würde. Die Soundness-Schleife schließt dieselbe Konstruktion wie bei `check` gegen `alert`: **Annehmen darf man, beobachten muss man.** Eine Annahme ist zugleich ein Monitor; ihre Verletzung ist ein FAIL-Befund (13.5), nie ein Fault.

**Kanal-Attribute gelten automatisch als Annahmen.** `max_slew`, `debounce`, `max_age` und die deklarierte Range eines Channels stehen bereits in der Quelle und werden vom defensiven Treiberrand (12.6) zur Laufzeit *erzwungen* — der Beweiser darf sie deshalb ohne Zusatzaufwand voraussetzen. Das ist die zweite Auszahlung des Treiberrands und kostet nichts; `assumption` bleibt für den Rest.

**Kompositionalität.** Unter dem Unit-Delay (Entscheidung 6) ist Assume-Guarantee zirkelfrei: Nimmt A in Tick k etwas über B an, ist der Wert von Tick k−1 gemeint, die Induktion läuft also über die Zeit statt über die Komponenten, ohne Fixpunktbildung. Mit `follows` bleibt das gültig, weil die Kanten azyklisch sind (Prüfung 33) — die Induktion folgt dann der topologischen Ordnung innerhalb des Ticks. Praktische Folge: `takt prove` beweist je Maschine, mit den Ψ-Lesevorgängen als freien Variablen unter Annahmen, statt das Gesamtsystem in einen Solver zu werfen.

### 13.4 Zertifizierungspfad
- Die Sprache ist per Konstruktion eine Teilmenge im Sinne von MISRA/JPL Power of Ten (keine Rekursion, keine dynamische Allokation, beschränkte Schleifen, keine Exceptions).
- Traceability: jeder `check`, `expect`, `alert`, `verify` und jede Transition trägt eine stabile ID (Hash aus Pfad und Text) und optional eine Anforderungsreferenz (`check p < LIMIT, "…" req "SR-12"`, v1.2).
- Qualifizierbarer Codegen (SCADE-Vorbild) ist eine spätere Investition; die Architektur (kleine MIR, Referenzinterpreter, differentielles Testen) ist darauf ausgelegt.
- Zielnormen: IEC 61508 (SIL 2–3), IEC 61513 (Nuklear), DO-178C (Ground Support Equipment bis Flight).


### 13.5 Messwerte, Prüfungen ohne Abbruch, Verdikte
```
measure boot_time = m.t - t_power_on          # (Name, Wert mit Einheit, Tick, Zustandspfad) -> Report; mehrfach = Zeitreihe
verify i_dut.max() < 10 mA, "supply not off"  # bei false: FAIL-Befund (Nachricht, Tick, Position); Ausfuehrung geht weiter
verdict pass "recovery ok"                    # explizites Ergebnis
verdict fail "image corrupted"
```
Alle drei sind Beobachtung (5.6): nie ein Fault; ungültige Werte werden als `<invalid>` protokolliert und zählen bei `verify` als Verletzung. Sie sind auch in Produktprogrammen erlaubt (dort landen sie im Betriebslog).

**Lauf-Verdikt** (Verbund-Halbverband, FAIL absorbiert):
```
FAIL          falls ein verify verletzt, ein verdict fail ausgefuehrt oder — bei fault_is_fail = true (Default) —
              ein Fault ein Fault-Ziel erreicht hat
PASS          sonst, falls mindestens ein verdict pass ausgefuehrt wurde
INCONCLUSIVE  sonst
```
Ein Lauf ohne Aussage gilt nie als bestanden.

### 13.6 Szenarien (v1.1)
Ein `scenario` ist eine Maschine für den Simulations-Build: Sie schreibt `sim`-Outputs (Single-Writer gegenüber Modellmaschinen), darf alles lesen (Unit-Delay) und setzt Verdikte.
```
scenario "erase interrupted at sector 3" every 1 ms:
    initial RUN
    state RUN:
        sequence:
            until dut_model.state == ERASING and dut_model.sector == 3 timeout 5 s
            expect brownout_test.state == CUTTING
            until brownout_test.state == DONE timeout 10 s
            verify brownout_test.result == RECOVERED, "expected recovery"
            verdict pass
```
`takt test` führt jedes Szenario als eigenen Sim-Lauf aus, sammelt Verdikte und Coverage (13.2: Zustände, Transitionen, Checks, Handler).

### 13.7 Kampagnen (v1.1)
```
campaign brownout_scan:
    program "supply_interruption.takt"
    profile QUAL
    sweep BROWNOUT_DELAY = 2 ms..500 ms step 250 us
    sweep IMAGE = [IMG_A, IMG_B]
    repeat 2
    stop_on fail
```
Der Laufraum ist das kartesische Produkt der Sweeps mal `repeat`; jeder Lauf ist eine deterministische Funktion seines Parametervektors und der Inputs (9.4.1); das Ergebnis ist eine Tabelle (Parametervektor, Verdikt, Messwerte, Lauf-ID). Ein fehlgeschlagener Lauf ist per `takt replay` exakt reproduzierbar (12.5). Die Runtime ignoriert `campaign`-Blöcke; sie sind Eingabe der CLI. Ein Sweep-Schritt für einen Parameter, der in einer `at`-Anweisung verwendet wird, muss mindestens `2 * jitter` des betroffenen Outputs betragen (7.5); sonst lehnt die CLI die Kampagne ab, weil die Messreihe unterhalb der Hardware-Präzision läge.


### 13.8 Treiber- und Native-Konformität
`takt driver-test <treiber>` führt einen Treiber gegen simulierte Hardware aus und prüft die Verträge aus 12.6 (Zeitstempel, `seq`, Kapazitäten, Qualitätsflags, Ranges, Safe-State bei Heartbeat-Verlust). Mit realer Hardware misst er je Output `guard` und `jitter` (7.5) und je Input die Latenz der Abtastung und schreibt die Werte in die Hardware-Konfiguration, aus der der Compiler seine Annahmen für `at`, Kampagnen und die Latenzformel liest. Für native Funktionen (4.5) prüft die Suite Bit-Gleichheit der Ergebnisse über alle Targets und Panic-Freiheit unter Fuzzing; erst danach wird eine Funktion in die kuratierte Menge aufgenommen. Je Target kalibriert die Suite außerdem die Kostentabelle `c_target[c]` pro Operationsklasse (9.4.3) und misst im `rtos`-Profil den Tick-Jitter bei aktivem Funk (12.8); beide Werte gehen in die Hardware-Konfiguration ein, aus der der Compiler die Schedulability (7.2) prüft. Die Suite prüft außerdem die Subnormal-Behandlung der FPU mit einem Referenzvektor (4.2).

**`takt bench`.** Aussagen über Geschwindigkeit werden gemessen, nicht geschätzt: repräsentative Kerne (PID-Schleife bei 1 kHz, 16-Kanal-Thermoelementprüfung, DFA-Zeilenparsing bei 2 000 Zeilen/s, 4×4-Matrix-Update, CRC32 über 256 Byte, Sequenzschritt mit Fault-Pfad) jeweils als Takt-Programm und als Referenz in C (`clang -O2`, gleiche Semantik inklusive Prüfungen), gemessen je Zielklasse (12.8). Ausgabe: Verhältnis Takt/C je Kern, Anteil der impliziten Prüfungen, kalibrierte `c_target`-Tabelle je Klasse, `T_IO`, Tick-Jitter. Erst diese Tabelle rechtfertigt eine Aussage wie „nahezu C-Geschwindigkeit" — pro Zielklasse und Kern. Die Suite misst außerdem die Stack-Reserven von Runtime, Treibern und ISRs je Profil (Stack-Painting unter Last) und den maximalen Stack-Bedarf jeder nativen Funktion, der als `stack`-Vertrag deklariert wird (4.5, 12.3).


### 13.9 Migration aus C: `takt import-c` und Orakel-Modus (v1.1)
**Klassifikation.** Ein Analysator auf Basis eines C-Frontends ordnet jede C-Funktion einer Klasse zu: *übersetzbar* (keine Rekursion, Schleifen mit erkennbarer Schranke, keine Allokation, keine Zeigerarithmetik jenseits von Array-Indizes, keine Funktionszeiger, kein `volatile`-MMIO), *übersetzbar mit Annotation* (`@bound`, `@range`, `@owner` für globalen Zustand, `@step` für blockierendes Warten), *TCB* (Interrupt-Handler, MMIO, Heap, Kryptographie-Kerne, DMA, Funkstack — bleiben Rust; die Schnittstelle wird als Channel, Stream, Native oder Job vorgeschlagen).

**Abbildungsregeln.**

| C | Takt |
|---|---|
| `struct`, `enum`, `#define`-Konstanten | `record` (+ `layout`), `enum` mit Diskriminanten, `const` |
| `switch`-Zustandsmaschine mit statischem `state` | `machine` mit `state`/`when` |
| `while (!ready) {}` | `until ready timeout d` als Sequenzschritt |
| `for` mit Laufzeitgrenze | `for i in range(MAX): if i >= n: break` (MAX annotiert) |
| globale Variablen | Maschinenvariablen des Besitzers; fremde Leser über `pub var` |
| RTOS-Task / Queue / Event-Group / Mutex | `machine` / interner Stream (8.6) / `signal`, `pub var` / entfällt (Single-Writer) |
| Callback-Tabellen, Ereignis-Dispatcher | `match` über Enum, `on`-Handler |
| Fehlercode-Rückgabe + Out-Parameter | `T!E` (3.8) |
| `memcpy`, `memset`, Parsing | `bytes<N>`, `reader`/`writer`, `inout` (3.9) |
| `printf`; Delay- und Zeitfunktionen | `log`; `wait`, `now` |
| `goto`/Cleanup, `longjmp` | Zustände, Fault-Wald |
| `union`-Type-Punning | `layout`-`decode`/`encode` |
| `malloc`/`free` | statische Pools (`vec<T, N>`, `bytes<N>`) oder TCB |
| `volatile`-Register | Treiber (TCB); später Treiberstufe (15) |

**Grundregel des Importers.** Jede C-Annahme, die der Importer nicht beweisen kann (Index, Division, Range, Schleifenschranke), wird ein `check`. Die Übersetzung ist damit *per Konstruktion* crash-frei; die Zahl der eingefügten Checks misst ihre Qualität und sinkt mit jeder Annotation und jedem Range-Typ.

**Orakel-Modus.** Die ursprüngliche C-Funktion wird als Native in den Simulationslauf eingebunden und Tick für Tick mit der Takt-Übersetzung verglichen — gleiche Inputs, verglichene Outputs und (bei Zustandsmaschinen) Zustandspfade, Abbruch bei der ersten Abweichung mit Tick und Position. Weil Takt deterministisch ist und alles aufzeichnet, ist jede Abweichung reproduzierbar. Das ist die Abnahme jeder migrierten Komponente; danach wird das Orakel entfernt.

**Vorgehen.** Migriert wird komponentenweise, jeweils mit Orakel: zuerst kleine, in sich geschlossene Zustandsmaschinen mit klarer Schnittstelle, dann Sequenzen mit Geräteinteraktion, dann Protokolle über Streams. Was in die Klasse *TCB* fällt, bleibt in der Implementierungssprache der Runtime und wird über Channels, Streams, Natives oder Jobs angebunden.

---

## 14. Beispiele

### 14.1 Zündablauf eines Triebwerksprüfstands mit Interlocks
```
system:
    tick = 1 ms

enum ValveCmd: CLOSED, OPEN

input  tank_p    : float[bar] in 0..100 bar @ hw("daq1/ai0") with max_age = 5 ms
input  chamber_p : float[bar] in 0..300 bar @ hw("daq1/ai1") with max_age = 5 ms
input  lox_temp  : float[K] in 50..400 K    @ hw("daq1/tc0") with max_age = 100 ms
output fuel_main : ValveCmd                 @ hw("plc1/do0") with safe = CLOSED
output lox_main  : ValveCmd                 @ hw("plc1/do1") with safe = CLOSED
output igniter   : bool                     @ hw("plc1/do2") with safe = false

command start
command abort_test
command reset

param CHAMBER_LIMIT : float[bar] in 100..300 bar = 250 bar
param IGNITION_P    : float[bar] in 5..100 bar = 20 bar
param BURN_DURATION : Duration = 3 s
param MIN_TANK_P    : float[bar] in 10..90 bar = 30 bar

machine hotfire:
    fault -> SAFE
    initial ARMED

    loop:  # gilt in jedem Zustand, auch in SAFE
        if abort_test:
            abort "operator abort"

    state ARMED:  # alle Betriebszustände; Interlocks gelten hier
        initial IDLE
        loop:
            check chamber_p < CHAMBER_LIMIT, "chamber overpressure {chamber_p}"
            alert lox_temp > 100 K, "LOX warming"

        state IDLE:
            loop:
                alert start and tank_p <= MIN_TANK_P, "start refused: tank pressure {tank_p}"
            when start and tank_p > MIN_TANK_P:
                log "starting hotfire"
                -> IGNITION

        state IGNITION:
            sequence:
                fuel_main = OPEN
                wait 150 ms
                igniter = true
                until chamber_p > IGNITION_P timeout 500 ms
                lox_main = OPEN
                check chamber_p > IGNITION_P, "flameout"
                wait BURN_DURATION
                -> SHUTDOWN

        state SHUTDOWN:
            enter:
                lox_main = CLOSED
                igniter = false
            after 200 ms:
                fuel_main = CLOSED
                -> DONE

        state DONE:
            when reset: -> IDLE

    state SAFE:  # φ(SAFE) = FAULTED (5.3)
        enter:
            fuel_main = CLOSED
            lox_main = CLOSED
            igniter = false
        when reset and chamber_p < 2 bar: -> IDLE
```
Was hier garantiert ist: Überdruck führt in jedem Betriebszustand innerhalb desselben Ticks nach `SAFE`, bevor ein Output committet wird; in `SAFE` gilt der Überdruck-Interlock nicht mehr (die Outputs sind bereits sicher), ein Operator-`abort` dagegen überall; ein Zündversuch ohne Druckaufbau endet nach 500 ms per `Timeout` in `SAFE`; ein Sensorausfall an `chamber_p` ist ein `SensorFault` → `SAFE`; `abort_test` wirkt sofort; die Sequenz ist in der Telemetrie als Zustandsfolge sichtbar.

### 14.2 Batterie-Thermozyklus mit Profil und Plant-Modell
```
system:
    tick = 10 ms

input  cell_v    : float[V] in 2.0..4.5 V       @ hw("daq2/ai0")     with max_age = 50 ms
input  chamber_t : float[degC] in -60..200 degC @ hw("chamber/pv")   with max_age = 1 s
output heater    : bool                         @ hw("chamber/heat") with safe = false
output cooler    : bool                         @ hw("chamber/cool") with safe = false
output charger   : bool                         @ hw("psu1/enable")  with safe = false

param CYCLE_COUNT : int in 1..1000 = 20
param PEAK_TEMP   : float[degC] in 20..150 degC = 85 degC
param LOW_TEMP    : float[degC] in -50..20 degC = -20 degC
param SOAK        : Duration = 30 min
param MAX_CELL_V  : float[V] in 3.0..4.4 V = 4.25 V

profile QUAL:
    CYCLE_COUNT = 50
    PEAK_TEMP   = 120 degC
    SOAK        = 2 h

machine battery_cycle every 100 ms:
    initial VERIFY

    loop:
        check cell_v < MAX_CELL_V, "cell overvoltage {cell_v}"
        check chamber_t < PEAK_TEMP + 10 K, "chamber overtemperature"

    state VERIFY:
        when cell_v.valid and chamber_t.valid: -> CYCLING

    state CYCLING:
        sequence:
            repeat CYCLE_COUNT:
                step "heat":
                    heater = true
                    cooler = false
                    until chamber_t >= PEAK_TEMP timeout 45 min
                    heater = false
                    wait SOAK
                step "cool":
                    cooler = true
                    until chamber_t <= LOW_TEMP timeout 45 min
                    cooler = false
                    wait SOAK
            -> DONE

    state DONE:
        enter:
            charger = false

# Plant-Modell für die Simulation: nur im Sim-Build gelinkt
output chamber_t_sim : float[degC] @ sim("chamber/pv")
output cell_v_sim    : float[V]    @ sim("daq2/ai0")

const LEAK_COEF : float[1/s] = 0.0005 1/s

machine chamber_model every 100 ms:
    var t : float[degC] = 22 degC
    initial RUN
    state RUN:
        loop:
            var drive : float[K/s] = (0.05 K/s if heater else 0 K/s) - (0.04 K/s if cooler else 0 K/s)
            var leak  : float[K/s] = (t - 22 degC) * LEAK_COEF
            t = t + (drive - leak) * (100 ms).as(s)
            chamber_t_sim = t
            cell_v_sim = 3.9 V
```
Das Modell liest die Outputs `heater`/`cooler` mit Unit-Delay, unterliegt denselben Typregeln (affine Temperatur, Einheiten) und läuft im Simulator schneller als Echtzeit — ein Qualifikationsprofil mit 50 Zyklen à 4 h ist in Sekunden durchsimuliert.

### 14.3 Parametrisierte Maschine mit Instanzen (Ventilregler mit Rückmeldung)
```
enum ValveCmd: CLOSED, OPEN

machine valve_ctrl(cmd: input bool, pos: input float[pct], out: output ValveCmd,
                   travel: Duration) every 10 ms:
    fault -> STUCK
    initial CLOSED_ST

    state CLOSED_ST:
        enter: out = CLOSED
        when cmd: -> OPENING
    state OPENING:
        enter: out = OPEN
        when pos > 95 pct: -> OPEN_ST
        after travel: -> STUCK
    state OPEN_ST:
        when not cmd: -> CLOSING
    state CLOSING:
        enter: out = CLOSED
        when pos < 5 pct: -> CLOSED_ST
        after travel: -> STUCK
    state STUCK:
        enter: out = CLOSED
        when cmd == false and pos < 5 pct: -> CLOSED_ST

input  open_v1 : bool       @ hw("ui/open_v1")
input  pos_v1  : float[pct] @ hw("daq1/ai4") with max_age = 20 ms
output valve_1 : ValveCmd   @ hw("plc1/do4") with safe = CLOSED

instance v1 = valve_ctrl(cmd = open_v1, pos = pos_v1, out = valve_1, travel = 2 s)
```
Single-Writer: `valve_1` darf nur an eine Instanz gebunden sein. Andere Maschinen lesen `v1.state == STUCK` mit Unit-Delay.

### 14.4 Funktionen und Blöcke (Standardbibliothek-Stil)
```
fn clamp[U](x: float[U], lo: float[U], hi: float[U]) -> float[U]:
    return lo if x < lo else (hi if x > hi else x)

block lowpass[U](tau: Duration):
    var y    : float[U] = 0
    var init : bool = false
    step(x: float[U], dt: Duration in tick..1 h) -> float[U]:
        if not init:
            y = x
            init = true
        var a : float = dt.as(s) / (tau.as(s) + dt.as(s))
        y = y + a * (x - y)
        return y

block pid[O, E](kp: float[O/E], ki: float[O/E/s], kd: float[O*s/E], out_lo: float[O], out_hi: float[O]):
    var integral : float[O] = 0
    var prev_err : float[E] = 0
    step(err: float[E], dt: Duration in tick..1 h) -> float[O]:
        var dts : float[s] = dt.as(s)
        integral = clamp(integral + ki * err * dts, out_lo, out_hi)  # Anti-Windup
        var deriv : float[O] = kd * (err - prev_err) / dts
        prev_err = err
        return clamp(kp * err + integral + deriv, out_lo, out_hi)
```
`U`, `O`, `E` sind deklarierte Einheitenvariablen; ein PID von `bar` nach `pct` Ventilöffnung instanziiert `pid(kp = 0.5 pct/bar, …)`. `dt` wird übergeben, damit Blöcke periodenunabhängig bleiben; die Range `tick..1 h` am Parameter macht `dts > 0` zu einem Intervall-Fakt — eine Relation zwischen zwei Variablen könnte die Analyse nicht herleiten (3.4).

### 14.5 Überwachung eines Thermoelement-Feldes
```
input  tcs : [16] float[degC] @ hw("daq1/tc[0:16]") with max_age = 200 ms
param TC_LIMIT : float[degC] in 0..1200 degC = 900 degC

machine tc_guard every 100 ms:
    initial WATCH
    state WATCH:
        loop:
            for i in range(16):
                if tcs[i].valid:
                    check tcs[i] < TC_LIMIT, "TC {i} over limit: {tcs[i]}"
                else:
                    alert true, "TC {i} invalid"
```
Der Index `i` hat Typ `int in 0..15`, der Zugriff ist ohne Laufzeitprüfung; die Schleife hat statische Schranke 16 und kostet 16·N(Körper) im Budget.


### 14.6 Versorgungsunterbrechung mit Live-Log-Auswertung (Streams, Muster, geplante Ausgaben, Verdikte)
```
system:
    tick = 1 ms

output vbus_set : float[V] in 0..6 V     @ hw("psu/vset")     with safe = 0 V
output vbus_en  : bool                   @ hw("psu/enable")   with safe = false
output reset_n  : bool                   @ hw("gpio/dut_rst") with safe = false  # low = Reset aktiv
input  i_dut    : samples<float[A], 100> @ hw("daq1/ai2")     with rate = 100 kHz
input  dut_log  : stream<line<256>>      @ hw("uart0/rx")     with max_rate = 2000 Hz, framing = lines, overflow = fault
output dut_tx   : stream<u8>             @ hw("uart0/tx")     with max_rate = 11520 Hz, capacity = 256

command start
command reset

param VBUS           : float[V] in 3..5.5 V = 5 V
param BROWNOUT_DELAY : Duration in 2 ms..2 s = 20 ms
param IMAGE_SIZE     : int in 1..1_000_000 = 65536
param IMAGE_CRC      : int in 0..0xFFFF_FFFF = 0x1234_5678
param BOOT_TIMEOUT   : Duration = 2 s

enum Result: NONE, RECOVERED, BRICKED, CORRUPT

machine brownout_test:
    fault -> SAFE
    pub var result : Result = NONE
    var t_power_on : Duration = 0 s
    initial IDLE

    state IDLE:
        enter:
            vbus_en = false
            reset_n = false
        when start: -> POWER_ON

    state RUNNING:
        initial POWER_ON
        loop:
            check i_dut.max() < 1.5 A, "DUT overcurrent {i_dut.max()}"
        on dut_log has "PANIC" as ev:
            verdict fail "bootloader panic: {ev.text}"

        state POWER_ON:
            sequence:
                vbus_set = VBUS
                vbus_en = true
                t_power_on = now
                wait 50 ms
                reset_n = true
                until dut_log matches "Boot v{major:int}.{minor:int}" as m timeout BOOT_TIMEOUT
                measure boot_time = m.t - t_power_on
                verify m.major >= 2, "bootloader too old: {m.major}.{m.minor}"
                -> HANDSHAKE

        state HANDSHAKE:
            sequence:
                send dut_tx, "UPDATE {IMAGE_SIZE} {IMAGE_CRC:hex}\n"
                until dut_log matches "READY" timeout 500 ms
                -> UPDATING

        state UPDATING:
            var erased : int in 0..255 = 0  # zustandslokal, bei Eintritt 0
            on dut_log matches "Erasing sector {n:int}" as m:
                erased = m.n
                measure erase_seen_at = m.t - t_power_on
                at m.t + BROWNOUT_DELAY:  # Hardware-genauer Zeitpunkt relativ zur Logzeile
                    vbus_en = false
                -> CUTTING
            on dut_log has "CRC mismatch" as ev:
                verdict fail "CRC mismatch during update: {ev.text}"
            after 10 s:
                verdict fail "no erase observed within 10 s"
                -> DONE

        state CUTTING:
            sequence:
                wait BROWNOUT_DELAY + 20 ms
                verify i_dut.max() < 10 mA, "supply still present during brownout: {i_dut.max()}"
                wait 500 ms
                reset_n = false
                vbus_en = true
                wait 50 ms
                reset_n = true
                -> RECOVERY

        state RECOVERY:
            sequence:
                until dut_log matches "Boot v{major:int}.{minor:int}" as m timeout BOOT_TIMEOUT else:
                    result = BRICKED
                    verdict fail "no boot after brownout"
                    -> DONE
                until dut_log matches "Recovery: {outcome:word}" as r timeout 5 s else:
                    result = CORRUPT
                    verdict fail "no recovery message"
                    -> DONE
                if r.outcome == "OK":
                    result = RECOVERED
                    verdict pass "recovered after brownout at {BROWNOUT_DELAY}"
                else:
                    result = CORRUPT
                    verdict fail "recovery state {r.outcome}"
                -> DONE

    state DONE:
        enter:
            vbus_en = false
        when reset: -> IDLE

    state SAFE:
        enter:
            vbus_en = false
            reset_n = false
        when reset: -> IDLE

campaign brownout_scan:
    program "supply_interruption.takt"
    sweep BROWNOUT_DELAY = 2 ms..400 ms step 250 us
    repeat 2
    stop_on fail
```
Der Überstrom-Interlock und der Panic-Handler gelten in allen Unterzuständen von `RUNNING`. Ein Fault (Überstrom, Sensorausfall, Stream-Überlauf) führt nach `SAFE` und verwirft den geplanten Abschaltzeitpunkt. Die Untergrenze 2 ms von `BROWNOUT_DELAY` folgt aus der Latenzformel (7.5) für die Box; die Präzision des Abschaltens ist die des PSU-Treibers. Jeder Lauf der Kampagne ist durch seinen Parametervektor reproduzierbar.

### 14.7 Batteriemanagement mit Selbsttests (MCU; Persistenz, Schlaf, Bit-Operationen, Tabellen)
```
system:
    tick = 10 ms

input  cell_v        : [4] float[V] in 2.0..4.5 V @ hw("afe/cell[0:4]")     with max_age = 50 ms
input  pack_i        : float[A] in -10..10 A      @ hw("afe/current")       with max_age = 50 ms
input  temp          : [2] float[degC]            @ hw("afe/ntc[0:2]")      with max_age = 200 ms
input  charger       : bool                       @ hw("gpio/vbus_det")     with wake = true
input  button        : stream<Edge>               @ hw("gpio/btn")          with max_rate = 50 Hz, wake = true
input  chg_status    : u8                         @ hw("i2c1/0x6B/0x0B")    with max_age = 500 ms
input  image_state   : ImageState                 @ hw("sys/image_state")  # 12.7
output image_confirm : bool                       @ hw("sys/image_confirm") with safe = false
output fet_chg       : bool                       @ hw("gpio/fet_chg")      with safe = false
output fet_dis       : bool                       @ hw("gpio/fet_dis")      with safe = false
output load_test     : bool                       @ hw("gpio/test_load")    with safe = false
output led           : u8                         @ hw("pwm/led")           with safe = 0

param V_MAX     : float[V] in 4.0..4.3 V = 4.2 V
param V_MIN     : float[V] in 2.5..3.2 V = 3.0 V
param I_MAX     : float[A] in 1..10 A = 6 A
param T_MAX     : float[degC] in 40..70 degC = 60 degC
param R_INT_MAX : float[mohm] in 20..500 mohm = 150 mohm

const OCV : table<float[V], float[pct]> = [(3.0 V, 0 pct), (3.4 V, 10 pct), (3.6 V, 30 pct),
                                           (3.8 V, 60 pct), (4.0 V, 85 pct), (4.2 V, 100 pct)]

record SelftestResult:
    passed : bool
    code   : int in 0..255
    r_int  : float[mohm] in 0..1000 mohm

machine bms every 100 ms:
    fault -> PROTECT
    persist var cycle_count : int in 0..100000 = 0 with min_interval = 10 s
    persist var fault_count : int in 0..100000 = 0 with min_interval = 10 s
    persist var last_test   : SelftestResult = SelftestResult(passed = false, code = 0, r_int = 0 mohm)
    pub var soc             : float[pct] = 0 pct
    pub var charge_as       : float[A*s] = 0 A*s
    var coulomb             = integrate[A](limit = 72000 A*s)
    initial SELFTEST

    state ACTIVE:  # Interlocks fuer alle Betriebszustaende
        loop:
            for i in range(4):
                check cell_v[i] < V_MAX + 0.05 V, "cell {i} overvoltage {cell_v[i]}"
                check cell_v[i] > V_MIN - 0.10 V, "cell {i} undervoltage {cell_v[i]}"
            check abs(pack_i) < I_MAX, "overcurrent {pack_i}"
            for i in range(2):
                check temp[i] < T_MAX, "overtemperature {temp[i]}"
            check not chg_status.bit(7), "charger IC fault flag"
            soc = interp(OCV, cell_v.min())
            charge_as = coulomb.step(pack_i, 100 ms)

        state SELFTEST:
            sequence:
                led = 128
                if cell_v.max() - cell_v.min() > 0.1 V:
                    last_test = SelftestResult(passed = false, code = 2, r_int = 0 mohm)
                    -> DEGRADED
                var v0 = cell_v.mean()
                load_test = true
                wait 200 ms
                var v1 = cell_v.mean()
                var i1 = pack_i
                load_test = false
                var r_int = ((v0 - v1) / max(i1, 0.1 A)).to(mohm)
                if r_int > R_INT_MAX:
                    last_test = SelftestResult(passed = false, code = 1, r_int = r_int)
                    -> DEGRADED
                last_test = SelftestResult(passed = true, code = 0, r_int = r_int)
                if image_state.or(CONFIRMED) == TRIAL:  # neues Image nur nach bestandenem Selbsttest bestaetigen; ohne Plattformangabe: bestaetigt
                    image_confirm = true
                -> RUN

        state RUN:
            loop:
                fet_dis = true
                fet_chg = charger and cell_v.max() < V_MAX
                led = round(soc / (100 pct) * 255) as u8
                if rising(charger):
                    cycle_count += 1
            when not charger and abs(pack_i) < 0.05 A: -> IDLE_WAIT

        state IDLE_WAIT:
            when charger or abs(pack_i) >= 0.05 A: -> RUN
            after 2 min: -> STANDBY

        state DEGRADED:
            loop:
                fet_chg = false
                fet_dis = true
                led = 8
            after 24 h: -> SELFTEST

    state STANDBY idle:  # kein loop in Zustand und Vorfahren; nur Wake-Quellen
        enter:
            fet_chg = false
            fet_dis = false
            led = 0
        when button matches Edge(rising = true) as e: -> SELFTEST
        when charger: -> SELFTEST
        after 7 d: -> SELFTEST

    state PROTECT:  # Fault-Ziel; phi(PROTECT) = FAULTED
        enter:
            fet_chg = false
            fet_dis = false
            led = 255
            fault_count += 1
        when fault_count > 20: -> FAULTED
        after 10 s: -> SELFTEST
```
`var` in einer Sequenz wird zur zustandslokalen Variablen gehoben (6.2), damit `v0` über die Zeitgrenze `wait 200 ms` hinweg lebt. `.to(mohm)` ist erlaubt, weil `V/A` und `mohm` dieselbe Dimension haben (3.2). Die Persistenz überlebt Neustarts; `fault_count` begrenzt Neustartschleifen über `-> FAULTED`. Nach einem Firmware-Update bestätigt das Programm das neue Image erst nach bestandenem Selbsttest (`image_confirm`, 12.7); scheitert der Selbsttest oder greift der Watchdog, bootet die Plattform das vorherige Image. Im Zustand `STANDBY` schläft die MCU; der Übergang auf die Tastenflanke ist ein Stream-Guard über eine Wake-Quelle (5.10), Interlocks gelten dort bewusst nicht — ein Übertemperatur-Weckereignis wäre als Wake-Quelle (Komparator) zu deklarieren.


### 14.8 Image-Auswahl und -Validierung beim Start (Startprofil; Chunk-Hash, Job, Kommando + Status, Persistenz)
```
system:
    tick   = 1 ms
    target = boot
    float  = f32

unit sector = 4 KiB

enum FlashCmd: NONE, ERASE(sector: u32), PROGRAM(addr: u32[B], len: u32[B] in 1 B..4096 B), READ(addr: u32[B], len: u32[B] in 1 B..4096 B)
enum FlashStatus: IDLE, BUSY, DONE, ERROR(code: u8)
enum HeaderErr: MAGIC, SIZE, VERSION

record ImageHeader layout little:
    magic   : u8 = 0xE9
    version : u32
    size    : u32[B]
    entry   : u32[B]
    hash    : bytes<32>
    sig     : bytes<64>

input  flash_status    : FlashStatus         @ hw("flash/status") with max_age = 10 ms
input  flash_rx        : stream<bytes<4096>> @ hw("flash/rx")     with max_rate = 200 Hz, capacity = 2
output flash_cmd       : FlashCmd            @ hw("flash/cmd")    with safe = NONE
input  efuse           : EfuseBlock          @ hw("sys/efuse")  # pubkey: bytes<64>, min_version: u32, ... (12.7)
input  image_confirmed : [2] bool            @ hw("sys/image_confirmed")
output boot_jump       : u8                  @ hw("sys/jump")     with safe = NONE
# log geht im Startprofil an die UART der Runtime (12.8)

const SLOT_BASE  : [2] u32[B] = [0x10000 B, 0x110000 B]
const HEADER_LEN : u32[B] = 109 B
param MAX_TRIALS : int in 1..5 = 3

fn parse_header(b: bytes<4096>, min_version: u32) -> ImageHeader!HeaderErr:
    var h = ImageHeader.decode(b)
    if not h.valid: return ERR(MAGIC)
    if h.size > 1 MiB: return ERR(SIZE)
    if h.version < min_version: return ERR(VERSION)
    return OK(h)

machine bootloader:
    fault -> HALT
    persist var trials : [2] int in 0..5 = [0, 0]
    persist var active : int in 0..1 = 0
    var idx            : int in 0..1 = 0
    var hdr            : ImageHeader!HeaderErr = ERR(MAGIC)
    var img            : ImageHeader = default
    var ctx            : Sha256Ctx = sha256_init()
    var done           : u32[B] = 0 B
    initial SELECT

    state SELECT:
        enter:
            idx = active
            for i in range(2):
                if image_confirmed[i].or(false):  # bestaetigte Images setzen ihren Zaehler zurueck
                    trials[i] = 0
            if trials[idx] >= MAX_TRIALS and trials[1 - idx] < MAX_TRIALS:
                idx = 1 - idx  # Fallback auf den anderen Slot
            if trials[idx] < 5:
                trials[idx] += 1  # Versuch zaehlt vor dem Sprung; persist wird vor boot_jump geschrieben
        when trials[idx] <= MAX_TRIALS: -> READ_HEADER
        when true: -> HALT

    state READ_HEADER:
        enter:
            flash_cmd = READ(addr = SLOT_BASE[idx], len = 4096 B)
        when flash_rx as c:
            hdr = parse_header(c.data, efuse.min_version)
            flash_cmd = NONE
            -> CHECK_HEADER
        after 50 ms: -> SLOT_FAILED

    state CHECK_HEADER:
        when hdr.ok:
            img = hdr  # durch hdr.ok dominiert: implizites Auspacken
            ctx = sha256_init()
            done = 0 B
            -> HASHING
        when true:
            log "slot {idx}: bad header ({hdr.err})"
            -> SLOT_FAILED

    state HASHING:  # ein Chunk je Tick; Budget statisch
        loop:
            if flash_status == IDLE and done < img.size:
                flash_cmd = READ(addr = SLOT_BASE[idx] + HEADER_LEN + done, len = min(4096 B, img.size - done))
        on flash_rx as c:
            ctx = sha256_update(ctx, c.data)
            done += (c.data.len as u32) * (1 B)
            flash_cmd = NONE
        when done >= img.size: -> VERIFY
        after 5 s: -> SLOT_FAILED

    state VERIFY:
        fault -> SLOT_FAILED
        sequence:
            var digest = sha256_final(ctx)
            expect digest == img.hash, "hash mismatch in slot {idx}"
            job v = ecdsa_p256_verify(key = efuse.pubkey, digest = digest, sig = img.sig)
            until v.done timeout 500 ms -> SLOT_FAILED
            expect v.result.or(false), "signature invalid in slot {idx}"
            -> JUMP

    state JUMP:
        enter:
            active = idx
            log "booting slot {idx}, version {img.version}"
            boot_jump = idx as u8  # beendet den Lauf nach dem Commit; persist zuvor synchron geschrieben

    state SLOT_FAILED:
        enter:
            flash_cmd = NONE
            log "slot {idx} unusable"
        when trials[1 - idx] < MAX_TRIALS:
            idx = 1 - idx
            if trials[idx] < 5:
                trials[idx] += 1
            -> READ_HEADER
        when true: -> HALT

    state HALT:
        enter:
            log "no bootable image"  # Watchdog der Plattform loest den Neustart aus
```
Was hier zusammenkommt: Der Header ist ein Drahtformat mit Konstantenfeld und Byte-Einheiten (3.7, 3.2); `parse_header` liefert `T!E` (3.8); Flash ist ein Gerät mit Kommando und Status (8.11); der Hash läuft als Chunk-Native mit einem Chunk je Tick, die Signaturprüfung als Job, dessen Fertigstellung ein Input ist (4.5); Versuchszähler und aktiver Slot überleben Neustarts (5.9) und werden vor dem Sprung synchron geschrieben (12.7); jeder Fehlerpfad endet in `SLOT_FAILED` oder `HALT`, nie in einem undefinierten Zustand. Die Stromausfallsicherheit des Schreibpfads (hier nicht gezeigt: Update über UART mit `PROGRAM`) prüft eine Kampagne über `CUT_AT_BYTE` des Flash-Modells (8.11).

---

## 15. Stufenplan und Erweiterungs-Roadmap

Die Stufen halten das Langfristbild an einem Ort. Alles in v1 ist der Kern, ohne den weder die analoge (Prüfstände, Thermokammern, Regelung) noch die digitale Hälfte der Anwendungen (Firmware-Tests, Protokolle, Logs) schreibbar wäre.

| Stufe | Inhalt | Begründung / Bedingung |
|---|---|---|
| **v1 (Kern)** | Alles aus Abschnitt 1–9 ohne Stufenvermerk: Maschinen, Sequenzen, Einheiten, Ranges, Qualität, Fault-Wald, Multirate; Ereignisströme mit Cursor-Semantik, typisierte Muster, `on`-Handler, `until ... matches`, Ausgabeströme; Records, Summentypen, `match`, `T?`, `bytes`/`vec`/`line`/`table`, Bit-Operationen, Konversionen; `at`/`pulse`/`cancel`; `measure`/`verify`/`verdict`; zustandslokale `var`, `every`, `signal`, `break`, Instanz-Arrays, Parameter-Defaults; Korrekturen B6–B16; Abort-Phase (5.4), defensiver Treiberrand (12.6), Zähler-Scheduling (7.2), `tick_source` (7.1), `mat<R, C>` (3.11), kuratierte native Funktionen (4.5), `jitter`/`max_slew` (7.5, 3.5); degradierender Treiberrand mit `debounce`/`Suspect` (3.5, 12.6), Bestätigungszeit `check … for d` (5.6), `tick_tolerance … for N` (7.1), typisiertes Kostenmodell (9.4.3), `reader`/`writer` (3.9), XIP-Flash-Regeln (12.3); Darstellungsverengung (3.4), `system: float` (4.2), `fma` und Schnellvarianten (11.4), IEEE-Modus-Regel (4.2), Zielklassen (12.8), `takt bench` (13.8); Byte-Ringe (8.6), Zeigerübergabe/Scratch/Overlay (11.2), `takt size` mit Speicherbudget (11.5), Stack-Zusammensetzung und Schutzbereiche (12.3); interne Streams (8.6), Jobs und Chunk-Natives (4.5), `T!E` (3.8), `layout`-Details und `default` (3.7), Byte-Einheiten (3.2), Kommando+Status mit Flash-Modell (8.11), `inout`, Bereichsmuster, `w.fmt` (3.9), Journal-Anforderungen (5.9), Compile-Zeit-Auswertung und reproduzierbare Builds (11.3) | Ohne diese Stufe ist die digitale Hälfte der Anwendungen (Firmware, Protokolle) nicht schreibbar; die späteren Ergänzungen schließen Latenz-, Soundness- und Robustheitslücken. |
| **v1.1** | `scenario`, `campaign`/`sweep` (13.6, 13.7); Konstantenvariablen in Generics (3.12); `persist var` (5.9); `idle`-Zustände mit Wake-Quellen und Systemschlaf (5.10, 9.9); Eigenschaftssprache `property` + BMC (13.3); Standardbibliothek vollständig (11.4); beschränktes `map<K, V, N>` (offene Adressierung über festes Array, worst case O(N), deterministische Iterationsreihenfolge) für Nachrichtentabellen; dimensionierte Matrizen `mat[R, C]`/`unitvec` (3.11); `tunable param` (8.4); `follows` (7.2); Oktagon-Analyse (3.4); Einheiten auf Integern (3.2); `len_field` (3.7); Geräteprofile (8.10); System-Channels (12.7); Laufzeitprofile inkl. `rtos` und `boot` (12.8); Start-Channels und irreversible Outputs (12.7); Projekt-Natives (4.5); `takt import-c` und Orakel-Modus (13.9) | Ergänzt Test-Workflow, Feldgeräte, Inbetriebnahme und Migration; jede Position ist unabhängig von den anderen. |
| **v1.2** | Gescopte Instanzen (5.11), `resume` (5.12), Trigger mit `arm`/`disarm`/`fired` (7.5), `capture<T, N>` (8.9), Generics über Typen (3.12); Treiberstufe: `port … @ mmio(…)` mit Registerrecords aus Bitfeldern, Zugriffe sofort und in Programmordnung (nicht am Commit), nur in `driver machine`, Budgetklasse `mem` mit gerätespezifischer Latenz, Gerätemodell in der Simulation Pflicht — Totalität bleibt, Hardware-Korrektheit ist Sache der Gerätemodell-Szenarien; damit können einfache Treiber (GPIO, UART-Polling, SPI-Flash-Kommandos) in Takt geschrieben werden; Startprogramme der ersten Generation behalten den Flash-Treiber in der Runtime. Beschränkte QP-Löser für modellprädiktive Regelung (feste Iterationszahl, deklarierte Kosten) als Bibliotheksbausteine; Trigger auf I/O-Knoten (7.5); `capture<T, N>` (8.9); Anforderungsreferenzen `req` (5.8, 13.4); History-Zustände (`resume`, gespeicherter Blattpfad); nutzerdefinierte native Funktionen mit Signatur- und Review-Prozess (4.5). Der frühere Punkt „instantane Kommunikation (`direct`) mit Kausalitätsanalyse" ist durch `follows` (v1.1) ersetzt. | Braucht Treiberarbeit; Natives erweitern die TCB und brauchen deshalb einen Prozess. |
| **v2** | Verteilte Ausführung nach den Regeln in 12.9 (gelten schon heute; Box + MCU-Knoten): LET-Semantik über synchronisierte Uhren (PTP/EtherCAT-DC), Unit-Delay pro Netzhop, Trigger als Knotenprogramme; deterministische Bytecode-VM als zweites Backend nur für Logik-Updates ohne Reflash (gleiche MIR, gleiche Semantik) | Verteilte Runtime; Semantik bleibt die von 9.4 mit Netz-Delays als zusätzlichen Unit-Delays. |
| **v3** | Qualifizierbarer Codegen (SCADE-Vorbild) und Tool-Qualifikation für IEC 61508 / DO-178C (13.4) | Zertifizierungskunde; Architektur (kleine MIR, Referenzinterpreter, differentielles Testen) ist darauf ausgelegt. |
| **entschieden (früher „offen")** | Parallele Regionen → gescopte Instanzen (5.11, v1.2); Generics über Typen → Klammerklassen mit Monomorphisierung (3.12, v1.2); Operator-Metadaten → Attribute `label`, `display`, `group`, `doc` (2.5, v1.1) | Grammatik und Semantik sind festgelegt; die Implementierung folgt den Stufen. |

Nicht-Ziele bleiben (0.3): Turing-Vollständigkeit im Tick, dynamische Datenstrukturen, Threads, Exceptions, Reflection, Hot-Swap im Lauf, Objektorientierung.

---

## 16. Entscheidungslog

| Frage | Optionen | Entscheidung | Begründung |
|---|---|---|---|
| Ausführungsmodell | Einzeltakt · Multirate synchron · ereignisgesteuert · diskretes Ereignismodell mit logischer Zeit | **Multirate synchron** | PLC-Determinismus, WCET-freundlich, verständlich; Ereignisse werden per Tick gesampelt |
| Zustandsmaschinen | flach · hierarchisch · Statecharts mit AND-States/History | **hierarchisch, ohne AND/History** | Interlock-Vererbung ohne Duplikation; AND-States durch parallele Maschinen abgedeckt |
| Transitionssemantik | starke Transitionen mit Kausalitätsanalyse · schwache Transitionen | **schwach + starke Faults + Entry-Tick-Regel** | keine instantanen Zyklen, keine Kausalitätsanalyse, Entry-Checks vor Commit |
| Transitionspriorität | inner-first (UML) · outer-first (Stateflow/SCADE) | **outer-first** | globale Sicherheitsbedingungen überstimmen lokalen Fortschritt |
| Fault-Behandlung | Exceptions · Result-Typen · Zustandsübergänge | **Zustandsübergänge im Fault-Wald** | für Techniker sichtbar, statisch terminierend, Safe-State eingebaut |
| Kommunikation zwischen Maschinen | instantan mit Sortierung · Unit-Delay · Nachrichten | **Unit-Delay** | Reihenfolge irrelevant (Satz 9.4.1), 1 Tick Latenz akzeptabel |
| Output-Timing | ASAP · LET/boundary | **ASAP, boundary optional** | Latenz vor Jitter im Testbetrieb; jitterfreie Variante für Regelung verfügbar |
| Zeit | Float-Sekunden · Integer-Ticks · Integer-Nanosekunden | **Integer-ns, Timer als Tick-Zähler** | exakt, targetunabhängig, simulierbar |
| Integer | i32 · i64 · targetabhängig | **i64, schmale Typen explizit** | eine Semantik überall; Determinismus vor Speicherersparnis |
| Overflow | wrapping · saturating · checked | **checked → Fault** | laut und sicher scheitern; Intervallanalyse eliminiert die meisten Prüfungen |
| Float-Sondwerte | IEEE-Propagation · Fault | **Fault bei nicht-endlichem Ergebnis** | kein NaN in Aktuator-Kommandos; Vergleiche bleiben total |
| Cross-Target-Reproduzierbarkeit | Best effort · eigene libm korrekt gerundet | **eigene korrekt gerundete libm** | Eindeutigkeit korrekt gerundeter Ergebnisse |
| Einheiten | keine · automatische Konversion · nominal | **nominal, explizit `to()`** | sichtbare Konversionen, entscheidbare Inferenz |
| Temperaturen | Rohzahlen · affine Typen | **affin (Punkt/Vektor)** | mathematisch korrekt, verhindert `degC + degC` |
| Ranges | keine · volle Refinement-Typen mit SMT · Intervalle | **Intervallanalyse mit impliziten Prüfungen + Warnung** | vorhersagbar, erklärbar, kein Solver im Compile-Pfad |
| Sensorqualität | Option-Typen · dreiwertige Logik · implizite Prüfung | **impliziter `valid`-Check mit Dominanzanalyse** | sicherer Default ohne Mehrarbeit |
| Strings | keine · dynamisch · feste Puffer | **feste Puffer, geprüfte Formatierung** | erschlägt den Python-Interpolationsfehler zur Compile-Zeit |
| Zustandsbehaftete Bausteine | Closures · Objekte · `block` mit `step` | **`block`** | Operatorsemantik synchroner Dataflow-Werkzeuge, statisch instanziiert |
| Sequenzen | eigenes Konstrukt mit eigener Semantik · Zucker | **Zucker mit formalem Desugaring** | eine Semantik, zwei Schreibweisen |
| Profile | Rebuild · Ladezeit-Params | **`param` + `profile`, ladezeitvalidiert** | Profilwechsel ohne neuen Code; Determinismus pro Lauf |
| Simulation | separates Modellformat · dieselbe Sprache | **Plant-Modelle als Maschinen, `sim`-Bindung** | gleiche Garantien, gleiche Zeit, HIL-Umschaltung nur über Bindung |
| Backend | Interpreter · Bytecode-VM · AOT/LLVM | **AOT/LLVM, VM als spätere Option** | Performance, WCET, Multi-Target; VM nur für Update-ohne-Reflash |
| MCU-Strategie | Subset-Sprache · gleiche Sprache, andere Runtime | **gleiche Sprache, `no_std`-Runtime** | Satz 9.4.4; ein Programm für Prüfstand und Gerät |
| Hot-Reload | mid-run · zwischen Läufen | **nur zwischen Läufen** | Determinismus und Nachvollziehbarkeit eines Laufs |
| Einrückung | 2 · 4 Leerzeichen | **4, Formatter-kanonisch** | Python-Konvention, Lesbarkeit |
| Ereignisströme | Snapshot pro Tick · explizite Queue mit `pop` · beschränkte FIFO mit Konsumenten-Cursor | **FIFO + Cursor, „untersucht heißt konsumiert"** | keine verlorenen Elemente über Zustandsgrenzen, eine einzige Konsumregel, mehrere Konsumenten mit verschiedenen Perioden |
| Überlauf-Default | drop · fault | **fault** (Input), `drop_oldest` opt-in | Mithalten ist ein „expected state"; Datenverlust muss explizit erlaubt werden |
| Mustersprache | reguläre Ausdrücke · Grammatiken · typisierte Muster | **typisierte Muster → DFA** | linear, total, für Techniker lesbar, Captures typisiert; deckt Logs und Textprotokolle ab |
| Handler im Entry-Tick | ausführen mit unterdrücktem `->` · nicht ausführen | **nicht ausführen** | sonst ginge ein Übergang auf ein im Eintritts-Tick angekommenes Element verloren |
| `FAULTED` und Vorfahren-Blöcke | ausführen · nicht ausführen | **nicht ausführen** | ein Interlock in `FAULTED` hätte kein Fault-Ziel; Outputs sind bereits sicher |
| Implizite Prüfungen in Aktionsblöcken | verbieten · erlauben wie Arithmetik-Faults | **erlauben, mit Warnung** | konsistent mit totaler Arithmetik; Terminierung über Fault-Wald |
| Sub-Tick-Zeit | kleinerer Tick · Hardware-Trigger · geplante Ausgaben | **geplante Ausgaben (`at`) + Trigger in v1.2** | Präzision wird vom Tick entkoppelt; Latenzuntergrenze ist explizit (7.5) |
| Geplante Ausgaben bei Fault | beibehalten · verwerfen | **verwerfen** | ein Safe-Wert darf nie nachträglich überschrieben werden |
| Optionalwerte | Option-Typ explizit · nur Channel-Qualität · `T?` mit Dominanz | **`T?` einheitlich; Channels sind `T?` mit Alter** | ein Modell, dieselbe Analyse, kein Unwrap-Rauschen |
| Fehlende Nachricht im Test | Fault · Befund | **`verify`/`verdict` als Beobachtung, `check`/`expect` als Fault** | Tests wollen weiterlaufen und berichten; Sicherheit will abbrechen |
| Verdikt | Bool · dreiwertig | **FAIL > INCONCLUSIVE, PASS nur explizit** | ein Lauf ohne Aussage darf nicht als bestanden gelten |
| Szenarien | eigene Test-DSL · Maschinen im Sim-Build | **Maschinen** | eine Semantik, Coverage gratis |
| Schlafen | tickless Runtime · `idle`-Zustände mit virtuellen Ticks | **`idle`-Zustände** | statisch prüfbar, Trace-Äquivalenz beweisbar (9.9) |
| Nicht-Wake-Streams im Schlaf | Überlauf-Fault · verwerfen mit Alert | **verwerfen mit Alert** | sonst wäre der `idle`-Schritt keine Identität |
| Persistenz | NVM-API · `persist var` | **`persist var`** | keine I/O im Nutzercode; Semantik = Wahl von s0 |
| Ausgabestrom | pro Tick strikt beschränkt · Puffer mit `free`-Input | **Puffer mit `free`-Input** | realistische Nachrichtengrößen; Determinismus über den Input erhalten |
| Timer-Breite | u32 · u64 | **u64** | Burn-in über Tage bei 10-µs-Tick |
| Notation der Semantik | Unicode in Pseudocode · ASCII | **ASCII** | Referenzinterpreter, Doku und Tests teilen dieselbe Schreibweise |
| Abort bei langsamen Maschinen | nächste Aktivierung · außerplanmäßige Aktivierung · Runtime-Safe-Latch · Abort-Phase | **Abort-Phase** | Latenz ≤ 1 Tick ohne Glitch; Kosten statisch (Σ F_m); Ordnungsunabhängigkeit durch `raised` erhalten |
| Werte außerhalb der Channel-Range | klemmen · `Bad` · Fault | **`Bad` mit Grund** | Soundness der Intervallanalyse; konsistent mit dem Sensormodell |
| Multirate-Scheduling | Hyperperioden-Tabelle · Zähler | **Zähler** | Semantik braucht keine Tabelle; Speicher linear in der Maschinenzahl |
| Schedulability | exakt über H · Spitze Σ B_m | **Spitze; exakt nur bei kleinem H mit Phasen** | bei Phase 0 ist die Spitze ohnehin exakt; Abort-Phase kommt additiv hinzu |
| Algorithmische Tiefe | Rekursion · dynamische Strukturen · Matrizen + Natives | **Matrizen fester Größe + kuratierte Natives mit Kostenvertrag** | erhält Terminierung und Budget; Kryptographie außerhalb der Sprache |
| Plausibilität am Rand | nur Blöcke · Attribute | **`max_slew` als Attribut, Rest Blöcke** | einheitlich am Rand, minimale Attributfläche |
| Präzision von `at` | vertrauen · deklarieren | **`jitter` aus Konformitätsmessung, statisch geprüft** | Sweeps unterhalb der Hardware-Präzision werden abgelehnt |
| Tick-Quelle | immer freilaufend · Hardware-Ereignis | **wählbar (`tick_source`)** | phasenstarre Regelung ohne Semantikänderung |
| Einheiten in Matrizen | uniform · Records mit linearen Abbildungen · dimensionierte Matrizen (Hart) | **`mat[R, C]` mit `unitvec`** | Kovarianz, Übergang, Gewinn sind äußere Produkte; Prüfung bleibt Exponentenvergleich |
| Live-Tuning | Input-Channel · `tunable param` | **`tunable param` als Zucker** | immer gültig, Halte-Semantik, Range erzwungen, atomarer Satz, reproduzierbar |
| Bus-Mappings | Sprachkonstrukt · Konfigurationsschicht | **Konfigurationsschicht + Bibliothek (8.10)** | Bindungen sind Konfiguration; keine neue Garantie durch Syntax |
| Firmware-Lebenszyklus | Sprachkonstrukt · System-Channels | **System-Channels (12.7)** | Sichtbarkeit und Hebel genügen; Plattform bleibt zuständig |
| Rand-Empfindlichkeit | scharf · degradieren | **Inputs degradieren, Outputs faulten; Entschärfung nur explizit (`debounce`, `for d`)** | ein Mechanismus je Fehlerart; Sicherheitsargument kann Haltedauern einrechnen |
| Relationale Analyse | Intervalle · Oktagone · SMT | **Oktagone (v1.1)** | polynomiell, vorhersagbar, deckt `a < b` und Differenzen ab |
| Variable Nutzlastlängen | dynamische Puffer · Cursor-API über feste Puffer | **`reader`/`writer` + `len_field`** | total, statisch, deckt reale Protokolle ab |
| Kostenmodell | ein Faktor · Klassen | **Klassen mit kalibrierter Tabelle** | FPU-lose Kerne; Fehler von zwei Größenordnungen vermieden |
| Numerik ohne FPU | Festkomma-Typ · Einheiten auf Integern | **`int[U]`** | kein zweiter Zahlentyp; nominale Einheiten tragen die Skalierung |
| Funk auf Ein-Chip-Systemen | Bare-Metal · RTOS-Profil · Zwei-Chip | **RTOS-Profil unterstützt, Zwei-Chip empfohlen** | Determinismus des Funkstacks nicht herstellbar; TCB-Grenze klar |
| Integer-Breite auf 32-Bit-Kernen | explizit tippen · targetabhängig · Verengung aus Ranges | **Verengung aus bewiesenen Intervallen (Lemma 3.4)** | semantikneutral, nutzt vorhandene Analyse, keine Mehrarbeit für Techniker |
| Fließkommabreite | explizit tippen · targetabhängig · programmweite Festlegung | **`system: float = f32 \| f64`** | Programmeigenschaft, Simulation rechnet identisch, Sim = HW bleibt |
| FMA | verboten · implizite Kontraktion · explizite Primitive | **explizites `fma`, korrekt gerundet** | auf allen FPU-Zielen in Hardware; bitidentisch; Kontraktion bleibt verboten |
| Schnelle Mathematik | Vendor-Bibliotheken · nur korrekt gerundet · deterministische Näherungen mit Schranke | **Näherungen mit deklarierter Fehlerschranke** | Genauigkeit wird sichtbare Wahl; Determinismus bleibt |
| Subnormale | ignorieren · FTZ · IEEE-Modus + Totband | **IEEE-Modus erzwingen, Totband in Filtern** | Bit-Identität; Leistungseinbruch deterministisch vermieden |
| Performance-Aussagen | Schätzungen · Messung | **`takt bench` je Zielklasse** | Zahlen nur aus Messung, gekoppelt an die Kalibrierung |
| Speicher variabler Stream-Elemente | Slots fester Größe · Byte-Ring · gemeinsamer Pool | **Byte-Ring mit Deskriptoren, sicherer Default, `expect_len` als Opt-in** | Grenzen je Stream bleiben; Speicher folgt der deklarierten Erwartung; Überlauf bleibt definiert |
| Große Wertetypen | LLVM · Referenzen in der Sprache · compilergeführte Zeigerübergabe + Scratch | **compilergeführt** | Korrektheit folgt aus der Referenzfreiheit; Stack bleibt exakt |
| `sched`-Warteschlangen | für alle Outputs · nur bei Verwendung | **nur bei Verwendung** | statisch bekannt; spart Kilobytes auf kleinen MCUs |
| Matrizengrenze | fest 16 · keine | **keine; Lint ab 16** | Budget und Größe prüfen ohnehin |
| Stack-Gesamtbedarf | nur Programm · Zusammensetzung mit Verträgen | **Zusammensetzung + Schutzbereich** | TCB-Anteile sind messbar, nicht beweisbar; Schutzbereich macht Verletzungen sichtbar |
| Tick-gelesene Konstanten auf XIP-Flash | im Flash · in RAM | **in RAM** | sonst Stillstand bei Flash-Schreibzugriffen |
| DFA-Tabellen | 256 Spalten · Alphabetklassen + ein DFA je Zustand | **Alphabetklassen + ein DFA je Zustand** | Größe und Budget sinken um eine Größenordnung; Semantik gleich |
| Zustandslokaler Speicher | getrennt · Overlay nach Geschwistern | **Overlay** | Geschwister sind nie gleichzeitig lebendig; Definite Assignment je Eintritt sichert es |
| Speicherbudget | nur Instruktions-RAM auf XIP-Targets · alle Embedded-Profile | **alle Profile, aufgeschlüsselt (`takt size`)** | Speicher kennt der Compiler, nicht der Entwickler |
| Speicherschutz | keiner · MPU-Regionen | **MPU-Regionen gegen TCB-Fehler** | das Programm braucht keinen Schutz, die TCB schon |
| Kommunikation als Warteschlange | nur `pub var`/`signal` · interne Streams | **interne Streams mit Stream-Semantik** | ein Mechanismus für Warteschlangen, gleiche Cursor-Regeln, Unit-Delay erhält Ordnungsunabhängigkeit |
| Berechnungen länger als ein Tick | verbieten · Tick vergrößern · Jobs + Chunk-Natives | **Jobs (Fertigstellung als Input) + Chunk-Natives** | Budget bleibt statisch; Determinismus über den Input-Strom; Sim = HW für Ergebnisse |
| Fehlercodes | `T?` · Records · Generics · `T!E` | **`T!E`** | dieselbe Analyse wie `T?`, keine Generics |
| Tick-Modus für Startprogramme | freilaufend (Zeit als Input) · periodisch | **periodisch, 1 ms** | ein Zeitmodell; Kosten im Millisekundenbereich |
| Flash-Operationen | Sonderkonstrukt · Kommando + Status | **Kommando + Status, Modell mit Stromausfall-Injektion** | keine neue Semantik; Stromausfallsicherheit wird kampagnenfähig |
| Register in Takt | nie · überall · Treiberstufe | **Treiberstufe `port` (v1.2), sofortige geordnete Semantik, nur in `driver`-Maschinen, Gerätemodell Pflicht** | Nutzen für einfache Treiber, TCB-Grenze bleibt sichtbar |
| Nachweis der Migration | Tests von Hand · Orakel-Modus | **Orakel-Modus in der Simulation** | deterministischer Tick-Vergleich gegen das Original; jede Abweichung reproduzierbar |
| Unbeweisbare C-Annahmen | ignorieren · `check` einfügen | **`check` einfügen; Anzahl als Qualitätsmaß** | Übersetzung ist per Konstruktion crash-frei |
| Irreversible Ausgaben | wie andere Outputs · `irreversible` mit `expect`-Pflicht | **`irreversible` mit `expect` und Szenario-Abdeckung** | ein Rückweg existiert nicht; die Voraussetzung muss sichtbar geprüft sein |
| Sprachentwicklung ohne Bruch | nie brechen · Toolchain-Pinning · Editionen | **Editionen** | Bedeutung hängt am Programm; Compiler trägt mehrere; MIR editionsfrei |
| Zukünftige Bezeichner | nichts reservieren · Wörter und Membernamen reservieren | **reservieren, versioniert je Edition** | `x.valid` bleibt eindeutig; neue Wörter brechen nichts |
| Wachsende Enums | immer geschlossen · offene Enums | **offen für System-Enums und per `open`** | neue Varianten brechen keine `match` |
| Generics | nie · Intrinsics · Monomorphisierung mit festen Fähigkeiten · Traits | **Monomorphisierung, sechs feste Fähigkeiten** | statisches Budget je Instanziierung; kein Dispatch; keine Trait-Auflösung |
| Parallelität in einer Maschine | Regionen · nur Maschinen · gescopte Instanzen | **gescopte Instanzen** | vorhandene Semantik; Spitzenlast über Konfigurationen maximierbar |
| History | flach · tief | **tief, ein Wort `resume`** | erwartetes Wiederaufnehmen; Faults ignorieren es |
| Verteilte Zeitbasis | Uhren je Knoten · eine logische Zeitbasis | **eine Zeitbasis, Knotenticks als Vielfache** | erhält Satz 9.4.1 |
| Lesen über Knoten | instantan mit Analyse · Verzögerung = Hops | **Verzögerung = Hops** | Verallgemeinerung von Ψ; keine Kausalitätsanalyse |
| Knotenausfall | Fault für alle · Inputs degradieren, Outputs faulten | **wie 12.6 auf Knoten angewandt** | ein Mechanismus je Fehlerart |
| Capture | eigener Typ · Stream-Element | **Stream-Element mit Armierung als Kommando** | keine neue Semantik |
| Trigger | kleine Maschine · deklarative Regel auf dem Knoten | **Regel auf dem Knoten, im Programm Input und Armierung** | Budget des Hauptprogramms unberührt; `bound` als garantierte Latenz |
| Eigenschaften | unbeschränkte LTL · nur Invarianten · beschränkte Temporallogik | **beschränkt, endlicher Speicher** | überwachbar, beweisbar, keine Faults |
| `map` | sortiertes Array · offene Adressierung · Buckets | **offene Adressierung, deterministischer Hash, Rückwärtsverschiebung** | O(N) beschränkt, Iteration bitidentisch |
| Formate | unversioniert · versioniert | **versioniert mit Aufwärtskompatibilität** | MIR-Vertrag für die VM; Replay alter Läufe |
| Schnell gekoppelte Regelkreise | `direct` in v1 · eine Maschine · `follows` | **eine Maschine (Blöcke); `follows` in v1.1 für Same-Rate-Aufteilungen** | kein reales Problem für Kaskaden (ein Basis-Tick Latenz); `follows` braucht nur einen DAG-Check statt einer Datenflussanalyse |

---

## Anhang A — Herkunft und Änderungsgeschichte

**Herkunft.** Der Entwurf entstand aus einer Analyse des öffentlich beschriebenen Konzepts einer proprietären Hardware-Steuersprache (RevelCode der Revel Software Corporation); deren belegte Eigenschaften — kompiliert, deterministisch, „runtime safe", Python-nahe Syntax, Zustands- und Check-Konstrukte, Umschaltung Simulation/Hardware über die Channel-Deklaration — dienten als Randbedingungen. Alle Entscheidungen in diesem Dokument sind eigene; keine Aussage beschreibt die Implementierung jener Sprache. Vergleiche mit anderen Werkzeugen (synchrone Dataflow-Werkzeuge, Ada/SPARK, MISRA-C, Rust) stehen nur im Entscheidungslog als Begründung.

**Änderungsgeschichte** (die Versionsangaben beziehen sich auf die Reihenfolge der Überarbeitungen; Inhalte früherer Fassungen sind vollständig in diese Fassung übernommen):

v0.2.6 = v0.2.5 plus die Ergänzungen für die Migration von Bootloader- und ESP-IDF-Logik (interne Streams, Jobs und Chunk-Natives, `T!E`, `layout`-Erweiterungen, Byte-Einheiten, Kommando+Status-Muster mit Flash-Modell, Boot-Profil, System-Channels für Boot, Journal-Anforderungen, `inout`, Bereichsmuster, Compile-Zeit-Auswertung, reproduzierbare Builds, Orakel-Modus und `takt import-c`, Treiberstufe `port` als v1.2-Entscheidung). v0.2.5 = v0.2.4 plus die Ableitungen aus der Speicheranalyse (Byte-Ringe für Streams, compilergeführte Zeigerübergabe mit statischem Scratch, Overlay exklusiver Zustandsspeicher, `sched` nur bei Verwendung, Stack-Zusammensetzung mit nativen Stack-Verträgen, tick-gelesene Konstanten im RAM, DFA-Alphabetklassen, Speicherbudget für alle Embedded-Profile, `takt size`, MPU-Schutzbereiche). v0.2.4 = v0.2.3 plus die Ableitungen aus dem Performance-Feedback (Darstellungsverengung aus Ranges, programmweite Fließkommabreite, explizites `fma`, deterministische Schnellvarianten, IEEE-Modus-Regel für FPUs, Kostenklassen `i32`/`i64`, Zielklassen, `takt bench`). v0.2.3 = v0.2.2 plus die Ableitungen aus dem zweiten Feedback und der ESP32-C6-Portierungsprüfung (dimensionierte Matrizen, `tunable param`, `follows`, degradierender Treiberrand mit `debounce`/`Suspect` und Bestätigungszeiten, Oktagon-Analyse, `reader`/`writer`, typisiertes Kostenmodell, Einheiten auf Integern, Geräteprofile, Plattformschnittstelle, RTOS-Profil). v0.2.2 = v0.2.1 plus die Ableitungen aus der Bewertung der externen Analyse (Abort-Phase, defensiver Treiberrand mit Rand-Durchsetzung deklarierter Ranges, Zähler-Scheduling statt Hyperperioden-Tabelle, Matrizen fester Größe, kuratierte native Funktionen mit Kostenvertrag, Tick-Quelle, `jitter`/`max_slew`). v0.2.1 = v0.1 (Fundament) mit vollständig eingearbeitetem Review v0.2: Korrekturen B6–B16 (Befund-Nummern des Reviews, hier jeweils an der korrigierten Stelle genannt), Ereignisströme, Muster, Datenmodell, Zeit unterhalb des Ticks, Testsemantik, Feldbetrieb. Alles in diesem Dokument ist eigene Design-Entscheidung; belegte RevelCode-Eigenschaften (Rev. 1/2) sind als Randbedingungen eingeflossen, keine Aussage hier beschreibt Revels Implementierung.

Die Korrekturen „B6–B16" stammen aus einem internen Review der ersten Fassung: B6 `FAULTED` führt keinen Nutzercode und keine Vorfahren-Blöcke aus (5.2, 5.3, 9.3); B7 implizite Prüfungen in Aktionsblöcken werden wie Arithmetik-Faults behandelt (5.5); B8 Timer als u64-Tick-Zähler (7.1, 11.2); B9 Outputs stehen vor dem ersten Commit auf `safe` (1.5); B10 Relationen zwischen Variablen kommen über Parameter-Ranges, nicht aus der Intervallanalyse (3.4, 11.4, 14.4); B11 Aktivierung per Zähler statt Hyperperioden-Tabelle (1.3, 7.2); B12 Checks der Eltern sehen `enter`-Outputs erst im nächsten Tick (5.2); B13 `break` (5.8); B14 ASCII-Notation der Semantik (9.0); B15 Parameter-Defaults und Instanz-Arrays (5.8); B16 zustandslokale Variablen (5.8). Die Nummern wurden aus dem Normtext entfernt.

v0.2.8 = v0.2.7 plus das Reservierungspaket für Vorwärtskompatibilität: Editionen (`system: language`), reservierte Wörter und Membernamen, offene Enums, Verdeckungs- und Additivitätsregeln (2.5); Generics als Klammerklassen mit Monomorphisierung (3.12); gescopte Instanzen statt paralleler Regionen (5.11); tiefe History `resume` (5.12); Trigger als Knotenregel mit `arm`/`disarm` und `fired` (7.5); `capture<T, N>` (3.9, 8.9); beschränkte Temporallogik für `property` (13.3); `map<K, V, N>` vollständig (3.9); Operator-Metadaten (2.5); versionierte Formate (11.3, 12.5, 8.10); Regeln der verteilten Ausführung, die schon heute gelten (12.9). Keine Bedeutungsänderung für bestehende Programme.

**Redaktionelle Präzisierung der Komponentenliste (11.1).** Die Liste des Rust-Workspace nennt jetzt jede Komponente, die an anderer Stelle der Referenz gefordert wird, aber bisher keinen eigenen Ort hatte: den Treiberrand als `takt-hal` (Traits für Skalare, Streams, geplante Ausgaben und Flash-Geräte, 12.6, 13.8 — die Sim/HW-Umschaltung nach 8.3 ist ein Treiberwechsel, kein zweiter Programmpfad), die Standardbibliothek `takt-stdlib` in Takt selbst (11.4), die Konformitätssuite `takt-conformance` mit `takt bench` und der Kalibrierung von `c_target`, `guard` und `jitter` (13.8, gelesen von 7.2 und 7.5) sowie das C-Frontend `takt-import-c` (13.9). Die beiden Runtimes `takt-rt-std` und `takt-rt-nostd` werden zu einem gemeinsamen `takt-rt-core` mit vier Profilaufsätzen, weil 12.8 vier Laufzeitprofile definiert (`linux_rt`, `baremetal`, `rtos`, `boot`) und drei davon `no_std` sind, sich aber in Tick-Quelle, Aufgabenmodell und Recorder unterscheiden; die Aussage aus 0.1, dass zwei Runtime-Arten hinter derselben MIR stehen, bleibt unberührt. `takt-cli` nennt zusätzlich die in 2.5, 8.4, 13.8 und 13.9 beschriebenen Kommandos `bench`, `tune`, `migrate` und `import-c`. Die Sprache selbst ändert sich dadurch nicht: keine neue Edition, keine Bedeutungsänderung für bestehende Programme.

**Verschlankung der Schlüsselwortliste (2.2) und Grammatikkorrekturen (2.3).** Die Liste enthielt Attribut-, Positions- und Typwörter, die nie am Zeilenanfang stehen; zwei davon (`debounce`, `rate`) waren zugleich Namen von Bibliotheksblöcken (11.4), und das Beispiel in 3.7 benutzte `offset` als Feldname. Die Liste folgt jetzt der in 2.2 genannten Regel; entfernt wurden `tick from layout hw sim safe max_age rate max_rate capacity framing overflow wake phase idle timeout fail cost total mat tick_source tick_tolerance jitter max_slew vec follows debounce ticks len target f64 inout irreversible bits offset align duration mmio language open req resume capture` sowie die doppelten Einträge `arm disarm`; neu sind `stream` (leitet interne Streams ein) und `then` (Trigger). Ein Programm, das eines der entfernten Wörter als Bezeichner nutzt, war bisher ein Fehler und ist jetzt gültig; ein Programm mit den Bezeichnern `stream` oder `then` gab es nicht. In der Grammatik sind die Kostenklassen aufgezählt (`cost_class`), Record-Felder mit Bitfeldern verlangen kein `NEWLINE` nach dem `DEDENT` mehr (`record_field`), und `i64` ist als Name erlaubt (3.1). Die Lexer-Spezifikation (`grammar/lexer.md`) präzisiert 3.3 um die Leerraumregel für Einheitenausdrücke; die Matrixbeispiele in 3.11 schreiben `float[s]`-Elemente jetzt als `(10 ms).as(s)`, weil `0.01 s` nach 3.3 eine Dauer ist. Zwei weitere Grammatikkorrekturen nach externem Review: ganze Zahlen in Diskriminanten, Offsets, Bitpositionen, Attributen und Kostenverträgen sind in jeder Schreibweise erlaubt (`int_lit`, wie die Beispiele in 3.7 mit `0x00` voraussetzen), und der weiche Timeout `until … timeout d else:` ist eine eigene Alternative von `seq_item`, weil sein Block das Zeilenende selbst trägt. Nach einem zweiten Review: `capture<T, N>` ist Stream-Elementtyp, nicht allgemeiner Typ (8.9); `fn` ohne Rückgabetyp ist mit `inout` erlaubt (3.9); Größenparameter in Signaturen sind als `[const N]` deklariert (3.12); die Reservierung der Membernamen (2.5) gilt nur noch für Wrapper-Zugriffe, weil Records und Enums einen eigenen Namensraum haben — die Beispiele mit `NONE`-Varianten und dem Feld `data` sind damit gültig, das Feld `ok` in `SelftestResult` heißt `passed`; die Einheit an der Obergrenze einer Range gilt für beide Grenzen (3.4, 3.6); `event` im `then`-Teil eines Triggers ist benannt (7.5). Aus dem ersten Korpus-Durchlauf (`grammar/parse_corpus.py`): `unit` darf eine dimensionslose Einheit ohne Einheitenausdruck definieren (3.2), und eine Typvariable darf wie ein Typname `?` und `!E` tragen (3.12, `-> T?`). Aus dem Schnipsel-Korpus aller Codeblöcke: der Beispiel-Channel `log` heißt `dut_log`, weil `log` ein Schlüsselwort ist; Matrizenvariablen in 3.11 sind klein geschrieben; Auslassungen `...` in Codeblöcken sind durch Code ersetzt, damit jeder Block parst. Konstantenvariablen in Generics (3.12) gehören zu v1.1 statt v1.2, weil die Standardbibliothek (11.4) sie braucht; die Grammatik parst sie ohnehin ab M0. Die Sprache bleibt Edition 1.

**Festlegungen aus dem differenziellen Mutationstest (`grammar/diff_parse.py`).** Der Vergleich des Parsers mit dem Grammatik-Orakel auf veränderten Referenzschnipseln zeigte drei Stellen, an denen die EBNF mehr zuließ als beabsichtigt; sie sind jetzt in der Grammatik (2.3) und der Lexer-Spezifikation festgehalten: ein kontextuelles Wort ist nie Einheitenname (`3 timeout` ist die Zahl 3 vor der Klausel, 2.2); die dimensionslose `1` eines Einheitenausdrucks steht nur als Zähler (`1/s`, `1/X`), sodass `1 1` kein Literal ist; in `<…>` eines Typs schließt `>` den Typ, ein Vergleich darin steht in Klammern (`bytes<(a > b)>`); und die Einheit nach einer Zahl ist eine eigene Produktion `unit_lit`, die genau so weit reicht, wie die Tokens anliegen (3.3), sodass `9.81 m/s^2 m/s^2` kein Ausdruck ist. Kein gültiges Programm ändert seine Bedeutung.

**Festlegungen aus dem Grammatik-Fuzzer (`grammar/fuzz_grammar.py`, `plan/fuzzer.md`).** Erzeugte Programme aus der EBNF in fünf Fassungen deckten Lücken des Parsers auf, die ohne Bedeutungsänderung geschlossen sind: `>` vergleicht auch in eckigen Klammern innerhalb von Typklammern (`bytes<[8 >> 1][0]>`); die Atome einer Eigenschaft sind Vergleiche, Musterprüfungen und Werte (`tprop_atom := … | cmp_expr`), weil `a or b if c else d` sonst zwei Ableitungen mit verschiedener Bedeutung hätte — die Bedingungsform steht in einer Eigenschaft nur innerhalb eines Aufrufs; ein Exponent und die Einheitenklammer eines Zahlentyps brauchen kein Anliegen (`float[K ^ 2]`, `u16 [mV]`), nur das Einheitenliteral nach einer Zahl (3.3); eine Einheit in Typnamenform mit anliegendem Operator (`f[KiB/s]`) ist ein Generik-Argument der Klasse Einheit; und die Verschachtelungstiefe ist auf 64 Ebenen begrenzt (2.1).

**Festlegung zur Edition (2.5).** Weil die Edition in der Datei steht, der Tokenizer aber den Wortschatz der Edition braucht, liest ein Vorlauf `language = N` aus dem `system:`-Block (Kommentare und Leerzeilen übersprungen, Ende beim ersten nicht eingerückten Wort nach dem Block); erst dann wird tokenisiert. Fehlt der Eintrag, gilt die neueste Edition mit Warnung 49; eine unbekannte Nummer ist ein Fehler 49. Prüfung 51 arbeitet bis zur Typinferenz mit dem deklarierten Typ des Subjekts, mit `x.reason` und `last_fault.kind` und mit den Variantennamen der Zweige; andere Subjekte bleiben bis dahin ungeprüft.

**Festlegungen für das Lowering und den Referenzinterpreter (`plan/m1.md`).** Beim Entwurf des Lowerings vom Syntaxbaum in die MIR und des Interpreters wurden acht Punkte festgelegt, die die Referenz bisher offen ließ oder in sich widersprüchlich beschrieb; kein gültiges Programm ändert seine Bedeutung. (1) Einheiten-, Konstanten- und Typvariablen werden aus den Argumenten gelöst, sobald ein Parameter genau eine offene Variable mit Exponent ±1 enthält; eine Variable ohne solchen Parameter wird explizit angegeben (3.12, Beispiel in 5.7 angepasst). (2) `wrapping_add`, `wrapping_sub`, `wrapping_mul`, `saturating_add` und `saturating_sub` sind Funktionen wie `rotl` und `rotr`, keine Membernamen (4.1); die Liste der eingebauten Zugriffe in 2.5 bleibt unverändert. (3) `float` ist der Typ der Programmbreite: in einem `f64`-Programm derselbe Typ wie `f64`, in einem `f32`-Programm derselbe wie `f32`; „nie implizit gemischt" (4.2) gilt zwischen den beiden Breiten. (4) Der Zustand einer Maschine (`m.state`) hat einen je Maschine eingebauten Enumtyp mit ihren Zustandsnamen und `FAULTED`; `last_fault` ist ein eingebautes Record mit den Feldern `kind`, `message`, `line`, `tick` (5.3); `OK` und `ERR` sind die Varianten 0 und 1 von `T!E`. (5) Innerhalb eines Programms darf ein innerer Sichtbereich keinen sichtbaren Namen erneut vergeben (Fehler 2); nur Namen der Standardbibliothek dürfen verdeckt werden (Warnung, 2.5). (6) Ein `hw`-Input ohne `sim`-Quelle ist in `takt sim` eine Warnung 13, weil ein Stimulus ihn treiben darf, und ab `takt test` ein Fehler; ein Input ohne Quelle und ohne Stimulus ist `Bad` mit Grund `Driver`. (7) Stimuli, Golden-Traces und die lesbare Form der Aufzeichnung (12.5) teilen ein zeilenorientiertes Textformat (`grammar/trace.md`) mit kanonischer Reihenfolge je Tick, sodass die Ordnungsunabhängigkeit (9.4.1) am Trace prüfbar ist. (8) Die MIR trägt die Hebung `T` nach `T?` (3.8), die Konstruktoren `OK`/`ERR` und die Primitive mit eigener Fault-Semantik (`sqrt`, `round`, `fma`, `interp`, `rotl`, …) als eigene Knoten (Formatversion 2, `plan/mir.md`). (9) `unit_decl` nimmt dieselben Namensformen wie `unit_term` (`unit_name := IDENT | UPPER_IDENT | TYPE_IDENT`), weil die vordefinierten Einheiten der Referenz (`V`, `Hz`, `Pa`, `N`, `B`, `Ah`) sonst benutzbar, aber nicht deklarierbar wären; die Standardbibliothek schreibt sie in Takt selbst (11.4). Ein Einheitenname ist damit nie ein Schlüsselwort und nie ein kontextuelles Wort (2.2).


**Die Bindung eines Stream-Elements ist ein Wrapper (8.6, 8.7).** Bis v0.2.8 reichte eine Bindung die Felder eines Record-Elements durch: `on can_rx as f:` machte `f.id` und `f.dlc` direkt lesbar, daneben `f.t` und `f.seq`. Das las sich kürzer und vermischte zwei Namensräume. Wo ein Rahmenformat selbst ein Feld `seq` führt — in Protokollen der Normalfall, etwa eine Sequenznummer im Header —, verdeckte das Metadatum es: `f.seq` lieferte die Nummer im Strom, und das Protokollfeld war *unerreichbar*. Nicht als Fehler, sondern als stiller falscher Wert. Dieselbe Kollision drohte bei `t`, `data` und `text`, die 2.5 als Record-Feldnamen ausdrücklich erlaubt (`CanFrame.data`).

Die Referenz hatte den Fall an anderer Stelle bereits entschieden: Auf einem Wrapper (`T?`, `T!E`, Channel, Job-Handle) „meint `x.name` immer den Wrapper, und Felder des Inhalts sind erst nach dem Auspacken erreichbar" (2.5). Eine Bindung ist genau das — ein Element mit Metadaten darüber. Sie trägt deshalb `.t`, `.seq` und den Inhalt unter *einem* Namen (`.data`, bei `line<N>` `.text`); Felder des Elements stehen darunter (`f.data.id`).

Drei Wirkungen. Erstens verschwindet die Verdeckung strukturell statt durch eine Verbotsliste: `f.seq` ist die Stromnummer, `f.data.seq` das Protokollfeld, beide sichtbar. Zweitens steht der Inhalt als ein Wert zur Verfügung — `parse_frame(f.data)` gibt ihn an eine reine Funktion weiter, was 4.1 und 13.8 als Form verlangen (ohne Hardware testbar) und was vorher eine Kopie je Feld erzwang. Drittens gilt ein Zugriffsmuster für alle Elementarten statt vier; ein Skalarstrom (`stream<u8>`) hatte zuvor gar keinen Zugriff auf seinen Wert. Der Preis ist ein Feldzugriff mehr je Stelle, nach einer Regel, die aus `T?` schon bekannt ist.

Die Änderung bricht Programme, die Felder direkt lasen (`f.id` wird `f.data.id`); sie ist mechanisch migrierbar und im Korpus an genau einer Stelle angefallen. Die Edition (2.5) trägt solche Änderungen, und je später sie kommt, desto teurer wird sie.