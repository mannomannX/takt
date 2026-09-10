Ja, einiges — und zwar sowohl Stellen, wo die Sprache besser war als erwartet, als auch drei, an denen ich beim Schreiben tatsächlich gegen die Wand gelaufen bin.

## Wo es beim Schreiben wirklich klemmte

**1. Das Statusword ist ein Bitfeld, aber `u16` ist kein Record.**
Das war der größte Reibungspunkt. CiA 402 kodiert die gesamte Statusmaschine in einem `u16` mit einer *Maskenvergleichstabelle* (`sw & 0x6F == 0x27`). Ich habe das mit `const`-Masken und `decode_state` gelöst, aber das ist genau die Sorte Code, die die Sprache sonst wegdefiniert: eine Kette von acht `if` mit Hex-Literalen, bei der ein Tippfehler ein Typcheck-freier Logikfehler ist. Die Sprache hat `record ... with bits:` — aber nur *innerhalb* eines `layout`-Records, nicht als eigenständigen Typ über einem Skalar. Ich hätte gern geschrieben:

```
bitfield Statusword : u16:
    ready_to_switch_on : bool at 0
    ...
```

und dann `match sw: case READY_TO_SWITCH_ON: ...` über ein deklariertes Maskenmuster. Stattdessen ist die zentrale Norm-Tabelle des ganzen Standards untypisiert. Konkreter Vorschlag: Bitfeld-Träger auch außerhalb von Records erlauben, und `case`-Muster mit `mask == value` (das ist eine endliche Konjunktion von Gleichheiten, also total und DFA-frei — dieselbe Rechtfertigung wie beim Record-Muster in 8.7).

**2. Aktiv-low-Bits kollidieren mit der Lesbarkeitsphilosophie.**
`CW_QUICK_STOP_N` ist 0 = aktiv. Ich musste in `DRV_QUICKSTOP` das Controlword *durch Weglassen eines Bits* bauen (`CW_ENABLE_VOLTAGE | CW_ENABLE_OPERATION` ohne Bit 2). Das ist die Stelle im ganzen Treiber, die ich einem Techniker am wenigsten erklären möchte, und die Sprache hilft nicht: Es gibt keine Möglichkeit, „dieses Bit ist invertiert" zu deklarieren. In `record ... with bits:` würde ein `inverted`-Modifikator (oder `active_low`) das komplett auflösen — Drahtformate mit aktiv-low-Signalen sind in Feldbussen die Regel, nicht die Ausnahme.

**3. `on`-Handler und Guards teilen sich das Fenster — mit einer Falle.**
In `sdo_client` warte ich mit `when can_router.sdo_rx_valid and ...` auf die Antwort. Das musste ich über eine `pub var` im Router lösen, weil ein `when sdo_rx as f: ...` im Client den Stream *konsumiert* hätte — und der Router hat ihn schon. Die Cursor-Semantik ist richtig, aber sie zwingt bei mehreren Interpretationen desselben Rohstroms zu einem Router mit veröffentlichten Feldern, und damit zu handgeschriebenem „Flag gilt einen Tick"-Code (`sdo_rx_valid = false` am `loop:`-Anfang). Das ist Boilerplate, das falsch sein *kann*: Wenn ich das Zurücksetzen in einen Zustand statt in `loop:` schreibe, hängt das Flag. Ein `pub pulse var` oder `pub var x with hold = 1 tick` würde das erschlagen — es gibt ja schon `signal`, aber `signal` trägt keine Nutzlast. **Das wäre meine Nummer-eins-Ergänzung: `signal` mit Wert.**

