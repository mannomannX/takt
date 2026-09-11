# Bewertung der Verbesserungsvorschläge (`feedback/ideen_dmx.md`)

Stand 2026-09-11. Bezug: `feedback/ideen_dmx.md`, `plan/definition.md` v0.2.8,
`plan/feedback.csv`, `plan/plan.md`.

Der Autor kannte eine ältere Referenz und den DMX-Treiber (`feedback/`).
Jeder Vorschlag ist unten gegen die aktuelle Fassung geprüft; wo etwas
inzwischen existiert oder sich beim Nachstellen anders verhält, steht das
dabei.

**Vorweg zur Rahmung.** Die Schärfung am Anfang des Dokuments — Takts Edge
sei nicht Sicherheit oder Determinismus, sondern die Kombination aus
bitgenauer Simulation, mitgeführten Budgets und einer Semantik für beide
Programmhälften — ist richtig und nützlich. Sie trifft genau das, was
0.1 und 0.2 behaupten, aber nirgends als Auswahlkriterium formulieren.
Der daraus abgeleitete Befund trägt ebenfalls: **Die Referenz beweist
mehrere Sätze, deren Auszahlung sie nicht einkassiert.** Das ist die
stärkste Beobachtung des ganzen Dokuments.

Was der Autor nicht wissen konnte: Fünf seiner Punkte (E2, E4, E6, Teile
von E3 und E8) sind seit dem DMX-Durchgang erledigt oder als Irrtum
geklärt. Sie stehen unten trotzdem, weil die Begründung zählt.

---

## Übersicht

| | Vorschlag | Urteil | Stufe |
|---|---|---|---|
| **A1** | Safe-State-Latenz berechnen, `within` | **annehmen — stärkster Vorschlag** | v1 (M3/M4) |
| A2a | Maschinen-Replay | annehmen | M6 |
| A2b | Satz Kompositionalität | annehmen (Doku) | M6 |
| A3 | Trace-Hashkette, `verify-trace` | annehmen, verkleinert | M6 |
| **B1** | `assumption` mit Monitor | **annehmen — zweitstärkster** | M6 |
| B2 | Block-Verträge | annehmen | M6 |
| B3 | Check-Klassifikation aus dem Beweis | annehmen; Budget-Effekt **ablehnen** | M6 / — |
| C1 | `takt certify` | annehmen als Sammelziel | M8+ |
| C2 | TCB-Manifest, `tcb_policy` | annehmen | M6 |
| C3 | Schatten-Interpreter | annehmen als Option | v2/v3 |
| D1 | Deklarierte Budgets je Maschine | **annehmen — bestes Aufwand/Wirkung** | M6 |
| E1 | const-Generics vorziehen | **ablehnen** (siehe FB-39) | M6 bleibt |
| E2 | `_` als Wegwerf-Ziel | **hinfällig** — nie nötig gewesen | — |
| E3 | `mark`/`patch` | teilweise annehmen | M6 |
| E4 | Record-Array-Konstanten | **hinfällig** — geht längst | — |
| E5 | Einheiten gegen Hardware-Konfiguration | **annehmen — unterschätzt** | M6 |
| E6 | `follows`-Abort-Lint | **erledigt** (FB-38) | M6 |
| E7 | `alert`-Polaritäts-Lint | annehmen | M6 |
| E8 | Profil-Validierung | halb erledigt, halb annehmen | M6 |

---

## A1. Safe-State-Latenz — annehmen, und zwar zuerst

**Prüfung.** Die Formel stimmt. Ich habe jede Größe gegen die Referenz und
die MIR geprüft:

| Term | Grundlage | in der MIR? |
|---|---|---|
| `D_detect = n_m · T0` | 1.3 (Periode) | ja: `Machine::period` |
| `D_confirm = ceil(d/P_m) · P_m` | 5.6, Prüfung 36 | ja: `Confirm` an `check`/`alert` |
| `D_prop = 0` bei `abort` | 5.4 (Abort-Phase, selber Tick) | ja: `StmtKind::Abort` |
| `D_prop = 1·T0` bei `pub var` | 7.2 (Unit-Delay) | ja: Leser/Schreiber bekannt |
| `D_commit = 0` bei `asap` | 5.2 Entry-Tick-Regel | ja: `system: output_timing` |

