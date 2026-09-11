Vorweg eine Schärfung, weil jede Verbesserung daran gemessen werden muss.

**Takts Edge ist nicht Sicherheit und nicht Determinismus** — beides verkaufen SCADE, SPARK und Lustre seit Jahrzehnten. Die Edge ist eine Kombination, die es sonst nicht gibt:

> Die Simulation *ist* das System (bitgenau, als Satz), das Programm trägt seine eigenen Zeit- und Speicherbudgets, und die Protokoll-/Ablaufhälfte lebt unter derselben Semantik wie die Regelungshälfte.

Alles, was diese drei Achsen verstärkt, ist Hebel. Alles andere ist Politur.

Auffällig ist nun: **Die Referenz hat mehrere Sätze schon bewiesen, deren Auszahlung sie nie einkassiert.** Das ist der größte Einzelbefund, und dort fange ich an.

---

# A. Was bereits bewiesen ist und nicht eingelöst wird

## A1. Die Safe-State-Latenz ist berechenbar — und wird nicht berechnet

Für IEC 61508 und ISO 26262 ist *die* Zahl die **Fault Tolerant Time Interval** bzw. Process Safety Time: Wie lange von „Bedingung tritt ein" bis „alle Aktoren stehen sicher"? Diese Zahl wird heute überall geschätzt, per Messung plausibilisiert und im Safety Case mit Marge versehen.

Takt kann sie **exakt ausrechnen**, und zwar aus Größen, die der Compiler ohnehin hat.

Sei `c` eine Bedingung, deren Verletzung einen Fault auslösen soll, geprüft in Maschine m mit Periode n_m·T₀.

```
D_detect  = n_m · T0                  # schlimmster Fall: c wird kurz nach m's Aktivierung wahr
D_confirm = ceil(d / P_m) · P_m       # nur bei "check ... for d"; sonst 0
D_prop    = 0                         # abort:      Abort-Phase, selber Tick (5.4)
          | 1 · T0                    # pub var:    Unit-Delay
          | hops · T0                 # v2:         Knotengrenze (12.9)
D_commit  = 0                         # asap: Commit am Tick-Ende, Output nahm nie einen falschen Wert an
          | 1 · T0                    # boundary (LET)
D_safe   <= D_detect + D_confirm + D_prop + D_commit
```

Dazu kommt physisch `guard(o)` des Treibers, gemessen in 13.8 und bereits in der Hardware-Konfiguration.

Drei Dinge machen das stark:

**Erstens**, die Entry-Tick-Regel liefert eine Aussage, die schärfer ist als „schnell": Bei `asap` wird der unsichere Output **nie committet**. Die Latenz ist nicht „kurz", sie ist für die physikalische Welt *null*. Das ist eine qualitativ andere Behauptung als in jedem C-System, und sie steht heute nur als Nebensatz in 0.2.

**Zweitens**, die Abort-Phase macht `D_prop = 0` über Maschinengrenzen. Eine Maschine mit Periode 1 s bekommt den Abort im selben Tick. Der Compiler könnte daraus eine Empfehlung ableiten: *„Dieser Interlock propagiert per `pub var` mit +1 Tick; `abort` wäre null."*

**Drittens** — und das ist der Punkt, den ich für den wertvollsten der ganzen Analyse halte — **das Ausführungsbudget F_m ist schon da.** 9.4.3 berechnet den Fault-Pfad, 7.2 prüft, dass Σ F_m in den Tick passt. Es fehlt nur die zweite Hälfte: die *Anzahl Ticks*. Beide Hälften zusammen ergeben eine bewiesene Worst-Case-Reaktionszeit in Nanosekunden.

**Was ich vorschlage:**

- Satz 9.4.5 (Safe-State-Latenz) in Abschnitt 9, mit dem Beweis über Lemma 9.3.1 und der Abort-Phase.
- Eine Spalte im Report je `check`, `expect` und `abort`: berechnete `D_safe` mit Aufschlüsselung.
- Eine **Fault-Erreichbarkeitsmatrix**: je Zustand, welche Fault-Ziele in wie vielen Ticks erreichbar sind. Das ist ein Graph-Durchlauf über den Fault-Wald, den Prüfung 9 ohnehin macht.
- Eine deklarierbare Anforderung: `check p < LIMIT, "…" within 5 ms` — Compile-Fehler, wenn `D_safe` sie überschreitet.

