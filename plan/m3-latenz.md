# Safe-State-Latenz, Maschinenbudgets und zwei Lints

Stand 2026-09-11. Bezug: `plan/ideen-bewertung.md` (A1, B1, D1, E5),
`plan/definition.md` v0.2.8, `plan/feedback.csv` (FB-45 bis FB-51).

Dieses Dokument plant die vier Ergänzungen, die aus der Bewertung von
`feedback/ideen_dmx.md` zur Umsetzung ausgewählt wurden, und hält fest,
welche davon jetzt gebaut werden können und welche nicht.

---

## 0. Was jetzt geht und was nicht

Die Bewertung hat A1, B1, D1 und E5 zur Umsetzung empfohlen. Beim
Nachsehen im Code zerfallen sie in zwei Gruppen:

| | Vorschlag | Voraussetzung | jetzt baubar? |
|---|---|---|---|
| **A1** | Safe-State-Latenz | Periode, Fault-Wald, `Confirm`, `output_timing` | **ja, vollständig** |
| **D1** | Budget je Maschine | `Machine::budget` (seit M3), `with attr` an `machine_decl` | **ja, der `ram`-Teil** |
| B1 | `assumption` | `property` — steht in der Grammatik, meldet aber Stufe v1.1 | nein: M6 |
| E5 | Prüfung 60 | Hardware-Konfiguration (8.10) | nein: M6 |

**B1 und E5 sind nicht aufschiebbar aus Bequemlichkeit, sondern
unmöglich.** Für B1 ist `assumption` per Konstruktion ein Begleiter von
`property`: Eine Annahme beschränkt die Beweisverpflichtung einer
Eigenschaft. Ohne Eigenschaften gibt es keine Verpflichtung, die sie
beschränken könnte — man baute ein Attribut ohne Wirkung. Für E5 fehlt
schlicht die Vergleichsseite: Es gibt im ganzen Workspace keine
Hardware-Konfiguration, weder Parser noch Format noch Datenstruktur
(`FMT-Hardware-Konfiguration` steht auf `offen`). Eine Prüfung, die
Einheiten gegen eine nicht existierende Datei vergleicht, ist kein
Fortschritt, sondern toter Code.

Beide sind bereits spezifiziert (FB-46, FB-48; Prüfung 60 steht seit
heute in 10.1), also verliert der spätere Bau nichts. Dieses Dokument
baut A1 und D1 und schärft zwei Lints, die keine fremde Voraussetzung
haben.

---

## 1. Abwägungen

### 1.1 Wo lebt die Latenzrechnung?

| | in `takt-sema` | eigener Durchlauf in `takt-mir/analysis` | im Report des CLI |
|---|---|---|---|
| braucht Fault-Wald und Perioden | ja, hat sie | ja, hat sie | ja |
| wiederverwendbar für `takt certify` (C1) | nein | ja | nein |
| wiederverwendbar für Codegen (M4) | nein | ja | nein |
| neben `cost.rs`/`size.rs`, die dasselbe tun | — | ja | — |

**Entscheidung: ein eigenes Modul `analysis/latency.rs` in `takt-mir`,
neben `cost.rs` und `size.rs`.**