Der Kern des Arguments ist die **Entry-Tick-Regel** (5.2, Punkt 4):
„`check`s wirken, `-> ZIEL` sind wirkungslos. Dadurch werden die
Invarianten des neuen Zustands geprüft, *bevor* die Outputs des Ticks
committet werden." Die Folgerung des Autors ist korrekt und schärfer als
alles, was die Referenz heute daraus macht: Bei `asap` nimmt der Output den
unsicheren Wert **nie an** — die Latenz ist für die physikalische Welt
null, nicht „kurz".

**Warum das der stärkste Vorschlag ist.** Er verlangt fast nichts, was
nicht schon da ist. Die Budgets stehen seit M3 (`activation`,
`fault_path` als `CostVec`), die Perioden stehen, der Fault-Wald wird von
Prüfung 9 ohnehin durchlaufen. Was fehlt, ist die *zweite Hälfte* — die
Anzahl Ticks — und die ist ein Graph-Durchlauf über vorhandene Daten.

Der Autor nennt das selbst „die einfachste der Liste". Das stimmt, und es
ist der Grund, warum sie zuerst kommen sollte: Sie ist ohne `prove`, ohne
Kalibrierung und ohne Runtime demonstrierbar.

**Eine Einschränkung, die das Dokument nicht macht.** `D_safe` in
*Nanosekunden* braucht `T0` — das ist eine Programmangabe und damit da.
Aber die Aussage „bewiesene Worst-Case-Reaktionszeit" gilt nur, solange
die Schedulability hält, und die braucht `c_target` aus der Kalibrierung
(13.8, M5). Ohne sie ist `D_safe` eine Zahl in Ticks mal nominalem `T0` —
richtig unter der Annahme, dass jeder Tick eingehalten wird. Das ist eine
saubere und ehrliche Aussage, aber sie muss so beschriftet sein. Ich
würde den Report deshalb zweispaltig ausgeben: **Ticks** (exakt, immer) und
**Zeit** (exakt, sobald `tick_source` und Schedulability stehen).

**`within` als Deklaration.** `check p < LIMIT, "…" within 5 ms` ist der
richtige Zuschnitt: Es ist keine neue Semantik, sondern eine Anforderung an
eine Zahl, die der Compiler ohnehin ausrechnet — dieselbe Konstruktion wie
`cost`/`stack` an einer nativen Funktion (4.5). Compile-Fehler bei
Überschreitung.

**Empfehlung:** Annehmen, in zwei Schritten.
- **M3/M4:** Berechnung plus Report (Ticks exakt, Zeit unter Vorbehalt),
  Fault-Erreichbarkeitsmatrix. Kein Sprachbau nötig.
- **M6:** `within` als deklarierte Anforderung, sobald die Kalibrierung
  die Zeitspalte trägt.

Ein Satz 9.4.5 in Abschnitt 9 ist angemessen — er ist ein Korollar aus
Lemma 9.3.1 und der Abort-Phase, kein neuer Beweis.

---

## A2. Unit-Delay: Replay und Kompositionalität

### (a) Maschinen-Replay — annehmen

**Prüfung.** Die Voraussetzung stimmt: Die Lesemenge jeder Maschine ist
statisch bekannt, 11.2 berechnet sie bereits für den Ψ-Doppelpuffer. Eine
Maschinen-Scheibe ist damit wohldefiniert.

Der Workflow „Feldfehler wird automatisch zum Unit-Test" ist die
praktischste Idee des ganzen Dokuments. Sie trifft eine Kostenstelle, die
jeder kennt: das Nachstellen eines Feldfehlers kostet Stunden bis Wochen
und gelingt oft nicht.

**Eine Voraussetzung, die das Dokument übergeht.** Damit die Scheibe
*allein* abspielbar ist, muss auch der Stream-Zustand zum Startzeitpunkt
rekonstruierbar sein — das Fenster, die Cursor, die Zähler. Bei einem
Mitschnitt ab Tick 0 ist das trivial; bei einem Ausschnitt ab Tick k
braucht die Aufzeichnung einen Zustands-Schnappschuss, nicht nur die
Eingaben. Das ist machbar (der Zustand ist endlich und statisch
dimensioniert), aber es ist mehr als ein Filter über den Trace.

**Empfehlung:** Annehmen für M6, mit der Präzisierung: `--machine`
extrahiert Eingaben *und* den Anfangszustand der Maschine. Ohne den zweiten
Teil ist der Extrakt nur ab Tick 0 gültig.

### (b) Assume-Guarantee ist zirkelfrei — annehmen (Doku)