**Kosten:** Keine Sprachänderung außer `within`. Der Rest ist Report.
**Wirkung:** Das Argument im Verkaufsgespräch verschiebt sich von „unsere Sprache ist sicher" zu „unser Compiler gibt Ihnen Ihre FTTI als bewiesene Zahl". Das kann sonst niemand.
**Stufe:** v1 für die Berechnung, v1 für `within`.

---

## A2. Das Unit-Delay erlaubt kompositionale Verifikation — und wird nur für Ordnungsunabhängigkeit benutzt

Entscheidung 6 (Unit-Delay) wird in der Referenz mit einem einzigen Argument begründet: Satz 9.4.1, die Ausführungsreihenfolge ist irrelevant. Das ist wahr, aber es ist die kleinere Hälfte.

Aus 9.4.1 folgt unmittelbar:

> `step_m` liest ausschließlich σ_m, I_k, die statisch bekannte Teilmenge von Ψ_k, die m liest, sein eigenes Stream-Fenster, und (mit `follows`) `fresh[m']` gefolgter Maschinen.

Daraus folgen zwei Dinge, die die Referenz nicht ausspricht:

### (a) Maschinen-Replay als eigenständige Operation

Eine Aufzeichnung enthält heute den ganzen Lauf. Aber weil die Lesemenge jeder Maschine statisch bekannt ist (11.2 berechnet sie bereits für den Ψ-Doppelpuffer), lässt sich eine **Maschinen-Scheibe** aufzeichnen: I_k eingeschränkt auf ihre Channels, Ψ_k eingeschränkt auf ihre Lesevorgänge, ihr Stream-Fenster.

Das ergibt einen Workflow, den ich in Embedded nirgends kenne:

```
takt replay run_2026_09_11.trace --machine bms --extract > bms_field_case_017.trace
takt test --machine bms --trace bms_field_case_017.trace
```

**Ein Feldfehler wird automatisch zu einem Unit-Test.** Nicht „wir haben versucht, es nachzustellen" — die exakten Eingaben, bitgenau, in Sekunden statt Stunden, und der Test bleibt in der Regressionssuite.

Das ist die Sorte Fähigkeit, die eine Toolchain-Entscheidung kippt, weil sie eine Kostenstelle trifft, die jeder kennt und keiner beziffert.

### (b) Assume-Guarantee ist unter Unit-Delay zirkelfrei sound

Der eigentliche mathematische Gewinn. Kompositionale Verifikation scheitert normalerweise an Zirkularität: A garantiert X unter Annahme Y, B garantiert Y unter Annahme X — und der Beweis dreht sich im Kreis.

Unter Unit-Delay gibt es diesen Kreis nicht. Wenn A in Tick k über B annimmt, ist der Wert von Tick k−1 gemeint. Die Induktion läuft über die Zeit, nicht über die Komponenten. Formal: Sei P_A eine Eigenschaft von A unter Annahme Q_B über B's veröffentlichte Größen, und P_B eine Eigenschaft von B unter Annahme Q_A. Gilt P_A ⊨ Q_A und P_B ⊨ Q_B, so folgt P_A ∧ P_B für das Gesamtsystem per Induktion über k — ohne Fixpunktbildung, weil jeder Schritt nur auf k−1 zugreift.

Mit `follows` bleibt das gültig, weil die Kanten azyklisch sind: die Induktion läuft dann entlang der topologischen Ordnung innerhalb des Ticks.

**Praktische Folge:** `takt prove` muss nie das Gesamtsystem in einen SMT-Solver werfen. Es beweist je Maschine, mit den Ψ-Lesevorgängen als freien Variablen unter Annahmen. Das ist der Unterschied zwischen „skaliert bis 5 Maschinen" und „skaliert bis 50".

