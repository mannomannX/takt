## Was gut trug

**Die Fehlerdomänen-Trennung über Maschinen mit eigenen Fault-Zielen** war der stärkste Moment. Die Frage „darf ein Parserfehler das Licht ausschalten?" beantwortet sich in dieser Sprache durch die Struktur selbst, nicht durch Disziplin. In C wäre das eine Konvention, die beim dritten Refactoring stirbt.

**`T!E` an der Protokollgrenze.** Die Unterscheidung „kaputtes Paket = Normalbetrieb, Invariantenverletzung = Fault" ist genau die Achse, an der Protokolltreiber sonst scheitern. Dass die Sprache beide Mechanismen hat und sie *verschieden aussehen*, ist wertvoller als jede Doku.

**Cursor je Konsument (8.6).** Das hat ein echtes Designproblem aufgelöst. Ich hatte einen Verteiler mit `pub var` eingeplant und ihn gestrichen, als mir klar wurde, dass jede Maschine den Strom direkt lesen kann. Ein Tick Latenz gespart, eine Single-Writer-Fessel weniger.

**Einheiten auf Records.** `u32[B]` für Byte-Offsets war im Bootloader-Beispiel überzeugend; hier hätte ich es bei Slot-Nummern gebraucht und es nicht durchgezogen (siehe unten).

---

## Wo ich hängen blieb

### 1. Reine Funktionen können nicht in Puffer schreiben — `push` ist Statement, aber ich brauchte es in `fn`

Das war der aufwändigste Reibungspunkt. Jede `build_*`- und `pd_*`-Funktion sieht so aus:

```
var a = pd.push((reason >> 8) as u8)
var b = pd.push((reason & 0x00FF) as u8)
```

`push` ist Statement mit Ergebnis-Stelle (4.4) — richtig, weil sonst die Auswertungsreihenfolge sichtbar würde. Aber das erzeugt in Serialisierungscode eine Wüste aus toten Bindungen `a`, `b`, `ok`, `ok2`, `ok3`. Ich habe sie durchnummeriert, weil die Sprache keinen Verwerfungs-Namen hat. `_` ist als Padding-Feld in `layout` belegt (3.7), nicht als Wegwerf-Binding.

Bei einem Puffer, dessen Kapazität statisch über dem Bedarf liegt, ist `push` nie `false`. Das weiß die Intervallanalyse im Prinzip (`pd.len` ist beschränkt, die Pushzahl ist statisch), nutzt es aber nicht. Zwei Vorschläge, beide klein:

- **`_ = pd.push(x)`** als erlaubtes Wegwerf-Ziel. Rein syntaktisch, keine Semantikänderung.
- **Statisches Wegbeweisen:** Wenn der Compiler zeigt, dass `len + n <= N` gilt, ist das Ergebnis `true` und die Bindung entfällt. Das ist genau die vorhandene Intervallanalyse auf `.len` angewandt — `pd.push(x)` als Statement ohne Ziel wäre dann erlaubt, wenn bewiesen.

Der zweite ist der bessere: er macht aus dem Rauschen eine Aussage.

### 2. `writer` gibt es erst in v1.1 — und v1 hat keinen Ersatz für Frames variabler Struktur

3.9 sagt selbst: *„Bis v1.1 übernimmt `layout` dieselbe Aufgabe … was fehlt, ist der laufende Cursor über einer Folge ungleicher Felder."* Genau das war mein Fall. Ein RDM-Frame ist fester Kopf (24 Byte) + variable Nutzlast (0–231) + Prüfsumme, wobei die Prüfsumme über *alles davor* läuft. Mit `layout` allein geht das nicht, weil die Prüfsummenposition von der PDL abhängt.

Meine Lösung — Kopf per `encode()`, Rest per `push`-Schleife, Prüfsumme per Funktion über den entstandenen Puffer — funktioniert, ist aber dreimal so lang wie mit `writer` und hat drei Stellen, an denen eine falsche Konstante (24, 231, 264) stillschweigend das Falsche tut.

**Beobachtung zur Stufenplanung:** `writer` ist in v1.1 einsortiert, weil es Konstantenvariablen in Generics braucht (3.12). Aber Protokolltreiber sind *das* Kerngeschäft dieser Sprache, und sie sind ohne Cursor in v1 unangenehm. Ich würde `writer`/`reader` mit fester Kapazität (ohne `const N`, also `writer_256`, `writer_64`) als v1-Baustein erwägen — hässlicher Name, aber verfügbar, wenn man ihn braucht.

### 3. Die Prüfsumme über einen Puffer, den ich gerade baue

```
var csum = rdm_checksum(out, msg_len)
var ok3 = out.push((csum >> 8) as u8)
```

Das liest `out` in einem Ausdruck und schreibt gleich danach hinein. Erlaubt (kein Aliasing, `out` ist eine benannte Stelle), aber mir war beim Schreiben zwei Minuten unklar, ob die Sprache das mag — weil `inout` (3.9) explizit Aliasing-frei sein muss und ich die Regel zuerst auf diesen Fall übertrug. Sie gilt dort nicht, aber die Nähe verwirrt.