**Prüfung.** Das Argument ist mathematisch korrekt und wichtig. Unter
Unit-Delay greift jeder Schritt nur auf k−1 zu, die Induktion läuft über
die Zeit statt über die Komponenten, und damit entfällt die
Fixpunktbildung. Mit `follows` bleibt es gültig, weil die Kanten azyklisch
sind (Prüfung 33) — die Induktion läuft dann entlang der topologischen
Ordnung innerhalb des Ticks.

Die praktische Folge, die der Autor nennt, ist der eigentliche Wert:
`takt prove` muss nie das Gesamtsystem in einen Solver werfen. Das ist der
Unterschied zwischen „skaliert bis 5 Maschinen" und „skaliert bis 50".

**Empfehlung:** Als Satz in Abschnitt 9 aufnehmen (drei Absätze), sobald
`property` gebaut wird. Er rechtfertigt Entscheidung 6 rückwirkend und ist
die Grundlage dafür, dass B1 und B2 überhaupt tragen.

---

## A3. Trace als Beweismittel — annehmen, verkleinert

**Prüfung.** Logik-Hash, kanonisches Traceformat (`grammar/trace.md`) und
Satz 9.4.4 sind da. Die Hashkette `h_k = H(h_{k-1} ‖ I_k ‖ O_k)` ist
darauf aufsetzbar, ohne die Semantik zu berühren.

**Was ich anders sehe.** Der Autor listet drei Dinge; sie sind nicht
gleich viel wert.

- **Hashkette und `verify-trace`**: Klarer Gewinn, kleine Kosten, keine
  Sprachänderung. Annehmen.
- **Signierter Lauf-Header**: Hier wird es heikel. Eine *Signatur*
  verlangt Schlüsselverwaltung, und die gehört nicht in einen Compiler.
  Der Header sollte die Hashes tragen (Logik, Binary, Kettenende); wer
  sie signiert und womit, ist eine Frage des Betriebs, nicht der Sprache.
  **Verkleinern auf: Header trägt die Hashes, Signatur ist außerhalb.**

Die genannten Anwendungen (Eichtechnik, Emissionsnachweis, Haftung) sind
real, aber sie sind Folge, nicht Begründung — der Gewinn steht schon,
wenn ein Dritter mit demselben Binary denselben Trace reproduziert.

**Empfehlung:** Annehmen für M6, ohne den Signaturteil.

---

## B1. `assumption` mit Monitor — annehmen, zweitwichtigster Punkt

**Prüfung.** Der Autor liest 3.4 genau richtig. Die Referenz sagt dort:
„Ein `assume` gibt es nicht; es wäre unsound" — und der Kontext ist
unmissverständlich die *Intervallanalyse*, wo eine unbewiesene Annahme die
Typsicherheit bräche. Auf Eigenschaftsebene ist das eine andere Frage: Dort
beschränkt eine Annahme die Beweisverpflichtung, nicht die
Laufzeitgarantie.

Das Problem, das er beschreibt, ist real und tödlich für die Akzeptanz:
Ohne Umgebungsannahmen produziert BMC physikalisch unmögliche
Gegenbeispiele, und nach dem dritten schaltet das Team das Werkzeug ab.

**Der Entwurf ist gut, aus einem Grund, den das Dokument nur andeutet.**
`assumption … with monitor = true` ist *dieselbe Konstruktion*, die
`property … with monitor = true` schon hat (2.3, `property_decl`). Das ist
keine neue Idee, sondern eine vorhandene auf einen zweiten Fall angewandt
— und die Soundness-Schleife schließt sich genau so, wie die Referenz sie
für `check` gegen `alert` bereits gezogen hat: Du darfst annehmen, aber
du musst beobachten.

**Der stärkste Teil ist der zweite.** Kanal-Attribute — `max_slew`,
`debounce`, `max_age`, Range — sind bereits deklariert und werden vom
defensiven Treiberrand (12.6) zur Laufzeit *erzwungen*. Der Beweiser darf
sie deshalb ohne Zusatzaufwand als Annahmen übernehmen. Das ist die zweite
Auszahlung des Treiberrands, und sie kostet nichts: Die Annahmen stehen
schon in der Quelle, sie müssen nur gelesen werden.

**Empfehlung:** Annehmen für M6, zusammen mit `property`. Die automatische
Übernahme der Kanal-Attribute sollte Teil der ersten Fassung sein, nicht
eine spätere Verfeinerung — ohne sie schreibt jeder dieselben fünf
`assumption`-Zeilen von Hand.

---

