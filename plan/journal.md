# Das `persist`-Journal

> *„Die Runtime schreibt geänderte Werte asynchron, atomar (Journal) und
> höchstens alle `min_interval`; das Schreiben ist Beobachtung und liegt
> außerhalb der Semantik."* — 5.9

Sprache und Laden stehen (M5, 2026-09-15): `persist var` wird gesenkt,
SC-23 prüft, der Interpreter bildet s0 aus geladenen, validierten Werten
und meldet `PersistReset`. Was fehlt, ist die andere Richtung — wie ein
Wert in den nichtflüchtigen Speicher kommt und einen Stromausfall
übersteht.

Dieses Dokument plant das. Es schließt `PAR-5.9-Journal_Anforderungen`,
vervollständigt `SEM-5.9` und `SEM-9.10` und bringt `PROFIL-baremetal`
einen Schritt weiter (Referenz Zeile 1767 nennt das NVM-Journal als Teil
des Profilaufsatzes).

---

## 0. Was die Referenz verlangt

Vier Stellen, zusammengezogen:

**5.9 — die Journal-Anforderungen selbst.** Zwei Slots im Wechsel
(ping-pong); jeder Eintrag trägt Sequenznummer und CRC32; ein
Schreibvorgang ändert genau einen Slot; beim Start gewinnt der gültige
Eintrag mit der höheren Sequenznummer; Schreiben nur mit
Sektorgranularität und `min_interval`. Vor `reboot`, `boot_jump` und Deep
Sleep schreibt die Runtime ausstehende Änderungen synchron. Der Absatz
nennt sie ausdrücklich „Teil der Treiberkonformität 13.8, in der
Simulation gegen das Flash-Modell mit Stromausfall-Injektion zu prüfen
(8.11)".

**12.1 — wo es im Tick sitzt.** `record_and_telemeter()  # außerhalb der
Semantik, nie blockierend; persist-Journal`. Dieselbe Zeile wie die
Telemetrie, und dieselbe Bedingung.

**12.3 — was es auf der MCU ist.** „`persist`: journalisierte
NVM-Schreibvorgänge (Flash-Sektorwechsel, CRC, Typ-Hash), Rate durch
`min_interval` begrenzt."

**12.3 XIP-Absatz — die Verschärfung.** „Das NVM-Journal schreibt eine
niedrig priorisierte Aufgabe, während der Tick aus dem RAM weiterläuft —
sonst erzeugt jedes Speichern einen `Runtime(Overrun)`." Eine
Sektorlöschung kostet zweistellige Millisekunden; bei 10 ms Tick sind das
mehrere verpasste Ticks.

---

## 1. Der Befund, der alles andere bestimmt

**Der erzeugte Code gibt keine Variable heraus.**

Pro Maschine entstehen fünf Funktionen — `_step`, `_init`, `_idle`,
`_advance`, `_deadline` (`takt-llvm/src/lower.rs:103-109`). Keine liest
einen Wert aus dem Zustand. Die Schrittsignatur ist
`void f(ptr state, ptr inputs, ptr psi, ptr latch)`
(`machine.rs:193-196`); der Zustand ist ein benannter LLVM-Typ
`%<name>_state`, dessen Layout nur LLVM kennt. Es gibt keine
`offset_of`-Rechnung für seine Felder — Byte-Offsets existieren im
Projekt nur für Abbild, Latch und Parameter (`takt-llvm/src/image.rs:72,
93, 105`).

Wie sehr das trägt, zeigt der MCU-Rahmen: Seine Telemetrie liest
ausschließlich aus dem **Latch** (`takt-conformance/src/mcu.rs:305, 315`
— `*(ct *)(latch + offset)`), nie aus einem Zustands-Struct. Outputs
haben ein bekanntes Byte-Layout, Maschinenvariablen nicht.

Das Journal muss aber genau an Maschinenvariablen heran. **Also entsteht
der Lesepfad mit dem Journal oder gar nicht.** Jede Option unten wird
zuerst daran gemessen.

Zweiter Befund derselben Art: **Es gibt keine Änderungserkennung.** Weder
`MachineState` noch der erzeugte Code kennen ein Dirty-Flag (projektweit
null Treffer auf `dirty`). Die einzige Änderungserkennung im Projekt
liegt im Trace-Writer und vergleicht *formatierte Strings*
(`takt-interp/src/run.rs:469-489`) — brauchbar als Vorbild für die
Struktur, nicht als Mechanismus.

---

## 2. Abwägungen

### 2.1 Die Byte-Form: das einzige Unwiderrufliche