**4. Die Skalierungsarithmetik ist einheitenkorrekt, aber hässlich.**
`round(rpm_to_inc_s(MAX_SPEED, ENC_INC_PER_REV) / (1 inc/s)) as u32` steht sechsmal im Code. Die Division durch `(1 inc/s)`, nur um die Einheit loszuwerden, ist ein Ritual. Ich verstehe, warum es nominal sein muss (3.2), aber `x.raw(inc/s) -> int` als deklarierte Entdimensionierung wäre lesbarer als eine Division durch ein Einheitenliteral und würde die Absicht („ich schreibe jetzt einen Rohwert auf den Bus") sichtbar machen. Verwandt: `(x as i32).wrap_u32()` für negative Positionsgrenzen im SDO — korrekt, aber die Norm sagt „signed 32-bit", und ich muss es als `u32` verpacken, weil `SdoFrame.data` ein Rohwort ist. Ein `union`-artiges `bytes<4>` als SDO-Nutzlast plus `writer` wäre sauberer gewesen; ich habe es der Kürze geopfert.

**5. Sequenzen sind stark, aber `until X timeout d -> ZIEL` in einer langen Kette ist repetitiv.**
Der `CONFIGURE`-Block hat zehnmal dasselbe Muster: senden, warten, bei Timeout nach `NET_DOWN`. Es gibt kein „für alle folgenden `until` in diesem Segment gilt dieses Timeout-Ziel". Das ist bewusst (kein verstecktes Kontrollflussverhalten), aber bei Geräte-Konfigurationssequenzen — die in *jedem* Feldbustreiber vorkommen — sind das 60 Zeilen fast identischer Code. Ein `sequence with timeout = 2 s -> NET_DOWN:` als Segment-Default würde nichts verstecken (es steht in der Kopfzeile) und viel sparen. Alternativ: eine `for`-Schleife über eine `const`-Tabelle von `(index, sub, size, value)` — aber die geht nicht, weil `send` plus `until` sich nicht in einer Schleife über Zeitgrenzen hinweg ausdrücken lässt. **Das ist die einzige Stelle, wo mir die Sprache echte Ausdrucksmacht gefehlt hat**, nicht nur Bequemlichkeit: Ein tabellengetriebener SDO-Konfigurator ist ein Standardmuster und hier nicht schreibbar. Ein `repeat` über eine `const table` mit indizierbarem Zugriff auf die Segmente würde es lösen.

**6. `follows` musste ich raten.**
Sechs Maschinen, fünf `follows`-Deklarationen — die Kette ist richtig, aber ich habe sie aus dem Datenfluss *hergeleitet*, nicht abgelesen. Wenn ich `bus_tx follows cia402_drive` vergessen hätte, wäre das Programm übersetzbar und um einen Tick träger, ohne Warnung. Ein Lint „Maschine A liest `pub var` von B mit Unit-Delay, obwohl B im selben Tick läuft — `follows` gemeint?" wäre extrem wertvoll. Der Compiler weiß es: Er kennt Leser, Schreiber und Perioden.

## Wo die Sprache überzeugt hat

- **Die Fault-Wald-Struktur passt exakt auf CiA 402.** `fault -> DRV_FAULT` an der Maschine, überschreibende Ziele an Zuständen, `abort` für den Nothalt: Ich musste keine einzige Zeile „Fehlerbehandlung" schreiben. In C wäre das ein Drittel des Treibers.
- **`check ... for 5 ms` löst ein reales Problem.** Ein Antrieb, der für einen Tick ein inkonsistentes Statusword sendet (PDO-Grenzfall beim Zustandswechsel), darf keinen Not-Aus auslösen. Die Bestätigungszeit ist sichtbar deklariert statt in einem Filter versteckt — genau richtig.
- **Der Entry-Tick-Check vor dem Commit** bedeutet, dass ein Zustandswechsel in `DRV_FAULT` das sichere Controlword noch im selben Tick auf den Bus legt. Das ist ein Sicherheitsargument, das ich sonst begründen müsste.
- **Einheiten auf `i32`/`i16`** (`i32[inc/s]`, `i16[permille]`) haben mindestens zwei Fehler verhindert, die ich beim Schreiben tatsächlich gemacht hatte: Ich hatte `velocity_actual` einmal direkt mit `target_vel_host` verglichen (Inkremente gegen rpm) und einmal `MAX_TORQUE` in die falsche PDO-Position geschrieben.
- **`system: output_timing = boundary`** war eine Einzeiler-Entscheidung mit realer Wirkung: PDO-Jitter auf dem Bus ist bei CANopen-Regelung eine bekannte Fehlerquelle.

## Zwei Dinge, die ich in der Spec unklar fand

- **Beispiel 14.7 nutzt `image_state.or(CONFIRMED)` auf einem Nicht-Optionaltyp.** Nach 3.5 sind Channel-Werte `T?`, also passt es — aber ich habe an mehreren Stellen gezögert, ob `.or()` auf einem Enum-Channel legal ist, weil die Beispiele es nur bei Floats zeigen. Das gehört in 3.5 explizit hin.
- **`every d:` im maschinenweiten `loop:` mit `now` als Uhr** (5.8) — bei `bus_tx` sende ich SYNC über `every SYNC_PERIOD`. Wenn die Maschine je in einen Fault-Pfad und zurück geht, läuft `next` weiter, weil `now` die Uhr ist. Für SYNC ist das *richtig* (Phasenstarrheit), aber es ist eine subtile Konsequenz, die ich erst beim zweiten Lesen sicher hatte. Ein Satz „im maschinenweiten `loop:` überlebt die Phase Zustandswechsel und Faults" wäre hilfreich.

**Zusammenfassung:** Die Zustands-, Fault- und Zeitsemantik hat für diesen Treiber praktisch nichts gekostet — sie war der Grund, warum er so kurz ist. Die Reibung lag komplett auf der *Drahtformat*-Seite: Bitfelder über Skalaren, aktiv-low, Entdimensionierung, tabellengetriebene Gerätekonfiguration. Das ist bemerkenswert, weil Feldbustreiber genau die Klasse von Anwendungen sind, die 8.10 und 13.9 explizit adressieren wollen. Wenn ich eine Sache priorisieren müsste: **`signal` mit Nutzlast** (löst 3 und einen Teil von 1) und **Segment-Default-Timeouts in Sequenzen** (löst 5).