## B2. Block-Verträge — annehmen

**Prüfung.** Die Voraussetzung stimmt: Ein Block ist endlich-zustandig und
geschlossen (5.7: kein I/O, keine globalen Lesevorgänge). Damit ist er per
BMC einzeln beweisbar, und an der Aufrufstelle wird der Vertrag als Fakt
benutzt statt in den Block hinein bewiesen.

Die drei genannten Effekte treffen zu. Der dritte ist der interessanteste
und wird im Dokument unterschätzt: Ein `ensures` bringt eine Relation an
die Aufrufstelle, **ohne Laufzeitprüfung**. Heute muss man dafür eine
range-typisierte Zwischengröße einführen (3.4), die einen impliziten Check
erzeugt. Block-Verträge sind damit auch ein Beitrag zur Kennzahl aus
FB-19 — sie senken die Zahl der impliziten Prüfungen, statt sie nur besser
zu sortieren.

**Eine Abgrenzung, die das Dokument nicht zieht.** `requires` und `ensures`
sind Beweisverpflichtungen, keine Laufzeitprüfungen. Ein `requires`, das
zur Laufzeit prüft, wäre ein verstecktes `check` mit unklarem Fault-Ziel.
Die Referenz sollte das ausdrücklich sagen, sonst erwartet es jeder.

**Empfehlung:** Annehmen für M6 mit `property`. Die Bibliothek bekommt
ihre Verträge dann im selben Zug (11.4 ist ohnehin M6).

---

## B3. Check-Klassifikation — annehmen; Budget-Effekt ablehnen

**Prüfung.** Die Klassifikation (bewiesen unerreichbar / erreichbar mit
Pfad / unentschieden) ist eine gute Idee und beantwortet eine Frage, die
die heutige Kennzahl nicht beantwortet: Welche Prüfung trägt Gewicht im
Sicherheitsargument?

Der Report „380 bewiesen unerreichbar, 29 erreichbar mit Pfad, 3
unentschieden" ist ein Artefakt, mit dem ein Assessor arbeiten kann.
Annehmen.

**Den Budget-Effekt lehne ich ab.** Der Vorschlag, bewiesen unerreichbare
Checks aus dem Worst-Case-Budget (9.4.3) auszuklammern, klingt
betriebswirtschaftlich attraktiv und ist genau deshalb gefährlich:

1. **Der Beweis gilt unter Annahmen.** Nach B1 stammen die Annahmen zum
   Teil aus Kanal-Attributen, die der *Treiberrand* erzwingt — also aus
   der TCB. Ein Budget, das auf einem Beweis unter TCB-Annahmen beruht,
   verliert seine Gültigkeit, wenn der Treiber falsch liegt. Das Budget
   ist aber gerade die Größe, die auch dann halten muss.
2. **Der Autor sagt selbst**, die Checks sollten nicht wegoptimiert
   werden — Verteidigung in der Tiefe gegen TCB-Fehler. Wenn der Check
   ausgeführt wird, kostet er auch. Ihn aus dem Budget zu nehmen, während
   er läuft, macht das Budget zu einer Schätzung statt zu einer Schranke.
3. Die Referenz zieht diese Linie an anderer Stelle bewusst: Das
   Kostenmodell ist eine *obere Schranke*, keine Erwartung (9.4.3).