**Was ich vorschlage:**

- Satz 9.11 (Kompositionalität) in Abschnitt 9 — der Beweis ist drei Absätze und rechtfertigt Entscheidung 6 rückwirkend doppelt.
- `--machine`-Modus für `replay`, `test` und `prove`.
- Trace-Format erweitert um Maschinen-Scheiben (das Format ist ohnehin versioniert, 11.3).

**Stufe:** Replay v1.1, Beweis-Kompositionalität v1.1 zusammen mit `property`.

---

## A3. Der Trace ist ein Beweismittel und wird wie ein Logfile behandelt

Gegeben: Logik-Hash, aufgezeichneter Input-Strom, Satz 9.4.4. Daraus folgt eine Aussage, die sonst niemand machen kann:

> Aus *diesen* Eingaben folgt *dieses* Verhalten, nachprüfbar von jedem Dritten mit demselben Binary.

Das ist forensische Qualität. Heute fehlt nur die Verpackung:

- **Tick-Hash-Kette.** `h_k = H(h_{k-1} ‖ I_k ‖ O_k)` über die kanonische Trace-Zeile (das Format ist bereits kanonisch definiert, `grammar/trace.md`). Kosten: ein Hash je Tick, im `linux_rt`-Profil vernachlässigbar, auf MCU optional.
- **Signierter Lauf-Header** mit Logik-Hash, Binary-Hash (reproduzierbare Builds haben ihn schon, 11.3), Profil, Parametervektor, TCB-Manifest und dem End-Hash der Kette.
- **`takt verify-trace`** als Drittprüfer-Werkzeug: Binary plus Trace rein, Bestätigung oder Abweichungsposition raus.

Das schaltet Anwendungen frei, die ich vorher als Randfall genannt habe und die eigentlich ein eigener Markt sind: Eichtechnik, Emissionsnachweis, Manipulationsnachweis, Haftungsfälle. Und es ist im Zertifizierungsgespräch ein Argument, das über Software hinausgeht.

**Kosten:** Klein. **Stufe:** v1.1.

---

# B. Was den Beweisteil erst brauchbar macht

Abschnitt 9.4.3 behauptet, das System sei ein endlicher Transduktor und Eigenschaften seien per k-Induktion prüfbar. Das stimmt. Aber `takt prove` wird in der jetzigen Form an realen Programmen scheitern, aus drei Gründen.

## B1. Ohne `assume` liefert BMC nur Müll-Gegenbeispiele

Ohne Umgebungsannahmen wird der Modellprüfer antworten: „Gegenbeispiel: Tankdruck springt in einem Tick von 0 bar auf 400 bar, Zellspannung wechselt jeden Tick zwischen 2,0 V und 4,5 V." Physikalisch unmöglich, formal zulässig. Nach dem dritten solchen Gegenbeispiel schaltet das Team das Werkzeug ab.

Jeder brauchbare Modellprüfer braucht Assume-Guarantee. Die Referenz lehnt `assume` in 3.4 ab — aber dort geht es um die *Intervallanalyse*, wo ein `assume` tatsächlich unsound wäre. Auf Eigenschaftsebene ist es etwas anderes: es beschränkt die Beweisverpflichtung, nicht die Typsicherheit.

Der saubere Entwurf schließt die Lücke sofort mit:

```
assumption slew_is_physical: always(abs(tank_p - tank_p.prev) < 5 bar)
    with monitor = true
```

**Eine Annahme ist zugleich ein Monitor.** In der Simulation und optional auf Hardware wird sie überwacht; eine Verletzung ist ein FAIL-Befund (13.5), nie ein Fault. Damit ist die Soundness-Schleife geschlossen: Du darfst annehmen, aber du musst beobachten. Das ist genau die Konstruktion, die die Referenz für `check` gegen `alert` schon gewählt hat — sie muss nur auf Eigenschaften übertragen werden.