Begründung: Es ist dieselbe Art Rechnung wie das Kostenmodell und
`takt size` — eine statische Auswertung über die fertige MIR, deren
Ergebnis mehrere Verbraucher hat. Prinzip 1 des Plans („ein Werkstück für
alle"): Der Report braucht sie heute, `takt certify` (C1) später, und die
Fault-Erreichbarkeitsmatrix ist derselbe Graph-Durchlauf. Läge sie in
`takt-sema`, müsste M8 sie nachbauen.

### 1.2 Ticks oder Nanosekunden?

Der Vorschlag rechnet in Nanosekunden. Das ist die Zahl, die der
Prozessingenieur will — aber sie gilt nur, solange jeder Tick eingehalten
wird, und *das* prüft die Schedulability (7.2), die `c_target` aus der
Kalibrierung braucht (13.8, M5).

| | nur Ticks | nur Nanosekunden | beides, getrennt ausgewiesen |
|---|---|---|---|
| heute exakt | ja | nein: hängt an der Tick-Treue | Ticks ja, Zeit unter Vorbehalt |
| für den Anwender brauchbar | halb | ja | ja |
| ehrlich | ja | **nein** | ja |

**Entscheidung: beides, mit getrennter Herkunft.** Die Tickzahl ist
exakt und immer gültig — sie folgt aus Perioden und Fault-Wald. Die Zeit
ist `Ticks · T0` und gilt unter der Annahme, dass die Tick-Quelle hält;
solange die Schedulability nicht geprüft ist, wird sie als solche
beschriftet. Das ist dieselbe Konstruktion, die `takt size` mit
`Origin::{Exact, Contract, Open}` schon hat, und sie hat sich bewährt.

### 1.3 Was ist überhaupt „die" Latenz?

Der Vorschlag formuliert `D_safe` je *Bedingung*. Beim Bauen zeigt sich,
dass drei verschiedene Fragen darin stecken:

1. **Je `check`/`expect`**: Wie lange von „Bedingung verletzt" bis „diese
   Maschine ist in ihrem Fault-Ziel"? — Das ist `D_detect + D_confirm`.
2. **Je Maschine**: Wie tief ist ihr Fault-Wald? Ein Fault-Ziel, dessen
   Körper erneut scheitert, kostet weitere Ticks bis `FAULTED` (5.3).
3. **Je Output**: Wann steht *dieser Aktor* sicher? Das ist das Maximum
   über alle Pfade, die ihn schreiben, plus Propagation und Commit.

**Entscheidung: alle drei ausgeben, aufeinander aufbauend.** Die dritte
ist die Zahl aus dem Vorschlag (die FTTI); die ersten beiden sind ihre
Bestandteile und für die Fehlersuche das eigentlich Nützliche — sie sagen,
*warum* die Zahl so groß ist.

### 1.4 `within` jetzt oder später?

`check p < LIMIT, "…" within 5 ms` ist eine Anforderung an eine Zahl, die
der Compiler ausrechnet. Sie jetzt zu bauen hieße, gegen die Zeitspalte zu
prüfen — die unter Vorbehalt steht.

**Entscheidung: `within` jetzt in Grammatik, AST, MIR und Prüfung, aber
gegen die *Tickzahl* geprüft.** `within 5 ms` bei `T0 = 1 ms` heißt „in
höchstens 5 Ticks". Das ist exakt entscheidbar, heute, ohne Kalibrierung —
und es ist genau die Zusage, die die Semantik gibt. Kommt die
Kalibrierung, verschärft sie die Aussage, ohne die Regel zu ändern.

Der Alternativentwurf (`within` erst mit M6) hätte bedeutet, die
Grammatik zweimal anzufassen. Das ist teurer und bringt nichts.

### 1.5 Budget je Maschine: welche Größen?

Der Vorschlag nennt `{ram = 2 KiB, wcet = 20 us}`. `wcet` braucht
`c_target` (M5).

**Entscheidung: `ram` jetzt, `wcet` als Grammatik akzeptiert und mit
Stufenmeldung abgelehnt.** So steht die Schreibweise fest, bevor jemand
sie anders erfindet, und der Compiler sagt klar, woran es liegt. Das ist
dasselbe Muster wie bei `map`, `mat` und `node` (Abschnitt 15).

---

## 2. Die Rechnung

### 2.1 Größen

| Symbol | Bedeutung | Quelle |
|---|---|---|
| `T0` | Basis-Tick in ns | `Program::system.tick` |
| `n_m` | Periodenfaktor der Maschine m | `Machine::period` |
| `d` | Bestätigungszeit eines `check … for d` | `Confirm::duration` |
| `depth_m(q)` | Ticks von Zustand q bis zu einem stabilen Fault-Ziel | Fault-Wald (5.3) |
| `asap` | Commit-Modus | `System::output_timing` |

### 2.2 Je Prüfstelle

```
D_detect  = n_m                          Ticks, schlimmster Fall: die Bedingung
                                         wird kurz nach m's Aktivierung wahr
D_confirm = ceil(d / P_m)                Ticks, nur bei `for d`; sonst 0
D_fault   = depth_m(q)                   Ticks im Fault-Wald bis zum stabilen Ziel
```

`depth_m(q)` ist der entscheidende Teil, den der Vorschlag nicht nennt.
5.3 sagt: Ein Fault-Ziel wird im Entry-Modus ausgeführt; scheitert sein
Körper erneut, folgt der nächste Fault im Fault-Wald, „spätestens
`FAULTED`". Jeder dieser Schritte ist ein weiterer Tick. Die Tiefe ist die
Länge des längsten Pfades im Fault-Wald ab q — ein Graph-Durchlauf über
`State::fault_target`, den Prüfung 9 auf Azyklizität bereits prüft.

### 2.3 Je Output

```
D_prop    = 0                            derselbe Tick (Abort-Phase, 5.4)
D_commit  = 0    bei `asap`              Entry-Tick-Regel: der unsichere Wert
                                         wird nie committet (5.2, Punkt 4)
          = 1    bei `boundary`          LET: Commit am Tickende

D_safe(o) = max über alle Prüfstellen s, die o beeinflussen:
              D_detect(s) + D_confirm(s) + D_fault(s) + D_prop + D_commit
```

„o beeinflussen" heißt heute: `o` gehört der Maschine, in der `s` steht
(Single-Writer, Prüfung 7). Mit `follows` und `pub var`-Ketten kommt
`D_prop` hinzu; das ist M6 und wird hier als `0` gerechnet, weil es
ohne `follows` keine Ketten gibt.

### 2.4 Was die Rechnung *nicht* behauptet

- Sie gilt unter der Annahme, dass jeder Tick eingehalten wird. Ohne
  geprüfte Schedulability ist das eine Annahme, keine Zusage.
- Sie gilt für die Logik, nicht für den Aktor: `guard(o)` und `jitter(o)`
  des Treibers kommen aus 13.8 (M5) und werden nicht addiert, solange sie
  fehlen.
- Sie ist eine **obere** Schranke. Der Normalfall ist kürzer.

Diese drei Sätze gehören in den Report, nicht nur in dieses Dokument.

---

## 3. Umfang je Schicht

### 3.1 A1: Latenz

| Schicht | Arbeit |
|---|---|
| Grammatik | `check_stmt` um `[ "within" duration_expr ]` |
| AST | `CheckStmt::within: Option<DurationExpr>` |
| MIR | `StmtKind::Check::within: Option<Expr>`; Formatversion +1 |
| Analyse | neues Modul `analysis/latency.rs` |
| Sema | Prüfung 61: `within` gegen die berechnete Tickzahl |
| CLI | `takt latency` (neu) und eine Zeile im `check --report` |
| Referenz | 9.4.5 (Satz), 5.6 (`within`), 10.1 (Prüfung 61), 11.1 (Kommando) |

### 3.2 D1: Budget je Maschine

| Schicht | Arbeit |
|---|---|
| Grammatik | `attr` um `"budget" "=" "{" budget_item { "," budget_item } "}"` |
| AST | `AttrKind::Budget(Vec<BudgetItem>)` |
| MIR | `Machine::declared_budget: Option<DeclaredBudget>` |
| Sema | Prüfung 62: deklariertes gegen berechnetes Budget |
| Referenz | 7.2, 10.1 (Prüfung 62), `attr`-Liste in 2.3 |

### 3.3 Zwei Lints ohne fremde Voraussetzung

- **E7 (`alert`-Polarität)**: Warnung, wenn ein `alert` dieselbe
  Vergleichsrichtung hat wie ein `check` auf derselben Größe im selben
  Block. 5.6 begründet die umgekehrte Polarität; genau deshalb wird sie
  verwechselt.
- **E8 (fehlender Param im Profil)**: Warnung zur Übersetzungszeit. Ein
  Profil, das einen Parameter vergisst, sieht aus wie eines, das den
  Default will.

Beide sind Prüfungen ohne neue Syntax und gehören zu Prüfung 3
beziehungsweise einer neuen Prüfung 63.

---

## 4. Reihenfolge

1. `analysis/latency.rs` mit Tests — die Rechnung allein, ohne Sprache.
2. `takt latency` und die Report-Zeile — sichtbar machen, was gerechnet
   wird.
3. `within` durch Grammatik, AST, MIR, Sema (Prüfung 61).
4. `budget` durch dieselben Schichten (Prüfung 62).
5. Die zwei Lints.
6. Referenz, Inventur, Korpus.

Schritt 1 und 2 sind der Prototyp, den die Bewertung als „in Wochen
machbar" bezeichnet. Er steht nach Schritt 2 und braucht keine
Sprachänderung.

---

## 5. Stand der Umsetzung

| Schritt | Ergebnis |
|---|---|
| 1 | `analysis/latency.rs` mit drei Tests — fertig |
| 2 | `takt latency` und ein CLI-Test — fertig |
| 3 | `within` durch Grammatik, AST, MIR (Version 6), Sema; Prüfung 61 mit Korpus — fertig |
| 4 | `budget` durch dieselben Schichten; Prüfung 62 mit Korpus — fertig (`ram`; `wcet` meldet Stufe) |
| 5 | Die zwei Lints — **offen**, siehe unten |
| 6 | Referenz (9.4.5, 5.6, 7.2, 10.1, 11.1), Inventur, Korpus `14_latency.takt` — fertig |

270 Tests, clippy und rustfmt sauber, 194 Dateien ohne Abweichung zwischen
Rust- und Python-Werkzeug.

### 5.1 Was das Bauen ergeben hat

**Ein Summand fehlte in der Formel des Vorschlags.** `ideen.md` rechnet
`D_detect + D_confirm + D_prop + D_commit`. Dazu kommt die **Tiefe des
Fault-Waldes**: 5.3 sagt, ein Fault-Ziel wird im Entry-Modus ausgeführt,
und scheitert sein Körper erneut, folgt der nächste Fault — jeder Schritt
ein Tick. Eine Maschine mit `fault -> SAFE`, deren `SAFE` selbst faultet,
braucht zwei Ticks statt einem. Ohne den Summanden wäre die Schranke zu
klein, also falsch in der gefährlichen Richtung, und der Fehler fiele
nicht auf, solange der Fault-Wald flach ist — was er in Beispielprogrammen
immer ist. Steht als FB-52 in der Tabelle und als Term in Satz 9.4.5.

**`fault_target_of` lag im falschen Crate.** Die Funktion ist reine
MIR-Logik, stand aber in `takt-sema/src/checks.rs`, wohin `takt-mir` nicht
greifen kann. Sie ist jetzt eine Methode an `Machine`; `checks.rs`
delegiert, damit die vorhandenen Aufrufer unverändert bleiben.

### 5.2 Was offen bleibt

- **Die zwei Lints (E7, E8).** Sie brauchen keine fremde Voraussetzung und
  sind klein, gehören aber nicht in denselben Commit wie eine neue
  Formatversion — sie kommen als eigener Schritt.
- **B1 (`assumption`) und E5 (Prüfung 60)**, aus den Gründen in
  Abschnitt 0. Beide sind spezifiziert; der spätere Bau verliert nichts.
- **Die Zeitspalte** wird belastbar, sobald die Schedulability geprüft ist
  (7.2 mit `c_target` aus 13.8, M5). Die Regel von Prüfung 61 ändert sich
  dadurch nicht — sie gewinnt an Aussage.

---

## 6. Nachtrag: die zwei Lints und die Auffindbarkeit

**Schritt 5 ist erledigt.** Beide Lints laufen unter Prüfung 63:

- **E7 (`alert`-Polarität)** meldet den Fall, der sich beweisen lässt:
  dieselbe Größe, dieselbe Schranke, dieselbe Vergleichsrichtung in einem
  `check` und einem `alert` desselben Blocks. 14.1 stellt beide
  ausdrücklich nebeneinander in denselben `loop:` — dort sehen die zwei
  Zeilen gleich aus, und genau deshalb fällt die Verwechslung niemandem
  auf. Die entgegengesetzte Polarität, der dokumentierte Normalfall,
  schweigt.
- **E8 (Profil-Vollständigkeit)** warnt, wenn ein `profile` einen `param`
  nicht nennt. Zur Übersetzungszeit, nicht zur Laufzeit: Es ist eine
  Eigenschaft des Profils, und im Lauf wäre die Meldung zu spät.

Dabei fiel ein Werkzeugbefund an: `Expr` leitet `PartialEq` über alle
Felder ab, also auch über `span`, `range` und `repr`. Zwei gleich
geschriebene Bedingungen an verschiedenen Stellen sind damit nie gleich —
und genau die sucht der Lint. `Expr::same_as` vergleicht ohne Position und
Annotationen; die Methode gehört nach `takt-mir`, weil die Frage „steht
hier zweimal dasselbe?" nicht auf den Lint beschränkt ist.

### 6.1 Auffindbarkeit von B1 und E5

Die Frage war, ob die beiden verschobenen Befunde irgendwo festgehalten
sind. Sie standen in `plan/feedback.csv` — aber **nur dort**, und das ist
zu wenig: Wer M6 aus der Referenz oder der Inventur plant, hätte
`assumption` nirgends gefunden. Es kam im ganzen Repository nur in zwei
Plandokumenten vor.

Behoben (FB-53):

| | vorher | jetzt |
|---|---|---|
| `assumption` in der Referenz | fehlte | 13.3 mit Begründung, Kanal-Attributen, Kompositionalität |
| `assumption` in der Grammatik | fehlte | `assumption_decl`, Schlüsselwort in 2.2 |
| `assumption` im Compiler | fehlte | wird geparst, meldet Stufe v1.1 |
| `assumption` in der Inventur | fehlte | `KW-assumption` + drei `PAR-13.3` |
| Hardware-Konfiguration | `FMT-…` auf M0 | auf M6, wo 8.10 entsteht |

**Zur Keyword-Entscheidung.** `assumption` ist Schlüsselwort der
Edition 1, obwohl das Konstrukt v1.1 ist. Das folgt dem vorhandenen
Muster: `campaign`, `persist`, `port`, `node`, `arm` und `disarm` sind
ebenfalls Konstrukte späterer Stufen, deren Wörter seit Edition 1
reserviert sind — 2.5 verlangt für neue Wörter eine neue Edition, also
werden sie vorab belegt. Ein Programm, das heute `assumption` als
Bezeichner benutzt, gäbe es nicht; ein Programm, das es nach einem
Editionswechsel nicht mehr benutzen dürfte, wäre der teurere Fall.

Damit gruppiert M6 alles, was B1 und E5 brauchen: `SEM-8.10`, `SEM-13.3`,
`FMT-Hardware-Konfiguration`, die Prüfungen 28, 29, 32, 39 und 60 sowie
die drei `PAR-13.3`-Absätze.