**Empfehlung:** Klassifikation annehmen (M6), Budget-Ausklammerung nicht.
Wenn der Gewinn gebraucht wird, ist der ehrliche Weg ein *zweites*,
ausgewiesenes Budget („unter den Annahmen A") neben dem harten — nicht
eine Absenkung des harten.

---

## C1. `takt certify` — annehmen als Sammelziel

**Prüfung.** Der Befund stimmt: Nicht der Code kostet, sondern die
Evidenz. Und die Rohdaten sind da — `req`-Referenzen (v1.2), Check-IDs,
Zustandsgraph, Fault-Wald, Coverage, Budgets mit Herkunft, TCB-Grenze.

**Was ich anders einordne.** `takt certify` ist kein Vorschlag, sondern
ein *Bündel* aus A1, B3, C2 und `takt size`. Es lohnt sich nicht als
eigener Meilenstein, sondern als das Ziel, auf das die vier hinarbeiten.
Wer es als Feature plant, baut ein Dokumentenwerkzeug, bevor die Inhalte
existieren.

**Empfehlung:** Als Sammelziel führen (M8+), nicht als eigenständige
Arbeit. Die Teile einzeln bauen; das Werkzeug ist dann eine Woche Arbeit
statt einer Sackgasse.

---

## C2. TCB-Manifest und `tcb_policy` — annehmen

**Prüfung.** 9.5 nennt die TCB in Prosa; der Compiler kennt sie exakt. Ein
Manifest im Lauf-Header ist billig und verwandelt eine Absichtserklärung
in ein prüfbares Artefakt.

`tcb_policy = curated_only | allowlist(…)` ist der richtige Zuschnitt: Es
ist eine Projektrichtlinie, die der Compiler erzwingt — dieselbe Klasse
wie `certification` (2.5) oder `fault_is_fail`. Kein neues Konzept.

**Empfehlung:** Annehmen für M6. Das Manifest fällt beim Bau von 4.5
(Projekt-Natives) ohnehin an.

---

## C3. Schatten-Interpreter — annehmen als Option

**Prüfung.** Das Argument ist stark und stimmt: Weil die Semantik
bitidentisch ist (Satz 9.4.4), muss ein Zustands-Hash-Vergleich exakt
aufgehen. Ein Codegen-Fehler wird damit zur Laufzeit entdeckt, nicht nur
im Test.

Die TD-Einstufung ist das eigentliche Argument — TCL 3 → 1 macht
Werkzeugqualifikation bezahlbar, und das ist bei einem neuen Compiler ohne
Zertifizierungshistorie der teuerste Posten überhaupt.

Der Autor nennt die Kosten ehrlich (eine Größenordnung langsamer, für
50 µs nicht machbar) und liefert die Entschärfung gleich mit: stichprobenartig
oder nur für sicherheitsrelevante Maschinen.

**Empfehlung:** Annehmen als Option für v2, als Qualifikationsbaustein für
v3. Nichts davon ist jetzt zu bauen, aber es sollte im Stufenplan stehen,
damit die MIR-Schnittstelle es nicht versehentlich unmöglich macht.

---

## D1. Deklarierte Budgets je Maschine — annehmen

**Prüfung.** Der Befund stimmt: Heute ist das Budget eine globale Prüfung
am Ende (7.2). In einem Projekt mit mehreren Teams fällt die
Überschreitung bei der Integration auf, wenn sie teuer ist.

```
machine current_ctrl every 50 us with budget = {ram = 2 KiB, wcet = 20 us}:
```

**Das ist das beste Aufwand/Wirkung-Verhältnis der ganzen Liste.** Die
Rechnung steht seit M3 (`Machine::budget` mit `activation` und
`fault_path`, `takt size` je Posten). Was fehlt, ist ein Attribut und ein
Vergleich. Der `wcet`-Teil braucht `c_target` (M5), der `ram`-Teil nicht —
er ist sofort möglich.

`takt size --baseline vorher.json` als CI-Gate ist die zweite Hälfte und
genauso billig. Budget-Regression wird damit so sichtbar wie ein
fehlgeschlagener Test.

**Empfehlung:** Annehmen. `ram` früh (M4), `wcet` mit der Kalibrierung
(M6). `--baseline` unabhängig davon, sobald `takt size` stabil ist —
das ist heute.

---

## E1–E8: die Korrekturen

**E1 — const-Generics vorziehen: ablehnen.** Das ist FB-39, und die
Begründung steht in `plan/feedback-design.md`: Die Sequenzierung ist nicht
der Engpass. Von den 117 Zeilen Serialisierung im DMX-Treiber entfielen 24
auf tote Bindungen (E2, nie nötig) und 11 von 19 Schleifen auf reine
Byte-Kopien — letztere sind seit `append` erledigt (FB-42). Was `reader`/
`writer` dann noch spart, ist Länge, nicht Möglichkeit.

**E2 — `_` als Wegwerf-Ziel: hinfällig.** `buf.push(x)` steht längst als
blanke Anweisung; ein Ziel war nie nötig (FB-33). Die „Wüste aus toten
Bindungen" entstand aus einer Fehlannahme. 4.4 sagt es jetzt ausdrücklich.

Der zweite Teil — **statisch bewiesenes `push`** — bleibt eine eigene,
gute Idee: Wenn die Intervallanalyse `len + n <= N` zeigt, ist das
Ergebnis konstant `true`. Nur ist der Nutzen nach E2 klein, weil niemand
mehr eine Bindung schreiben muss. **Zurückstellen**, nicht ablehnen.

**E3 — Backpatching-Idiom: teilweise annehmen.** Das Beispiel fehlte
tatsächlich; 3.9 hat es seit FB-35, und es ist durchgerechnet. Der
Vorschlag `w.mark() -> Mark` plus `w.patch_u16(m, v)` geht darüber hinaus
und ist gut: Er bindet die Position an ein Objekt, dessen Schranke beim
Erzeugen bewiesen ist, statt einen nackten Index zu übergeben. Das gehört
zu `writer` (M6), nicht davor.

**E4 — Record-Array-Konstanten: hinfällig.** Geht längst; 3.7 hat seit
FB-34 ein Beispiel. Der Autor vermutet selbst richtig.

**E5 — Einheiten gegen die Hardware-Konfiguration: annehmen, und das ist
der unterschätzteste Punkt des Dokuments.** Die Konfiguration trägt je
Channel Einheit, Skalierung und Range (8.10, Feldtabelle). Ein Programm,
das `@ hw("daq1/ai0")` als `float[bar]` bindet, während die Konfiguration
`psi` sagt, ist heute unentdeckt — und das ist ausgerechnet die Stelle, an
der Einheitenfehler *aus dem Programm heraus* nicht mehr sichtbar sind.
Die ganze Einheitenrechnung von 3.2 endet am Channel-Rand.

Das ist keine Ergonomie, sondern eine Lücke in der zentralen Zusage. Der
Autor nennt sie „Prüfung 59"; diese Nummer ist inzwischen vergeben (FB-10,
Treiberstufe). Sie wird **Prüfung 60**.