Für die Lesbarkeit wäre ein zweites Wort hilfreich: Annahmen über *Kanäle* sind oft schon deklariert (`max_slew`, `debounce`, `max_age`, Range). Der Beweiser sollte sie **automatisch als Annahmen übernehmen** — dann braucht man `assumption` nur für den Rest. Das ist der Punkt, an dem der defensive Treiberrand (12.6) seine zweite Auszahlung bekommt: er *erzwingt* die Annahmen zur Laufzeit, also darf der Beweiser sie benutzen.

Das ist meines Erachtens die wichtigste einzelne Ergänzung, die die Referenz braucht.

## B2. Block-Verträge machen Beweise kompositional und die Bibliothek zum Argument

Heute hat `block pid` keine Vor- oder Nachbedingung. Für eine Bibliothek, die in Sicherheitssysteme geht, ist das eine Lücke:

```
block rate_limiter[U](max_rate: float[U/s]):
    step(target: float[U], dt: Duration in tick..1 h) -> float[U]
        requires dt.as(s) > 0
        ensures abs(result - result.prev) <= max_rate * dt.as(s)
```

Ein Block ist endlich-zustandig und geschlossen (keine I/O, keine globalen Lesevorgänge). Das heißt: **Block-Verträge sind per BMC beweisbar, und zwar je Block einzeln.** An der Aufrufstelle wird der Vertrag als Fakt benutzt, statt in den Block hinein zu beweisen.

Das hat drei Effekte:

1. `takt prove` skaliert, weil Blöcke aus dem Suchraum fallen.
2. Die Standardbibliothek wird zu einem qualifizierbaren Artefakt: *jeder Block der Bibliothek trägt einen bewiesenen Vertrag.* Das hat SCADE nicht.
3. Die Intervallanalyse bekommt Futter. Heute muss man `var dp : float[bar] in 0..50 bar = p_out - p_in` schreiben, um eine Relation einzubringen (3.4). Ein `ensures` auf einem Block leistet dasselbe an der Aufrufstelle, ohne Laufzeitprüfung.

**Stufe:** v1.1 zusammen mit `property`.

## B3. Bewiesene Checks sind ein Zertifizierungsartefakt — und ein Performance-Gewinn

Das ist der Punkt, an dem sich der Beweisaufwand betriebswirtschaftlich rechnet.

Ein Programm hat typisch hunderte Checks: explizite Interlocks plus implizite Range-, Index-, Overflow- und Validitätsprüfungen. Heute sind sie eine aggregierte Kennzahl, aufgeschlüsselt nach Ursache (3.4). Das ist gut, aber es beantwortet nicht die Frage des Assessors: *Welche dieser Prüfungen trägt Gewicht im Sicherheitsargument?*

Mit `takt prove` lässt sich jeder Check klassifizieren:

| Klasse | Bedeutung | Konsequenz |
|---|---|---|
| **bewiesen unerreichbar** | Die Verletzung kann unter den Annahmen nicht eintreten | Kein Argument nötig; aus dem WCET-Worst-Case ausklammerbar |
| **erreichbar, Pfad bekannt** | Der Beweiser nennt die Eingabefolge | Muss im Safety Case behandelt sein; Gegenbeispiel ist ein Testfall |
| **unentschieden** | k-Induktion terminiert nicht in der Schranke | Bleibt Laufzeitprüfung, muss argumentiert werden |

Der Report sagt dann: *„Von 412 Prüfungen sind 380 bewiesen unerreichbar, 29 erreichbar mit Pfad, 3 unentschieden."* Das ist eine Aussage, mit der ein Assessor arbeiten kann, und sie ist automatisch aktuell.

Der Nebeneffekt ist die Antwort auf den Performance-Einwand. Bewiesen unerreichbare Checks gehen nicht in das Worst-Case-Budget (9.4.3) ein — der Beweis *ist* die Rechtfertigung. Das kann bei Regelpfaden zweistellige Prozente ausmachen.

**Wichtig:** Ich würde die Checks nicht wegoptimieren. Verteidigung in der Tiefe gegen TCB-Fehler bleibt wertvoll, und die Referenz argumentiert an anderer Stelle genauso (MPU-Schutzbereich, 12.3). Der Gewinn liegt im *Budget* und im *Artefakt*, nicht im Weglassen.