Alles andere in diesem Plan lässt sich später ändern. Die Byte-Form
nicht: Sobald ein Gerät im Feld einen Eintrag geschrieben hat, ist jede
Änderung ein Verlust seiner Daten. Sie gehört deshalb zuerst entschieden
und am gründlichsten.

**Was sie können muss.** Beispiel 14.7 persistiert `SelftestResult` —
einen Record **ohne `layout`**, mit einem `float[mohm]`-Feld
(`definition.md:2485-2488`). Dazu `int in 0..100000` und `[2] int`
(14.6). Die POD-Menge aus 5.9 ist: Skalare, Records, Arrays, Enums, dazu
`bytes`, `str`, `vec` und `map` (3.7/3.9).

**Warum `wire::encode` ausscheidet.** Es ist der einzige vorhandene
Byte-Pfad für Werte (`takt-interp/src/wire.rs:50`), aber er verlangt
`layout`: `let endian = def.layout.as_ref()?; let size = def.wire_size?;`
(`:57`) und je Feld `let at = f.offset?;` (`:27`). Ohne `layout` liefert
er `None`. 3.7 sagt das auch direkt: „Ohne `layout` gibt es keine
Byte-Repräsentation." Er deckt außerdem `Str`, `Vec`, `Map` und
`Duration` nicht ab.

Das ist kein Mangel von `wire.rs`. `layout` beschreibt **externe**
Formate — Protokollframes, Partitionstabellen, C-Structs (`:639`). Das
Journal ist ein **internes** Format: Nur die Runtime schreibt und liest
es, kein fremdes Gerät muss es interpretieren. Die beiden Aufgaben haben
verschiedene Anforderungen, und `layout` für das Journal zu verlangen
hieße, jedem Nutzer von `persist` eine Deklaration abzuverlangen, die
sein Problem nicht löst — Beispiel 14.7 hat sie nicht.

**Warum ein roher Speicherabzug ausscheidet.** Naheliegend wäre, den
Zustands-Struct-Bereich der persist-Variablen mit `memcpy` zu sichern.
Das scheitert an zwei Dingen: Der Codegen bildet `bool` auf `i1` ab
(`takt-llvm/src/ty.rs:95`), im Speicher ein Byte mit sieben undefinierten
Bits — der CRC32 wäre nicht reproduzierbar. Und das Struct-Layout
bestimmt LLVM, nicht wir; ein Compilerwechsel verschöbe die Felder und
entwertete jeden Eintrag im Feld, ohne dass der Typ-Hash es merkt.

**Entscheidung: eine eigene kanonische Form, `persist::value`.** Sie
steht neben `persist::type_hash` in `takt-mir`, weil sie dieselbe
Eigenschaft braucht — über Firmware-Grenzen stabil — und weil beide
zusammen den Eintrag bilden: Der Typ-Hash sagt, *was* die Bytes
bedeuten, die Wertkodierung liefert sie.

Aufbau, bewusst schlicht:

| Typ | Bytes |
|---|---|
| `bool` | 1 (0 oder 1, nie etwas anderes) |
| `int`-Breiten | 1/2/4/8, little-endian, Zweierkomplement |
| `float` | 4 oder 8, IEEE-754-Bitmuster little-endian |
| `Duration` | 8, Nanosekunden als i64 |
| `enum` | Diskriminante als i64 (8 B), dann die Felder der Variante |
| `record` | Felder in Deklarationsreihenfolge, ohne Padding |
| `[N] T` | N Elemente hintereinander |
| `bytes<N>`, `str<N>`, `vec<T,N>` | Länge als u32, dann die Elemente |
| `map<K,V,N>` | Anzahl als u32, dann Paare in Slot-Reihenfolge |

Drei Festlegungen, die Begründung verdienen:

- **Kein Padding, keine Ausrichtung.** Das Journal wird byteweise
  gelesen und geschrieben, nie als Struct gemappt. Padding kostete Flash
  und brächte nichts.
- **Little-endian überall.** Beide Zielarchitekturen (Cortex-M4F,
  RV32IMAC) sind little-endian. Eine Wahl wäre eine Fehlerquelle ohne
  Nutzen; wer ein big-endian-Ziel bekommt, tauscht beim Lesen.
- **Die Diskriminante immer als 8 Byte, auch bei `layout u8`.** Der
  Typ-Hash trägt `layout` bereits (seit FB-152), also könnte man die
  deklarierte Breite nehmen. Das koppelte die Journal-Form aber an eine
  Deklaration, die für ein *Drahtformat* gedacht ist — ändert jemand
  `layout u8` zu `u16`, weil ein Protokoll es verlangt, ändert sich
  ungewollt auch die Journal-Form. Acht Byte sind teurer und
  entkoppeln sauber.

