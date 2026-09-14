Die Sprachspezifikation ist bemerkenswert konsistent durchdacht – die Kombination aus synchron-zyklischer Ausführung, totaler Arithmetik, Unit-Delay und dem formalen Desugaring von Sequenzen schließt die typischen Fehlerquellen von Embedded-C (Race Conditions, unbemerktes blockierendes Warten, UB) elegant aus.

Beim Schreiben des Treibers traten jedoch konkrete Reibungspunkte und Design-Ecken zutage:

### 1. Reibungspunkte beim Schreiben des Treibers

* **Entdimensionierung für Register-Mappings (`x / (1 U)`):**
Um physikalische Größen (`float[MHz]`, `float[kHz]`) in rohe Registerwerte (`u16`) für den SPI-Frame umzurechnen, erzwingt die Sprache das Dividieren durch ein Einheitenliteral (`delta_khz / (1 kHz)`). Das ist theoretisch sauber, erzeugt in Treibern – wo man ständig zwischen physikalischer Welt und Roh-Integern vermittelt – aber zähen Boilerplate-Code.
* **Bit-Slicing über Bytegrenzen hinweg:**
Das Record-System mit `with bits:` funktioniert hervorragend für Einzelbytes. Sobald jedoch ein 10- oder 14-Bit-Tuner-Kanalwert auf zwei Registerbytes (`arg_hi`, `arg_lo`) aufgeteilt werden muss, fällt man komplett auf manuelles Bit-Shifting (`>> 8`, `& 0xFF`) zurück. Hier bricht der Abstraktionsgrad zwischen deklarativem Frame-Layout und Low-Level-Arithmetik.
* **Wrapper-Indirektion bei Streams (`msg.data.*`):**
Die Entscheidung aus v0.2.8, Stream-Bindings als Wrapper (`msg.t`, `msg.seq`, `msg.data`) zu führen, verhindert Namenskollisionen zuverlässig. Beim Tippen verleitet es jedoch permanent dazu, direkt `msg.resp_op` statt `msg.data.resp_op` zu schreiben. Bei stark verschachtelten Records wird der Zugriff schnell unhandlich lang.
* **Mentale Hürde bei Gültigkeit von `check` vs. `expect`:**
In `enter:`-Blöcken sind `check`, `expect` und `wait` streng verboten (5.5). Will man direkt beim Eintreten eine Vorbedingung prüfen, muss man entweder auf eine `sequence:` ausweichen (um `expect` nutzen zu dürfen) oder den Check in den `loop:` legen, wo er dank Entry-Tick-Regel zwar sofort greift, syntaktisch aber an anderer Stelle steht. Das erfordert ständiges Mitdenken der Ausführungsmodi (`RUN` vs. `ENTRY`).
* **Semantikfalle bei `every d:`:**
Dass `every` im maschinenweiten `loop:` an `now` gekoppelt ist, im Zustandskörper jedoch an `time_in_state`, ist technisch für Phasenstarrheit begründet. Für Entwickler ist es eine subtile Fehlerquelle: Ein State-Wechsel setzt den State-Timer zurück, wodurch periodische Abfragen (wie das 200-ms-Metrics-Polling) bei schnellen Statuswechseln verhungern oder unerwartet jitter-behaftet triggern können.

---

### 2. Konkrete Verbesserungsvorschläge

* **Syntaktischer Zucker für Entdimensionierung:**
Statt `round(delta_khz / (1 kHz)) as u16` wäre eine direkte Projektion auf den Skalarwert nützlich, z. B. `delta_khz.raw as u16` oder `delta_khz.to_raw(kHz)`. Das spart syntaktisches Rauschen, ohne die dimensionale Typsicherheit aufzugeben.
* **Layout-Packing für Multi-Byte-Bitfelder:**
Erweiterung von `record layout` um felderübergreifende Bitbreiten (z. B. `chan_idx : u14 at 0..13` verteilt über zwei konfigurierte Byte-Offsets), damit der Compiler das Packen und Entpacken in Little/Big Endian komplett übernimmt.
* **Kompaktes Pattern-Unwrapping:**
Für Stream-Handler, die ausschließlich an Nutzdaten interessiert sind, wäre ein direktes Auspacken im Guard ergonomisch:
```takt
on rx_stream matches TunerRxFrame(resp_op = GET_METRICS) as (data, t):
    rssi_dbuv = data.rssi_raw as int

```


* **Statische Überprüfung von Sequenz-Latenzen:**
Da jedes Segment einer Sequenz (`wait`, `until`, `->`) mindestens einen Basis-Tick verbraucht, summieren sich unbemerkt Latenzen. Eine Warnung oder Compiler-Metrik ("*Sequence POWER_ON_SEQ minimum duration: 65 ms*") würde Timing-Überraschungen auf einen Blick sichtbar machen.