Was hier wirklich fehlt, ist das **Backpatching-Muster als Idiom**. 3.9 nennt es ausdrücklich als Motivation für schreibenden Index-Zugriff („Längenpräfix, CRC am Ende, COBS-Rahmung"), aber es gibt kein Beispiel dafür in der Referenz. Ich habe es umgangen, indem ich `message_length` vorher ausrechne. Bei COBS ginge das nicht.

### 4. `fn` mit Array-Konstanten: ich weiß nicht, ob `const` im Funktionskörper erlaubt ist

```
fn pd_supported_params() -> bytes<231>:
    const LIST : [6] u16 = [...]
```

Die Grammatik hat `const_decl` nur auf Dateiebene (`file :=`). Im Block gibt es nur `var_decl`. Also ist das oben vermutlich ein Syntaxfehler, und ich müsste `LIST` nach oben ziehen. Das ist konsistent, aber ich habe es beim Schreiben nicht bemerkt — und ich vermute, das passiert jedem, der aus C oder Rust kommt.

Falls es Absicht ist (eine Datei-Konstante ist sichtbarer als eine versteckte), gehört ein Satz in 2.4 dazu. Falls nicht: `const` im Block ist harmlos, weil er per Definition compile-zeit-konstant ist.

### 5. Ternäre Ketten in Konstruktoren werden unleserlich

```
sensor_type      = 0x00 if n == 0 else 0x01,
sensor_unit      = 0x01 if n == 0 else 0x05,
range_min        = -400 if n == 0 else 0,
range_max        = 1250 if n == 0 else 300,
```

Das ist eine Tabelle, als Ausdruckskette geschrieben. `match` ist ein Statement (2.3, `match_stmt`), also kann ich nicht `sensor_type = match n: ...` schreiben. Die saubere Form wäre ein `const`-Array von `SensorDef`-Records und ein Index — was daran scheitert, dass ich den Record im `const` mit Literalen füllen müsste und dabei wieder bei derselben Länge lande.

**Das ist der Punkt, an dem ich am meisten Zeit verloren habe.** Statische Gerätebeschreibungen (Sensordefinitionen, PID-Tabellen, Personality-Tabellen) sind in jedem Protokolltreiber *der* Datenblock. Die Sprache hat `table<A,B>` für Stützstellen und `const`-Arrays für Skalare, aber kein bequemes Konstrukt für „Array von Records mit Literalen".

Falls `const SENSORS : [2] SensorDef = [SensorDef(...), SensorDef(...)]` erlaubt ist, löst das den Fall vollständig — dann fehlt nur das Beispiel. Ich konnte aus 3.7 und 11.3 nicht sicher ableiten, ob ein Record-Konstruktor ein `const_expr` ist.

### 6. Ich habe die Slot-Einheit deklariert und nicht benutzt

```
unit slot   = 1
unit lvl    = 1
```

Beide stehen ungenutzt im Code. Das ist mein Fehler, aber ein instruktiver: Ich habe sie deklariert, weil die Referenz das Argument „Verwechslung von Sektornummer und Byteadresse ist ein Typfehler" (3.2) überzeugend macht. Dann kam `slots : bytes<512>`, und ein `bytes`-Index ist `int in 0..N-1` — ich kann ihn nicht als `int[slot]` typisieren, ohne den Indexzugriff zu verlieren.

Das ist eine echte Lücke: **Einheiten auf Indizes gibt es nicht.** Ein Array-Index ist strukturell `int in 0..N-1`, und `int[slot] in 0..511` ist als Index nicht erlaubt (3.1: „Index vom Typ `int in 0..N-1`"). Für einen Treiber mit drei verschiedenen Nummernräumen (DMX-Slot 1-basiert, Puffer-Offset 0-basiert, PWM-Kanal 0-basiert) wäre das genau die Stelle, an der Einheiten Fehler fingen — und sie greifen dort nicht.

---

## Eine Stelle, an der ich der Sprache widersprechen würde

**`follows` verleitet zu Ketten, die die Abort-Phase nicht hat.**

Ich habe `rdm_dispatch follows rdm_responder follows dmx_core` gebaut, weil jeder Tick Latenz im RDM-Antwortfenster zählt. 7.2 sagt: *„Frische Lesevorgänge gelten nur in der Schrittphase; in der Abort-Phase gilt Ψ_k."*

Das ist semantisch sauber, aber es bedeutet: **Meine Kette verhält sich im Fault-Fall anders als im Normalfall.** `rdm_dispatch` sieht in der Abort-Phase ein `tx_len` von vor einem Tick. Für meinen Fall harmlos (im Fault geht `tx_enable` sowieso auf `safe`), aber ich musste es prüfen — und der Compiler hilft nicht dabei, weil Prüfung 33 nur *frische Lesevorgänge in Aktionsblöcken der Abort-Phase* verbietet, nicht die stille Bedeutungsänderung.

Vorschlag: Ein Lint, der meldet, wenn ein Follower eine gefolgte Größe liest, die auch in seinem Fault-Pfad vorkommt. Das ist genau die Stelle, an der die Semantik wechselt.

---

## Zusammenfassend

Die Sprache hat mich an keiner Stelle gezwungen, gegen sie zu arbeiten — das ist bemerkenswert für einen Protokolltreiber, die klassische Domäne für „ich brauche hier mal einen Zeiger". Die Reibung lag durchgängig in der **Serialisierungsschicht**: Puffer füllen, Prüfsummen über Teilstücke, statische Gerätetabellen. Das ist kein Zufall — es ist die eine Stelle, an der ein Treiber Bytes als Bytes behandelt statt als typisierte Werte, und dort hilft ein starkes Typsystem naturgemäß am wenigsten.

`writer`/`reader` in v1.1 adressiert zwei Drittel davon. Das letzte Drittel — Record-Arrays als Konstanten — ist vermutlich schon erlaubt und braucht nur ein Beispiel in 3.7.