**E6 — `follows`-Abort-Lint: erledigt.** Ist FB-38, bereits in 10.1 und
der Inventur (Prüfung 33, Klasse `F / W`). Der Autor hat denselben Befund
unabhängig gefunden — ein gutes Zeichen für beide Analysen.

**E7 — `alert`-Polaritäts-Lint: annehmen.** 5.6 begründet die umgekehrte
Polarität sauber (der Zähler steigt, solange die Alert-Bedingung
*zutrifft*), aber genau deshalb wird `alert x < LIMIT` geschrieben, wo
`alert x >= LIMIT` gemeint ist. Ein Lint, der bei einem `alert` mit
derselben Vergleichsrichtung wie ein benachbarter `check` warnt, kostet
wenig. **Kleiner Punkt, aber richtig.**

**E8 — Profil-Validierung: halb erledigt.** Nachgestellt:

- *Unbekannter Param im Profil* → wird bereits doppelt abgelehnt
  (`SC-2: nicht definiert`, `SC-3: kein Parameter`). Nichts zu tun.
- *Fehlender Param im Profil* → nimmt heute still den Default. Der
  Vorschlag (Default plus Alert) ist richtig; ich würde es als **Warnung
  zur Übersetzungszeit** machen statt als Laufzeit-Alert: Es ist eine
  Eigenschaft des Profils, nicht des Laufs, und zur Laufzeit ist es zu
  spät.

---

## Was ich zuerst machen würde

Der Autor empfiehlt A1 zuerst und B1 als zweites. **Beidem stimme ich zu**,
mit einer Präzisierung bei A1 und einer Ergänzung.

**1. A1, aufgeteilt.** Die Tick-Rechnung ist heute möglich und braucht
weder `prove` noch Kalibrierung — das ist der Prototyp, den der Autor
beschreibt, und er ist in M3/M4 machbar. Die *Zeitspalte* und `within`
folgen mit der Kalibrierung. Die Trennung ist wichtig, weil die Zahl sonst
mehr verspricht, als sie unter nominalem `T0` halten kann.

**2. D1 (`ram`-Teil) und `size --baseline`.** Der Autor stuft D1 als
„sehr billig, hohe Wirkung" ein, priorisiert es aber nicht. Ich würde es
vorziehen: Es ist die einzige Ergänzung der ganzen Liste, die *heute*
vollständig baubar ist — die Rechnung steht seit M3 —, und sie verhindert
eine Fehlerklasse, die mit jedem zusätzlichen Team teurer wird.

**3. B1.** Sobald `property` gebaut wird (M6), und zwar mit der
automatischen Übernahme der Kanal-Attribute in derselben Fassung.

**Was ich nicht vorziehen würde:** E1 (abgelehnt, siehe FB-39) und C1
(Bündel, kein Einzelvorschlag). Und den Budget-Effekt aus B3 gar nicht —
er ist die einzige Stelle des Dokuments, an der eine Garantie gegen eine
Kennzahl eingetauscht wird.