**Stufe:** v1.1 mit `prove`, Budget-Effekt v1.2.

---

# C. Was das Zertifizierungsgeschäft trägt

## C1. `takt certify` — Nachweise generieren statt zusammentragen

In einem realen 61508-Projekt kostet nicht der Code, sondern die Evidenz: Anforderungs-Trace-Matrix, Coverage-Report, WCET-Nachweis, Speichernachweis, FMEDA-Eingaben, Safety Manual. Das wird heute von Hand gepflegt, veraltet zwischen zwei Reviews und ist bei jeder Änderung neu zu machen.

Takt hat alle Rohdaten im Programm: `req`-Referenzen, stabile Check-IDs, den vollständigen Zustandsgraph, den Fault-Wald, Coverage aus der Simulation, Budgets mit Herkunft, die TCB-Grenze.

Ein `takt certify` sollte daraus erzeugen:

- **Trace-Matrix**: Anforderungs-ID → Checks/Transitionen, die sie umsetzen → Szenarien, die sie abdecken → Coverage-Status → Beweisstatus (B3). Immer aktuell, weil aus der Quelle.
- **Safe-State-Analyse je Output**: welche Zustände ihn schreiben, sein `safe`-Wert, welche Fault-Pfade ihn dorthin bringen, `D_safe` (A1). Das ist unmittelbare FMEDA-Eingabe und sonst reine Handarbeit.
- **Check-Inventar** mit Klassifikation nach B3.
- **Budget-Bericht** mit Herkunft (`takt size` liefert ihn bereits, er muss nur in das Dokument).
- **TCB-Manifest** (C2).
- **Werkzeug-Betriebsanleitung**: welche Compiler-Version, welche Flags, welche Konformitätsmessung, welche Edition — alles im Lauf-Header vorhanden.

Das ist kein Sprachfeature, sondern die Stelle, an der Takts Struktur zu Geld wird. Ein Team, das diese Dokumente generiert statt schreibt, spart im ersten Projekt mehr, als die Sprachumstellung kostet.

**Stufe:** v1.2 in Grundform, v3 als qualifiziertes Kit.

## C2. Die TCB als maschinenlesbares, erzwungenes Artefakt

9.5 nennt die TCB in Prosa. Der Compiler kennt sie exakt: jede `native fn`, jeder `job`, jeder Treiber, jeder `port` (v1.2), jede Projekt-Native.

Daraus sollte ein **TCB-Manifest** werden, das im Lauf-Header steht und Folgendes je Eintrag trägt: Name, Version, Quelle, `cost`/`stack`-Vertrag, Ergebnis der Konformitätsmessung (13.8), Hash der Implementierung.

Dazu eine Projektrichtlinie, die der Compiler erzwingt:

```
system:
    tcb_policy = curated_only      # keine Projekt-Natives
    tcb_policy = allowlist("crc_custom", "vendor_aes")
```

Für einen Assessor ist „hier ist exakt, was Sie vertrauen müssen, mit Evidenz je Posten" das wertvollste Dokument überhaupt — und es ist zugleich das, was 9.5 ehrlich vorbereitet, aber nicht zu Ende bringt.

**Stufe:** v1.1.

## C3. Schatten-Interpreter als Diversitätsargument (v2/v3)

Der Referenzinterpreter ist heute ein Entwicklungswerkzeug. In v3 ist er ein Sicherheitsmechanismus.

Auf einem Zweikernsystem (Lockstep oder asymmetrisch) läuft auf dem zweiten Kern der Interpreter über derselben MIR, mit denselben Inputs, und vergleicht je Tick einen Zustands-Hash. Weil die Semantik bitidentisch ist, muss der Vergleich exakt aufgehen.

Das liefert:

- **Diversitäre Redundanz auf Werkzeugebene.** Ein Codegen-Fehler wird zur Laufzeit entdeckt, nicht nur im Test. Für die TD-Einstufung (Tool Error Detection) ist das das stärkstmögliche Argument: TD1 wird verteidigbar, und damit fällt TCL von 3 auf 1.
- Ein Argument, das mit C-Toolchains strukturell nicht geht, weil es dort keine zweite unabhängige Ausführung derselben Semantik gibt.

Die Kosten sind ehrlich zu nennen: der Interpreter ist um eine Größenordnung langsamer. Für T₀ ≥ 1 ms und moderate Programme geht es; für 50 µs nicht. Aber selbst eine *stichprobenartige* Schattenausführung (jeder n-te Tick, oder nur die sicherheitsrelevanten Maschinen) trägt das Argument.

**Stufe:** v2 als Option, v3 als Teil des Qualifikationskits.

---

# D. Was Teams brauchen, sobald mehr als eine Person schreibt

## D1. Deklarierte Budgets je Maschine

Heute ist das Budget eine globale Prüfung am Ende (7.2, 11.5). In einem Projekt mit mehreren Teams heißt das: Die Überschreitung fällt bei der Integration auf, wenn sie teuer ist.

```
machine current_ctrl every 50 us with budget = {ram = 2 KiB, wcet = 20 us}:
```

Compile-Fehler bei Überschreitung, lokal, sofort. Damit wird das Budget zum **Vertrag zwischen Teams** statt zum Integrationsrisiko. Dieselbe Logik gilt für Bibliothekskomponenten: ein Block, der sein Budget deklariert, ist wiederverwendbar ohne Überraschung.

Ergänzend: `takt size --baseline vorher.json` mit Delta-Ausgabe als CI-Gate. Budget-Regression wird damit so sichtbar wie ein fehlgeschlagener Test.

**Stufe:** v1.1. Sehr billig, hohe Wirkung.

---

# E. Korrekturen mit Hebel, ohne neue Konzepte

Diese kosten fast nichts und räumen Reibung weg, die ich beim Schreiben real gespürt habe.

**E1. Konstantenvariablen in Generics und `reader`/`writer` nach v1 ziehen.** Die Referenz sagt selbst, die Grammatik parst sie ohnehin ab M0, und 3.9 gibt zu, dass bis v1.1 der laufende Cursor fehlt. Protokollarbeit ist v1-Kerngeschäft. Das ist eine reine Sequenzierungsentscheidung.

**E2. `_` als Wegwerf-Ziel** (`_ = buf.push(x)`) und **statisch bewiesenes `push`**: Wenn die Intervallanalyse `len + n <= N` zeigt, darf `push` als bloßes Statement stehen. Beides entfernt die Wüste aus toten Bindungen in jedem Serialisierer.

**E3. Backpatching braucht ein Idiom.** 3.9 motiviert schreibenden Index-Zugriff mit Längenpräfix, CRC und COBS, zeigt aber kein Beispiel. Ein `w.mark() -> Mark` plus `w.patch_u16(m, v)` macht es sicher — die Position ist im `Mark` gebunden, die Schranke beim Erzeugen bewiesen.

**E4. Record-Arrays als Konstanten klarstellen.** `const SENSORS : [2] SensorDef = [SensorDef(...), SensorDef(...)]` — statische Gerätetabellen sind *der* Datenblock jedes Treibers. Falls erlaubt, fehlt ein Beispiel in 3.7. Falls nicht, ist es die wichtigste kleine Ergänzung der ganzen Liste.

**E5. Einheiten-Querprüfung gegen die Hardware-Konfiguration (Prüfung 59).** Die Konfiguration trägt Einheit, Skalierung und Range je Channel (8.10). Ein Programm, das `@ hw("daq1/ai0")` mit `float[bar]` bindet, während die Konfiguration `psi` sagt, ist heute unentdeckt. Das ist ausgerechnet die Stelle, an der Einheitenfehler real Schaden anrichten.

**E6. `follows`/Abort-Phase-Lint.** Ein Follower liest in der Abort-Phase Ψ_k statt frisch (7.2). Semantisch sauber, aber ein stiller Bedeutungswechsel. Prüfung 33 sollte warnen, wenn eine gefolgte Größe auch im Fault-Pfad des Followers gelesen wird.