Die Form ist **nicht selbstbeschreibend** — sie lässt sich nur mit dem
Typ lesen, den der Typ-Hash bestätigt. Das ist beabsichtigt: Ein
selbstbeschreibendes Format (Tags je Wert) kostete Flash und verdoppelte
die Aussage, die der Typ-Hash schon macht.

### 2.2 Wer liest die Werte aus der Maschine?

Drei Wege, den fehlenden Lesepfad zu schaffen.

| Option | Bewertung |
|---|---|
| **A** Codegen exportiert `<maschine>_persist_get(st, i, buf) -> u32` und `_persist_set(st, i, buf, len)` | Zwei Funktionen je Maschine, die genau die persist-Variablen kennen. Der Codegen weiß den Feldindex (`index_of(Role::Var, …)`) und den Typ, kann also die kanonische Form direkt emittieren. Die Runtime sieht nur Bytes und braucht kein Wissen über Struct-Layouts |
| **B** Codegen exportiert Byte-Offsets als Konstanten, Runtime kopiert | Braucht eine `offset_of`-Rechnung, die es nicht gibt, und sie müsste LLVMs Struct-Layout exakt nachbilden — inklusive Alignment-Regeln je Target. Ein Fehler darin ist still und zerstört Daten |
| **C** Die Runtime bekommt einen generischen Zugriff über eine Tabelle | Verschiebt dasselbe Problem: Die Tabelle müsste Offsets enthalten, siehe B |

**Entscheidung: A.** Der Codegen ist die einzige Stelle, die das
Struct-Layout kennt und die Typen der Variablen; beides zusammen braucht
die Kodierung. Die Signatur hält die Runtime byteorientiert und
typfrei — sie muss `Value` nicht kennen, was im `no_std`-Kern ohnehin
nicht ginge.

Der Zuschnitt folgt dem, was schon da ist: `_idle`, `_advance` und
`_deadline` sind genau solche kleinen Zusatzfunktionen je Maschine, die
die Runtime ruft, ohne die Semantik anzufassen.

