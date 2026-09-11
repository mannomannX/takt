# Bewertung: Einheiten auf Ganzzahlen vorziehen?

Stand 2026-09-11. Bezug: `plan/plan.md` Abschnitt 5.1 („Drei Kandidaten
könnten trotzdem früher kommen"), `plan/definition.md` 3.2 und 3.1,
Prüfung 38, `plan/feedback.csv` (FB-17, FB-37, FB-54).

Der Plan nennt drei Konstrukte, die vor v1.1 kommen könnten, weil sie
keinen Codegen brauchen, und empfiehlt: *„nach M3 bewerten, nicht jetzt
entscheiden."* Zwei sind entschieden (FB-39: `[const N]` abgelehnt;
FB-48 und `plan/feedback-design.md` 10: Hardware-Konfiguration
zurückgestellt). Dies ist die Bewertung des dritten.

**Ergebnis vorweg: vorziehen, als erster Baustein von M6.** Begründung
unten; die Gegenargumente stehen in Abschnitt 4.

---

## 1. Was fehlt heute

`int[mV]`, `u16[raw]`, `i32[inc/s]` sind in Grammatik und Referenz
vollständig beschrieben (3.2, `scalar_type` mit `@check 38`), werden
geparst und dann abgelehnt:

```
const BAUD : u32[Bd] = 921_600 Bd
                 ^^^^ error[SC-3]: Einheiten auf Ganzzahlen wird erst ab v1.1 unterstuetzt
```

Die Regel selbst ist entschieden und steht in 3.2: nominal wie bei Floats,
`*`/`/` kombinieren, `+`/`-`/Vergleich verlangen Gleichheit, `x.to(U2)`
nur bei ganzzahligem Faktor, sonst `x.to_float(U2)`. Prüfung 38 hält
genau diese Bedingung fest und steht seit M3 auf `definiert` — mit
getestetem Ablehnungsfall, aber ohne scharfe Regel.

---

## 2. Was die Praxis sagt

Die drei Treiberberichte sind der Belastungstest, den der Plan als
Entscheidungsgrundlage wollte. **Alle drei Autoren haben Einheiten auf
Ganzzahlen spontan benutzt**, ohne dass jemand sie darauf hingewiesen
hätte:

| Datei | Stellen mit `int[U]` |
|---|---|
| `feedback/test_servo.takt` | 16 |
| `feedback/test_uart.takt` | 5 |
| `feedback/test_dmx.takt` | 1 |

Der Servo-Treiber schreibt durchgehend `i32[inc/s]`, `u32[inc/s]`,
`i16[permille]` — die CANopen-Objekte *sind* ganzzahlige Größen mit
Einheit, und ein Inkrement pro Sekunde ist kein Float. Der UART-Treiber
schreibt `u32[Hz]` und `u32[Bd]` für Taktfrequenz und Baudrate. Das ist
kein Zufall und keine Bequemlichkeit: In der Treiberhälfte einer Sprache
für Steuerungstechnik ist die ganzzahlige dimensionierte Größe der
**Normalfall**, nicht die Ausnahme.

Heute muss jeder dieser Werte entweder auf `float` ausweichen — was auf
einem Kern ohne FPU laut 12.8 zwei Größenordnungen kostet und die
Bit-Genauigkeit von Registerwerten aufgibt — oder die Einheit weglassen
und damit genau die Sicherheit verlieren, für die 3.2 wirbt.

**Der Zusammenhang mit FB-17.** Der Servo-Bericht beklagt
`round(x / (1 inc/s)) as u32` als „Ritual". Ich habe den Vorschlag
`x.raw(U)` abgelehnt (plan/ideen-bewertung.md 7) — richtig, aber das
Ritual hat eine tiefere Ursache: Es steht dort nur, *weil* `x` ein Float
sein muss. Mit `i32[inc/s]` entfällt die Umrechnung ganz, statt eine
kürzere Schreibweise zu bekommen.

---

## 3. Was es kostet

Der Plan nennt es „Typregeln, keine neue Laufzeit". Das Nachsehen
bestätigt es und ist sogar günstiger als erwartet:

| Schicht | Stand |
|---|---|
| Grammatik | `scalar_type := "bool" \| int_type [ "[" unit_expr "]" ]` — **steht** |
| Parser | parst die Einheit bereits |
| AST | `ScalarType::Int { ty, unit }` — **steht** |
| MIR | `Type::Int { width, unit, range }` — **das Feld existiert** |
| Einheitenalgebra | `takt-sema/src/units.rs`, 551 Zeilen, typunabhängig — **steht** |
| Interpreter | Einheiten sind nominal — bis auf `.to()`, siehe unten |
| Codegen | dito |

Offen sind **vier Stellen**:

1. `lower/types.rs:302` — den Typ mit Einheit bauen statt abzulehnen.
2. `lower/expr.rs:333` — das Literal mit Einheit (`921_600 Bd`).
3. `lower/expr.rs:1001` — `.to()` und `.to_float()`, mit der einen neuen
   Regel: ganzzahliger Faktor erlaubt `to`, sonst verlangt er `to_float`.
4. `takt-interp/src/eval.rs:721` — `ConvertKind::To` trappt heute
   ausdrücklich für Integer (`bug("Einheiten auf Ganzzahlen ab M6")`).

Die ersten beiden spiegeln Logik, die für Floats direkt daneben steht.
Die dritte ist Prüfung 38: eine Faktorprüfung auf einem Rational, das
`units.rs` bereits exakt führt. Die vierte ist die einzige echte
Laufzeitarbeit — und sie ist klein, weil der ganzzahlige Faktor per
Prüfung 38 garantiert ist: eine Multiplikation und eine Division auf
`i128` statt der Float-Rechnung daneben. Der Trap-Text nennt M6 bereits
als Zeitpunkt; die Stelle ist also vorgesehen, nicht übersehen.

**Es bleibt trotzdem die kleinste der drei Kandidatenarbeiten.**
`[const N]` braucht Monomorphisierung über eine neue Variablenklasse; die
Hardware-Konfiguration braucht Parser, Format und Datenstruktur von null.
Einheiten auf Ganzzahlen brauchen drei `match`-Arme, eine Faktorprüfung
und eine Integer-Division.

---

## 4. Was dagegen spricht

**Es bricht die Reihenfolge des Plans.** Das ist das ernsthafteste
Argument, und es hat in den anderen beiden Fällen den Ausschlag gegeben.
Hier wiegt es leichter, aus zwei Gründen:

- Die anderen beiden Kandidaten wurden abgelehnt, *weil ihr Nutzen an
  etwas anderem hing* — `[const N]` am falschen Engpass, die
  Konfiguration an fehlenden Messwerten. Einheiten auf Ganzzahlen hängen
  an nichts: Die Regel ist entschieden, die Algebra steht, der Nutzen
  fällt sofort an.
- Der Plan selbst nennt sie als Kandidaten und begründet es mit „Typregeln,
  keine neue Laufzeit". Das Nachsehen bestätigt die Begründung; sie
  abzulehnen hieße, gegen die eigene Analyse zu entscheiden.

**Es löst nur eine Prüfung.** Von den 19 `definiert`-Prüfungen gibt es
genau Nummer 38 frei. Das stimmt — aber die Zahl ist hier das falsche
Maß: Prüfung 38 ist nicht das Ziel, sondern die Folge. Das Ziel ist, dass
die Treiberhälfte der Sprache ihre Größen typisieren kann.

**Halbe Features sind schlimmer als keine.** Der Plan kritisiert in
Abschnitt 1 genau das (Vorgehen B). Hier entsteht kein halbes Feature:
Einheiten auf Ganzzahlen sind in 3.2 vollständig definiert, haben ihre
eigene Prüfung und hängen an keinem anderen v1.1-Konstrukt. Sie sind
abgeschlossen, sobald sie gebaut sind.

---

## 5. Empfehlung

**Vorziehen — aber als erster Baustein von M6, nicht vor M4.**

Die Unterscheidung ist wichtig. „Vorziehen" hieße nicht, sie *jetzt*
zwischen M3 und M4 zu schieben: M4 ist der Codegen, und er ist der
Meilenstein, der die zweite Semantikquelle bringt (differentielles Testen
Interpreter ≡ nativ). Das ist der größere Gewinn und sollte nicht warten.

Sinnvoll ist die Reihenfolge **M4 → Einheiten auf Ganzzahlen → Rest von
M6**: Dann prüft der differentielle Test die neue Typregel von Anfang an
mit, und die Standardbibliothek (11.4, ohnehin M6) kann ihre
Integer-Varianten (`pid_i`, `lowpass_i`, 3.2) gleich mit Einheiten
schreiben, statt sie später nachzurüsten.

**Was jetzt dafür zu tun ist: nichts.** Prüfung 38 hat ihren
Ablehnungstest seit M3 — genau wie es `plan/m3.md` vorgesehen hat, damit
ein Vorziehen „nur noch den Testfall austauscht statt ihn zu erfinden".
Die Vorarbeit ist geleistet.

---

## 6. Was daraus für die Inventur folgt

| ID | vorher | nachher |
|---|---|---|
| `SC-38` | M3 (Gate), `definiert` | **M6 (früh)**, `definiert` |
| `SEM-3.2` (Teil Integer) | v1.1 | unverändert v1.1, aber am Anfang von M6 |

Mehr ist nicht zu ändern: Die Stufe bleibt v1.1 (kein Breaking Change,
2.5), nur die Position innerhalb von M6 wird benannt. Die Entscheidung
steht als FB-55 in `plan/feedback.csv`.