**E7. `alert`-Polaritäts-Lint.** Die entgegengesetzte Polarität zu `check` ist begründet (5.6), wird aber Fehler produzieren. Ein Lint für `alert x < LIMIT`-Muster mit Vorschlag kostet nichts.

**E8. Profil-Validierung schärfen.** Fehlender Param im Profil → Default plus Alert. Unbekannter Param im Profil → Fehler (Tippschutz). Heute nicht geregelt.

---

# Zuordnung zu den Stufen

| Stufe | Ergänzung | Wirkung |
|---|---|---|
| **v1** | Safe-State-Latenz berechnen + `within`; Fault-Erreichbarkeitsmatrix | Die Kennzahl, die im Safety Case zählt |
| | const-Generics, `reader`/`writer`, `_`, bewiesenes `push`, `mark/patch`, Record-Array-Konstanten | Protokollarbeit wird idiomatisch statt mühsam |
| | Prüfung 59 (Einheiten gegen Hardware-Konfiguration) | Schließt die gefährlichste Einheitenlücke |
| | Lints E6, E7, E8 | Billig, verhindert reale Fehlerklassen |
| **v1.1** | `assumption` mit Monitoring; Kanal-Attribute als automatische Annahmen | Macht `takt prove` überhaupt benutzbar |
| | Block-Verträge `requires`/`ensures` | Kompositionaler Beweis; Bibliothek wird Argument |
| | Check-Klassifikation aus dem Beweis | Zertifizierungsartefakt + Budget-Gewinn |
| | Maschinen-Replay; Satz 9.11 (Kompositionalität) | Feldfehler → Unit-Test; Beweise skalieren |
| | Trace-Hash-Kette, signierter Header, `verify-trace` | Determinismus wird Beweismittel |
| | Deklarierte Budgets je Maschine; `size --baseline` | Budget wird Teamvertrag statt Integrationsrisiko |
| | TCB-Manifest + `tcb_policy` | Die Grenze wird prüfbar statt Prosa |
| **v1.2** | `takt certify` mit generierter Trace-Matrix und Safe-State-Analyse | Evidenz generieren statt pflegen |
| | Budget-Ausklammerung bewiesener Checks | Performance-Einwand entkräftet |
| **v2** | Safe-State-Latenz über Knoten (`+ hops · T₀`); Schatten-Interpreter als Option | Verteilte FTTI-Garantie — sonst nirgends |
| **v3** | Schatten-Interpreter im Qualifikationskit; TD1-Argument formal | TCL 3 → 1, damit Werkzeugqualifikation bezahlbar |

---

# Was ich zuerst machen würde

Wenn ich eine Sache auswählen müsste: **die Safe-State-Latenz als berechnete, deklarierbare Größe.**

Nicht weil sie technisch die schwierigste ist — sie ist die einfachste der Liste, ein Graph-Durchlauf über bereits vorhandene Daten. Sondern weil sie die Positionierung ändert. Eine Sprache, die „sicher" verspricht, konkurriert mit einem Dutzend anderer. Ein Compiler, der einem Prozessingenieur die Worst-Case-Reaktionszeit seiner Sicherheitsfunktion als bewiesene Zahl in Nanosekunden ausgibt, aufgeschlüsselt nach Erkennung, Bestätigung, Propagation und Commit, tut etwas, das kein anderes Werkzeug tut.

Und sie hat die richtige Eigenschaft für einen frühen Meilenstein: Sie ist ohne `prove`, ohne Qualifikation, ohne fertige Runtime demonstrierbar. Ein Prototyp, der ein Beispielprogramm einliest und diese Tabelle ausgibt, ist in Wochen machbar und zeigt die Edge sofort.

Die zweitwichtigste wäre `assumption` mit Monitoring — weil ohne sie der gesamte Beweisteil in Abschnitt 13.3 in der Praxis nicht zündet, egal wie gut die Semantik dahinter ist.