Nebenwirkung, die für A spricht: Der Lesepfad ist auch das, was
**Telemetrie für `pub var`** auf der MCU brauchen wird (12.3 nennt
„Zustandspfad, Faults, `pub var`"). Wer A baut, baut ihn einmal.

### 2.3 Was löst einen Schreibvorgang aus?

5.9 sagt „geänderte Werte", 12.3 „Rate durch `min_interval` begrenzt".
Beides zusammen heißt: Es braucht eine Änderungserkennung, und sie darf
nicht in jedem Tick über alle Variablen laufen.

| Option | Bewertung |
|---|---|
| **A** Dirty-Flag: Der Codegen setzt es bei jedem Schreibzugriff auf eine persist-Variable | Genau und billig zur Laufzeit (ein `store i1 1`), aber es verteilt Journal-Wissen über den ganzen Codegen — jede Zuweisungsstelle müsste wissen, ob ihr Ziel persistiert ist |
| **B** Vergleich mit dem zuletzt geschriebenen Stand, in der Journal-Aufgabe | Ein Speicherabbild der persist-Variablen in der Runtime; der Vergleich ist ein `memcmp` über die kanonische Form. Kostet den Puffer doppelt, aber die Rate ist ohnehin durch `min_interval` gedeckelt — der Vergleich läuft also selten |
| **C** Gar keine Erkennung: alle `min_interval` schreiben | Verschleißt Flash ohne Not. Ein Sektor hält typisch 10⁴–10⁵ Zyklen; bei `min_interval = 10 s` wäre er nach gut einem Tag durchgeschrieben |

**Entscheidung: B.** Der Ausschlag gibt nicht die Laufzeit, sondern wo
das Wissen liegt: Bei A müsste der Codegen an jeder Zuweisung das
Journal kennen; bei B kennt nur die Journal-Aufgabe das Journal. Das ist
dieselbe Trennung, die 5.9 mit „außerhalb der Semantik" meint.

Der Zusatzspeicher ist klein und statisch bekannt — `takt size` kann ihn
ausweisen (siehe 5).

### 2.4 Wo läuft das Schreiben?

12.1 setzt das Journal in `record_and_telemeter()`, 12.3 verlangt für
XIP-Targets eine „niedrig priorisierte Aufgabe … während der Tick aus dem
RAM weiterläuft".

Hier liegt die Falle, die **FB-149** schon einmal gestellt hat: Die
Telemetrie saß an derselben Stelle, blockierte auf `TXE` und machte aus
1,0 s Blinkzyklus 6,0 s. Eine Sektorlöschung ist zwei Größenordnungen
teurer als eine UART-Zeile.

**Entscheidung: Das Journal blockiert den Tick nie — und der Trait macht
das unmöglich, nicht bloß unerwünscht.**

Die Schnittstelle ist ein Zustandsautomat, kein Aufruf:

```rust
pub trait Nvm {
    fn begin_erase(&mut self, slot: u8) -> bool;
    fn begin_write(&mut self, slot: u8, offset: u32, bytes: &[u8]) -> bool;
    fn poll(&mut self) -> NvmState;      // Idle | Busy | Done | Failed
    fn read(&mut self, slot: u8, offset: u32, into: &mut [u8]) -> bool;
}
```

`begin_*` startet und kehrt sofort zurück; `poll` fragt. Ein Treiber, der
blockieren *wollte*, könnte es nur in `begin_*` tun — und das fällt in
der Konformitätsprüfung auf (13.8), weil die Tickdauer es zeigt. Ein
Trait mit `fn write(&mut self, …) -> Result<(), E>` hätte die Blockade
dagegen eingeladen.

`read` darf blockieren: Es läuft einmal beim Start, vor dem ersten Tick,
wo es keine Deadline gibt.

### 2.5 Wo liegt das Journal im Code?

| Option | Bewertung |
|---|---|
| **A** Alles in `takt-rt-baremetal` | Referenz Zeile 1767 nennt es dort. Aber dann kann der Interpreter es nicht fahren, und die Prüfung gegen das Flash-Modell (8.11) bräuchte Hardware |
| **B** Die Journal-Logik in `takt-rt-core`, der Treiber-Trait je Profil | Die Logik ist plattformfrei: Ping-Pong, Sequenznummer, CRC32, Vergleich. Was plattformabhängig ist, steckt hinter `Nvm`. `takt-rt-core` ist `no_std`, das trägt |
| **C** Eigenes Crate `takt-journal` | Ein Crate für ~400 Zeilen, das nur `takt-rt-core` benutzt. YAGNI |

**Entscheidung: B.** Die Logik nach `takt-rt-core/src/journal.rs`, der
`Nvm`-Trait daneben in `loopcore.rs` zu den anderen Runtime-Traits.

Das hat eine Bedingung, die den Zuschnitt festlegt: **`takt-rt-core`
hängt bewusst nicht von `takt-mir` ab** — „Der Kern läuft auf dem Target
und darf die IR nicht kennen" (`Cargo.toml`, nach `m4.md` 5.4). Die
Journal-Logik sieht die Nutzlast deshalb nur als `&[u8]`; wer sie
erzeugt, weiß sie zu deuten. Der Slot-Kopf (2.7) ist reine Arithmetik und
CRC32, beides ohne MIR. Der CRC32 muss dafür erreichbar sein —
`takt-native/src/crc.rs:18` ist es, und es ist tabellenfrei, also
`no_std`-tauglich.
`takt-rt-baremetal` bekommt später die Flash-Implementierung, `takt-rt-linux`
eine dateibasierte — und für Tests genügt eine RAM-Attrappe im selben
Crate, die Fehler und Abbrüche injizieren kann.

Damit ist das Journal **ohne Hardware prüfbar**, und das ist der Punkt:
5.9 verlangt die Prüfung gegen Stromausfall, und die geht nur, wenn man
den Abbruch injizieren kann.

### 2.6 Der Interpreter: mitfahren oder nicht?

Der Interpreter hält das NVM heute als `HashMap<u64, Value>`
(`takt-interp/src/nvm.rs:54-56`) — bewusst nicht als Bytes, mit der
Begründung „eine eigene Byte-Kodierung wäre eine zweite Quelle neben dem
Journal" (`m5.md:383`).

Diese Begründung fällt jetzt weg: Die Byte-Kodierung *ist* die eine
Quelle (2.1), und `takt-mir` ist für beide Seiten erreichbar. Damit
stellt sich die Frage neu.

| Option | Bewertung |
|---|---|
| **A** Interpreter behält die `Value`-Abbildung | Einfach, aber dann prüft nichts, ob Kodierung und Dekodierung zueinander passen — der Fehler zeigte sich erst auf Hardware |
| **B** Interpreter bekommt dieselbe Journal-Logik | Satz 9.4.4 sagt, der Interpreter sei die Spezifikation. Ein Journal, das nur im erzeugten Code existiert, hat keine Spezifikation, gegen die es geprüft wird |

**Entscheidung: B, aber in zwei Schritten.** Zuerst bekommt `Nvm` einen
Byte-Pfad (`encode`/`decode` über `persist::value`), sodass Roundtrip und
Journal-Logik im Interpreter testbar sind. Die volle Kopplung an die
Tickschleife des Interpreters folgt erst, wenn der Differentialtest sie
vergleichen kann — sonst entstünde eine zweite Implementierung ohne
Vergleich, genau der Zustand aus FB-151.

### 2.7 Eintragsform

Ein Slot enthält **alle** persist-Variablen des Programms, nicht eine je
Variable. Begründung: 5.9 sagt „ein Schreibvorgang ändert genau einen
Slot" und „beim Start gewinnt der gültige Eintrag mit der höheren
Sequenznummer" — beides im Singular, über das Ganze. Ein Journal je
Variable bräuchte je Variable zwei Sektoren; bei drei Variablen in 14.7
wären das sechs Sektoren statt zwei.

```
Slot-Kopf (32 Byte, feste Größe):
  magic      u32   "TKPJ"     Erkennung eines beschriebenen Slots
  version    u16   1          Formatversion (11.3: Leser akzeptieren aeltere)
  count      u16              Zahl der Eintraege
  sequence   u64              monoton; der hoechste gueltige gewinnt
  logic_hash u64              gegen ein fremdes Programm im selben Flash
  length     u32              Nutzlaenge in Byte
  crc32      u32              ueber Kopf (ohne dieses Feld) und Nutzlast

Je Eintrag:
  type_hash  u64              der Schluessel aus 5.9
  length     u32              Laenge der Wertbytes
  bytes      …                kanonische Form (2.1)
```

Der `logic_hash` ist eine Zutat über 5.9 hinaus. Sie kostet acht Byte und
verhindert einen Fall, den der Typ-Hash allein nicht abdeckt: Zwei
verschiedene Programme auf derselben Hardware (Testaufbau, A/B-Image),
deren Variablen zufällig gleich heißen und gleich typisiert sind. Der
Typ-Hash wäre identisch, die Bedeutung nicht. `hash::program_hash` gibt
es bereits.

**Was beim Lesen schiefgehen darf.** Jeder Fehler — falsches Magic,
unbekannte Version, CRC-Fehler, fremder `logic_hash`, unbekannter
Typ-Hash, falsche Länge — führt zu genau einem Ergebnis: Der Eintrag
gilt nicht. Sind beide Slots ungültig, ist es der Erstlauf. Das Journal
faultet nie; es liefert Werte oder keine. Die vorhandene
`Load`/`Reason`-Unterscheidung (`nvm.rs:19-36`) trägt das schon.

---

## 3. Arbeit je Werkstück

### 3.1 `takt-mir/src/persist.rs` — die Byte-Form

Zu `type_hash` und `is_pod` kommen:

```rust
pub fn encode(p: &Program, v: &Value, ty: TypeId, out: &mut Vec<u8>) -> bool
pub fn decode(p: &Program, ty: TypeId, bytes: &[u8]) -> Option<(Value, usize)>
pub fn encoded_size(p: &Program, ty: TypeId) -> Option<u32>   // None bei variabler Laenge
```

Problem: `takt-mir` kennt `Value` nicht — der Typ lebt in `takt-interp`,
das von `takt-mir` abhängt, nicht umgekehrt.

Drei Auswege, und nur einer ist sauber: Die Kodierung arbeitet nicht über
`Value`, sondern über einen kleinen **Besucher**. `takt-mir` liefert die
Struktur (welcher Typ, welche Felder, welche Reihenfolge), der Aufrufer
liefert die Werte. Der Interpreter implementiert ihn über `Value`, der
Codegen über LLVM-Register.

```rust
pub trait Writer {
    fn bool(&mut self, v: bool);
    fn int(&mut self, v: i64, width: IntWidth);
    fn float(&mut self, bits: u64, width: FloatWidth);
    fn len(&mut self, n: u32);
}
```

Das hält die Form an *einer* Stelle und macht sie für beide Seiten
benutzbar — genau das, was `m5.md:383` mit „eine zweite Quelle" vermeiden
wollte.

`encoded_size` liefert `None` für `bytes`/`str`/`vec`/`map`, weil deren
Länge variabel ist. Das ist keine Einschränkung des Journals (die
Eintragslänge steht im Kopf), aber `takt size` braucht eine obere
Schranke — dafür `max_encoded_size`, das mit der Kapazität rechnet.

### 3.2 `takt-rt-core/src/journal.rs` — die Logik

```rust
pub struct Journal<N: Nvm> { … }

impl<N: Nvm> Journal<N> {
    pub fn load(&mut self, into: &mut [u8]) -> Loaded;  // beim Start, darf blockieren
    pub fn poll(&mut self, now: i64, current: &[u8]);   // je Tick, nie blockierend
    pub fn flush(&mut self);                            // vor reboot/jump/sleep, blockierend
}
```

`poll` ist der Zustandsautomat: vergleicht `current` mit dem zuletzt
geschriebenen Stand, prüft `min_interval`, startet bei Bedarf Erase und
Write, treibt sie über mehrere Ticks voran. Die Aufrufstelle ist
`Runtime::step` zwischen `watchdog.kick()` und `sink.record()`.

`no_std`, kein `alloc`: Die Puffer kommen als Slices vom Aufrufer, die
Größe rechnet der Compiler (`takt size`).

### 3.3 `takt-llvm` — der Lesepfad

Eine neue Funktion je Maschine mit persist-Variablen:

```
i32 @<maschine>_persist_snapshot(ptr %state, ptr %out, i32 %cap)
```

Sie schreibt die kanonische Form aller persist-Variablen der Maschine
nach `%out` und liefert die Länge. Das Gegenstück:

```
i32 @<maschine>_persist_restore(ptr %state, ptr %in, i32 %len)
```

Beide entstehen aus derselben Strukturbeschreibung wie 3.1 — der
`Writer`-Besucher, implementiert über LLVM-Register.

Die `without_persist`-Meldung aus `lower.rs:42-47` entfällt dann für
Maschinen, die beide Funktionen bekommen.

### 3.4 Der `Nvm`-Trait und drei Implementierungen

| Wo | Was |
|---|---|
| `takt-rt-core/src/loopcore.rs` | der Trait (2.4) |
| `takt-rt-core/src/journal.rs` | `FakeNvm` für Tests: RAM, mit Abbruchinjektion |
| `takt-rt-baremetal` | später, gegen die Flash-Peripherie des Boards |
| `takt-rt-linux` | später, dateibasiert |

`FakeNvm` ist der wichtigste: Er trägt `cut_at(byte)` und erfüllt damit,
was 8.11 als `CUT_AT_BYTE` beschreibt — ohne dass das Takt-Flash-Modell
(eine Maschine, M6) dafür schon existieren muss.

### 3.5 `min_interval` endlich lesen

Steht seit M0 in der MIR (`machine.rs:52`), wird von nichts gelesen. Das
Journal braucht einen Wert je Programm, nicht je Variable: Der Slot
enthält alle Variablen, also gilt das **Minimum** über alle deklarierten
`min_interval`. Fehlt die Angabe überall, gilt ein Default.

Der Default gehört nicht in die Sprache — 5.9 nennt keinen. Er gehört in
die Hardware-Konfiguration (8.10), zu den anderen Größen, die vom Target
abhängen: Ein Flash mit 10⁵ Zyklen verträgt mehr als einer mit 10⁴. Das
Format steht (`takt-mir/src/hardware.rs`), die Felder für Flash-Geometrie
und Default-Intervall kommen mit diesem Schritt dazu — `FORMAT_VERSION`
wird dabei erhöht.

---

## 4. Tests und Abnahme

Die Reihenfolge folgt der Lehre aus FB-150: **Erst das Verhalten prüfen,
dann den Text.** Drei der dortigen Tests prüften die IR-Zeichenkette und
übersahen darum, dass die Einheit falsch war.

1. **Roundtrip der Byte-Form**, über alle POD-Typen, gegen die
   Korpusprogramme: `encode` → `decode` ergibt denselben Wert. Dazu
   Stabilität: dieselbe Eingabe ergibt dieselben Bytes (sonst wäre der
   CRC nicht reproduzierbar).
2. **Journal-Logik mit `FakeNvm`**: Ping-Pong wechselt wirklich; die
   höhere Sequenznummer gewinnt; ein Slot mit falschem CRC wird
   verworfen; zwei gültige Slots → der neuere; beide ungültig →
   Erstlauf.
3. **Stromausfall**, die eigentliche Abnahme nach 5.9: Für **jedes** Byte
   *n* des Schreibvorgangs einen Lauf mit `cut_at(n)`, dann neu laden.
   Erwartung: Entweder der alte oder der neue Stand, nie etwas
   dazwischen, nie ein Fault. Das ist ein erschöpfender Test, kein
   Stichprobentest — die Schreibvorgänge sind klein genug.
4. **`min_interval` hält**: Bei 10 s und 1-ms-Tick höchstens ein
   Schreibvorgang je 10 000 Ticks.
5. **Der Tick bleibt frei**: Ein Lauf mit `FakeNvm`, dessen Erase 50 ms
   „dauert" (in Ticks gezählt), zeigt keinen einzigen Overrun. Das ist
   der Test, der FB-149 verhindert hätte.
6. **Differentiell** (Satz 9.4.4): `35_persist.takt` mit gefülltem
   Speicher auf beiden Seiten — dann prüft der Test endlich das Laden
   und nicht nur die Rechnung. Heute steht dort der Vermerk, dass er es
   *nicht* tut.

---

## 5. Was `takt size` dazu ausweisen muss

11.5 nennt den Posten „Flash (Code, Konstanten, `persist`-Journal)". Er
fehlt heute. Mit diesem Plan ist er rechenbar:

- **Flash**: 2 Slots × aufgerundet auf Sektorgröße. Die Sektorgröße
  **muss die Hardware-Konfiguration erst bekommen** — `Target` trägt
  heute `name`, `core_hz`, `c_target` und `t_io`
  (`takt-mir/src/hardware.rs:121`), keine Flash-Geometrie. Bis dahin ist
  der Posten `offen` im Sinne von 11.5, nicht geschätzt.
- **RAM**: ein Vergleichspuffer in Größe der Nutzlast (2.3), plus der
  Schreibpuffer.

Beides mit Herkunft `exakt`, sobald die Hardware-Konfiguration vorliegt —
die Zahl folgt aus der MIR.

---

## 6. Schritte

Jeder Schritt ist für sich prüfbar; kein Schritt braucht Hardware.

1. **Byte-Form** (3.1) mit Roundtrip-Tests. Die unwiderrufliche
   Entscheidung zuerst, solange nichts davon abhängt.
2. **`Nvm`-Trait und `FakeNvm`** (3.4). Klein, aber er legt die
   Nicht-Blockierbarkeit fest.
3. **Journal-Logik** (3.2) mit den Tests 2 bis 4.
4. **Interpreter-Anbindung** (2.6, Schritt eins): `Nvm` bekommt den
   Byte-Pfad, die Journal-Logik wird gegen den Interpreter gefahren.
5. **`min_interval` und Hardware-Konfiguration** (3.5).
6. **Codegen-Lesepfad** (3.3) mit Test 6 — ab hier wird differentiell
   verglichen.
7. **`takt size`** (5).
8. **`takt-rt-baremetal`-Treiber** gegen die Flash-Peripherie. Braucht
   Hardware; alles davor nicht.

---

## 7. Risiken

| Risiko | Gegenmaßnahme |
|---|---|
| Die Byte-Form erweist sich später als zu eng | Formatversion im Slot-Kopf (2.7); 11.3 verlangt sie ohnehin. Ein Leser akzeptiert ältere Versionen |
| Das Journal blockiert doch den Tick | Der Trait lässt es kaum zu (2.4), und Test 5 misst es. Zusätzlich: `Tick::overrun` zeigt es im laufenden Betrieb |
| Zwei Implementierungen driften (Interpreter/Codegen) | Beide über denselben Besucher aus `takt-mir` (3.1); Test 6 vergleicht sie |
| Flash-Verschleiß im Dauerbetrieb | `min_interval` plus Änderungsvergleich (2.3); ein Zähler der Schreibvorgänge gehört in die Telemetrie |
| Der Schreibvorgang überlebt den Stromausfall nicht | Test 3 prüft jedes Byte einzeln, nicht stichprobenhaft |

---

## 8. Was offen bleibt

- **Das Takt-Flash-Modell** als Maschine nach 8.11 (`flash_model(sectors,
  t_erase, t_program, CUT_AT_BYTE)`) gehört zu M6 und zur
  `takt-stdlib`, die es noch nicht gibt. `FakeNvm` deckt die
  Journal-Prüfung ab; das Modell prüft später *Programme*, die Flash
  ansteuern — eine andere Frage.
- **Die `campaign`-Kampagne** über `CUT_AT_BYTE` braucht Szenarien
  (v1.1) und bleibt bei M6, wie in `m5.md` 3.6 festgehalten.
- **XIP-Targets** (12.3): Die „niedrig priorisierte Aufgabe" ist auf dem
  STM32F401 gegenstandslos — es hat kein XIP (`takt-board-stm32f401`
  vermerkt es ausdrücklich). Der Trait ist so geschnitten, dass ein
  XIP-Target ihn erfüllen kann, aber geprüft wird das erst mit einem
  solchen Board.

---

## 9. Stand nach der Umsetzung (2026-09-15)

Schritte 1 bis 7 stehen; Schritt 8 (der Flash-Treiber für das Board)
braucht das Gerät und bleibt offen.

| Schritt | Wo | Beleg |
|---|---|---|
| 1 Byte-Form | `takt-mir/src/persist/value.rs`, `takt-interp/src/persist.rs` | 26 Roundtrip-Tests, darunter `SelftestResult` aus 14.7 |
| 2 `Nvm`-Trait, `FakeNvm` | `takt-rt-core/src/loopcore.rs`, `journal.rs` | `cut_at` als `CUT_AT_BYTE` |
| 3 Journal | `takt-rt-core/src/journal.rs` | 13 Tests, Stromausfall an jedem Byte |
| 4 Interpreter | `Nvm::payload`/`from_payload`, Trace-Zeile `persist <hex>` | Roundtrip über das echte Journal |
| 5 `min_interval`, Geometrie | `persist::min_interval_ns`, `hardware::NvmGeometry` (Format 2) | 3 Tests |
| 6 Codegen | `takt-llvm/src/persist.rs`: `_persist_snapshot`, `_persist_restore` | Snapshot byteweise gleich, Restore und Range-Ablehnung differentiell |
| 7 `takt size` | `persist-Journal (RAM)` exakt, `(Flash)` mit Geometrie | 2 Tests |
| Runtime | `Program::persist_snapshot/restore`, `Persist`, `Runtime::step_persisting` | Schleife schreibt, Neustart liest; langsames Gerät kostet keinen Overrun |
| Linux | `takt-rt-linux::FileNvm`, I/O im Hintergrund-Thread (12.2) | 4 Tests |
| MCU-Rahmen | `takt_mcu_init_with`, `takt_mcu_persist_snapshot/restore` | 17 Rahmentests |

**Wo die Umsetzung vom Plan abweicht, und warum.**

- **Der Journal-Aufruf liegt nach `step()`, nicht darin** (3.2 sagte
  „zwischen `watchdog.kick()` und `sink.record()`"). `step()` enthält den
  Schlaf, und vor dem Schlaf zu schreiben hieße, auf das Gerät zu
  warten — genau das Blockieren, das der Trait ausschließt. Ein
  begonnener Flash-Vorgang läuft in der Hardware weiter; die Runtime
  fragt nach dem Wachwerden nach. `Runtime::step_persisting` hält die
  Reihenfolge fest.
- **Zwei Puffer, nicht drei.** `stored` ist zugleich Vergleichsstand und
  Kopie der Nutzlast: `begin` kopiert hinein und schreibt daraus. Das
  war kein Sparen, sondern die Korrektur eines Fehlers — der CRC lief
  über den lebenden Puffer, der sich zwischen den Polls ändert (FB-153).
- **Kein `Writer`-Besucher** (3.1). `takt-mir` liefert `Encoder` und
  `Decoder` als Bausteine; Interpreter und Codegen zerlegen die Struktur
  je selbst. Dass beide dieselben Bytes erzeugen, prüft der Codegen-Test
  byteweise — das ist der Beleg, den der Besucher versprochen hätte.
- **`_init` ist geteilt.** Der Interpreter lädt zwischen `init_vars` und
  `machine::init`; der erzeugte Code hatte beides in `_init`. Jetzt gibt
  es `_init_vars` und `_enter` daneben, und `_init` bleibt als Verbund
  für Programme ohne Journal.
- **Der Rahmen meldet `PersistReset` nicht als Alert.** `_persist_restore`
  liefert die Zahl der übernommenen Einträge; der Rahmen vergleicht sie
  nicht. Der Differentialtest sieht die Wirkung trotzdem — ein
  verworfener Eintrag zeigt sich als Default in den Outputs.

**Was Schritt 8 braucht.** Eine `Nvm`-Implementierung gegen die
Flash-Peripherie des STM32F401 in `takt-board-stm32f401` (Sektorlöschen,
Programmieren, Busy-Abfrage über das `SR`-Register), zwei Sektoren in der
Hardware-Konfiguration, und im Bring-up `takt_mcu_init_with` statt
`takt_mcu_init`. Der Rest ist da.
