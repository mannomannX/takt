# MIR — Entwurf gegen die volle Referenz

Stand: umgesetzt und eingefroren (`crates/takt-mir`, Formatversion 1; Abschnitte 6 und 7).
Die MIR ist das eine Werkstück für Interpreter, Analysen, Codegen,
`takt size`, Orakel und Importer (plan.md, Prinzip 1). Sie wird hier gegen die *volle*
Referenz entworfen — mit Knoten für alles, was erst in v1.1, v1.2 oder v2 ausgeführt wird —,
dann als Rust-Typen in `crates/takt-mir` gebaut, gegen die Inventur geprüft und eingefroren.
Nicht umgesetzt heißt danach: der Knoten existiert, Sema oder Interpreter melden „ab vX".

## 1. Abwägung: welche Art von IR?

| | Typisierter AST | Strukturierte, entzuckerte Maschinen-IR | Kontrollflussgraph (Basisblöcke, wie rustc-MIR/LLVM) |
|---|---|---|---|
| Nähe zur Semantik 9.x | hoch, aber mit Zucker (Sequenzen, `pulse`, Generics) | 1:1 — `exec`, `step_m`, `tick` laufen darauf | die Zustandsstruktur und die `max`-Kosten gehen verloren |
| Interpreter | muss Zucker selbst auflösen | direkt (Big-Step über Knoten) | Small-Step über Blöcke, weit von 9.x |
| Analysen (Intervalle, Definite Assignment, Kosten 9.4.3) | über Zucker unklar | strukturell (`N(if) = cost + max`, je Zustand) | klassisch, aber Kostenmodell und Overlay (11.2) brauchen die Struktur wieder |
| Codegen | zu weit weg | LLVM baut den CFG selbst aus strukturiertem Code (wie Clang) | nah am Backend |
| VM (v2) | ungeeignet | als Bytecode je Knoten kodierbar | natürlich |

Die Semantik der Referenz ist Big-Step und strukturiert: `exec` über Statements,
`step_m` über die Zustandskette, `tick` über Maschinen; das Kostenmodell 9.4.3 rechnet mit
`max` über Zweige und Konfigurationen; der Overlay exklusiver Zustandsspeicher (11.2) und
die Fault-Pfade brauchen den Zustandsbaum. Ein CFG würde all das wegwerfen und in Analysen
und `takt size` rekonstruieren. **Entscheidung: strukturierte, vollständig typisierte,
entzuckerte Maschinen-IR.** Der Kontrollflussgraph entsteht erst im Backend (M4), lokal je
Schrittfunktion, so wie Clang ihn aus dem AST baut. Für die VM (v2) reicht eine
Bytecode-Kodierung der strukturierten Knoten (Loop-Sprache, 9.4 Bemerkung).

Konsequenzen:

- **Vollständig aufgelöst.** Jeder Name ist ein Index (`VarId`, `StateId`, `OutputId`,
  `StreamId`, `MachineId`, `TypeId`, `UnitId`, `FnId`); Namen und Spannen bleiben in
  Seitentabellen für Diagnosen, Telemetrie (`pc = LINE`, 11.2) und Aufzeichnung.
- **Vollständig typisiert.** Jeder Ausdruck trägt seinen Typ (interniert), Einheiten als
  `UnitId` mit nominaler Identität (3.2) und Dimensionsvektor in der Einheitentabelle,
  Ranges als Annotation; implizite Konversionen und Prüfungen (4: Division, Index,
  Konversion, Range) sind *explizite* Knoten, die M3 einsetzt — der Interpreter führt aus,
  was da steht, und nichts Verborgenes.
- **Entzuckert.** Sequenzen werden nach 6.2 zu Zuständen; `pulse` zu `at` (9.2); Generics
  monomorphisiert (3.12); Blöcke als Instanzen mit eigenem Zustand; `check … for d` mit
  `viol`-Stelle; `every` mit Zähler; Timer als Tick-Zähler je Ebene (7.1).
- **Zweistufig innerhalb eines Typs.** Die Oberfläche der MIR darf einen `Sequence`-Knoten
  enthalten (Ergebnis des Lowerings aus dem AST); `desugar()` in `takt-mir` ersetzt ihn
  nach 6.2 durch Zustände und liefert die Kern-MIR (Invariante `is_core`). So bleibt die
  Desugaring-Regel als Code an einem Ort, die `step "name"`-Telemetrie kennt ihre Segmente,
  und `takt graph` kann beide Stufen zeigen.
- **Editionsfrei.** Editionen sind Frontend-Eigenschaft (2.5); die MIR trägt die Edition
  nur als Hash-Anteil.

## 2. Ebenen und Knoten

Die Tabellen nennen je Knoten die Felder in Kurzform und die Stelle der Referenz. Knoten
späterer Stufen tragen die Stufe; ihre Felder sind vollständig, ihre Ausführung meldet
„ab vX", bis der Meilenstein sie umsetzt.

### 2.1 Programm

| Knoten | Felder | Referenz |
|---|---|---|
| `Program` | `edition`, `tick: T0`, `output_timing`, `fault_is_fail`, `float_width`, `tick_source`, `tick_tolerance`, `target`, Tabellen: `types`, `units`, `fns`, `natives`, `blocks`, `machines`, `channels`, `streams`, `outputs`, `commands`, `params`, `profiles`, `nodes`, `properties`, `scenarios`, `campaigns`, `triggers`; `hash: LogicHash` | 2.4, 7.1, 8.1, 8.4, 11.3 |
| `Channel` | `dir`, `name`, `ty`, `binding: Hw(addr) \| Sim(addr) \| None`, `attrs` (safe, max_age, rate, max_rate, capacity, framing, overflow, wake, jitter, max_slew, debounce, capacity_bytes, expect_len, irreversible), `meta` (label, display, group, doc), `owner: Option<MachineId>` | 8.1, 8.2, 8.6, 8.8, 2.5 |
| `Stream` (intern) | `elem`, `capacity`, `writer: MachineId`, `readers` | 8.6 |
| `Param` | `name`, `ty`, `default`, `tunable`, `meta` | 8.4 |
| `Profile` | `name`, `assignments` | 8.4 |
| `Command` | `name`, `attrs`, `meta` | 8.5 |
| `Node` (v2) | `name`, `address`, `tick`, Platzierung der Maschinen | 12.9 |
| `Property` (v1.1) | `name`, `formula: TProp`, `monitor`, Zähler und Ringpuffer als Speicherbedarf | 13.3 |
| `Scenario` (v1.1) | `name`, `every`, `body: MachineBody` (schreibt nur `sim`-Outputs) | 8.3, 13.x |
| `Campaign` (v1.1) | `program`, `profile`, `sweeps`, `repeat`, `stop_on` | 13.7 |
| `Trigger` (v1.2) | `name`, `node`, `guard`, `then: AtStmt`, `bound`, `handle: TriggerHandleSlot` | 7.5 |
| `LogicHash` | Hash über die serialisierte MIR ohne Bindungen und Metadaten; enthält Edition und `float_width` | 11.3, 2.5, 4.2 |

### 2.2 Typen und Einheiten

| Knoten | Felder | Referenz |
|---|---|---|
| `Type` (interniert) | `Bool`, `Int{width: I8…I64 \| U8…U64, unit, range}`, `Float{width, unit, range}`, `Duration{range}`, `Enum{id}`, `Record{id}`, `Array{elem, n}`, `Bytes{n}`, `Vec{elem, n}`, `Str{n}`, `Line{n}`, `Samples{elem, n}`, `Table{k, v}`, `Mat{r, c, unit \| dims}`, `Map{k, v, n}`, `Optional{t}`, `Result{t, e}`, `Stream{elem}`, `Capture{elem, n}`, `Handle{Job \| Trigger}` | 3.1–3.11, 4.5, 7.5, 8.9 |
| `EnumDef` | `name`, `variants: [{name, discriminant, fields}]`, `layout`, `open`, `builtin` | 3.7, 2.5 |
| `RecordDef` | `name`, `fields: [{name, ty, const_value, offset, len_field, bits}]`, `layout: {endian, align}` | 3.7 |
| `UnitDef` | `name`, `dimension: [i8; BASE]`, `factor: Rational`, `affine_offset`, `predefined` | 3.2 |
| `Range` | `lo`, `hi` als Konstanten des Typs; Herkunft (deklariert \| bewiesen) | 3.4 |

### 2.3 Funktionen, Natives, Blöcke

| Knoten | Felder | Referenz |
|---|---|---|
| `Fn` | `name`, `params: [{name, ty, inout}]`, `ret`, `body: Block`, `cost: CostVec`, `stack`, `pure`; monomorphisierte Instanz einer generischen Vorlage (`origin`, `args`) | 3.9, 3.12, 9.4.3 |
| `Native` | `name`, `params`, `ret`, `cost: {i32,i64,f32,f64,mem,call,native}`, `stack`, `duration` (Job), `total`, `from` | 4.5 |
| `BlockDef` | `name`, `params`, `state_vars`, `step: Fn`, `methods: [Fn]`, Instanzen tragen ihren Zustand im Maschinenspeicher | 5.7 |
| `CostVec` | sieben Klassen | 9.4.3 |

### 2.4 Maschinen

| Knoten | Felder | Referenz |
|---|---|---|
| `Machine` | `name`, `driver`, `params`, `period n_m`, `phase`, `follows`, `node`, `vars` (Slots, Overlay-Gruppen), `persist: [PersistVar]` (v1.1), `signals`, `fault_target`, `states: Tree<State>`, `initial`, `loop`, `handlers`, `budget: {B_m, F_m}`, `timers`, `every_counters`, `viol_sites`, `jobs_max K_j`, `pub_vars`, `instances: [ScopedInstance]` (v1.2), `meta` | 5.1, 5.3, 5.7, 5.9, 7.2, 9.1 |
| `State` | `name`, `parent`, `children`, `initial`, `idle` (v1.1), `resume: Option<SavedPathSlot>` (v1.2), `vars` (zustandslokal, gehoben), `enter`, `exit`, `loop`, `handlers`, `transitions`, `fault_target`, `sequence: Option<Sequence>` (nur Oberfläche), `instances` (v1.2), `done_flag`, `meta` | 5.1, 5.8, 5.10–5.12, 6.2 |
| `Transition` | `trigger: When(Guard) \| After(DurationExpr)`, `actions`, `target`, `kind: Strong \| Weak` (6.2), `fault_kind: Option<FaultKind>` (Timeout, Expect) | 5.2, 6.2 |
| `Guard` | `Expr(bool)` \| `Match{subject, pattern, binding}` \| `Next{stream, binding}` (nächstes Element) | 5.2, 8.7 |
| `Handler` | `stream`, `pattern: Option<Pattern>`, `binding`, `body`, `dfa: Option<Dfa>` (M2) | 8.7 |
| `Sequence` (Oberfläche) | Items `Stmt \| Wait \| Until{guard, timeout: Option<{d, action}>} \| Expect \| Repeat{n, body} \| Step{name, body}`; Desugaring nach 6.2 in `desugar()` | 6.2 |
| `PersistVar` (v1.1) | `name`, `ty` (POD), `default`, `min_interval`, `type_hash` | 5.9 |
| `ScopedInstance` (v1.2) | `name`, `template`, `args`, `index_range`, `resume`, `scope: StateId` | 5.11 |
| `FaultTarget` | `state` oder `FAULTED`; `fault_kind`-spezifische Ziele | 5.3 |

### 2.5 Anweisungen (genau die Formen von 9.2)

| Knoten | Referenz | Knoten | Referenz |
|---|---|---|---|
| `Assign{target: Place, value}` | 9.2 | `Send{stream, value, len_max}` | 9.2, 8.8 |
| `Check{cond, msg, confirm: Option<{d, viol_site}>, target, req, kind: Check \| Expect}` | 5.6, 6.2 | `At{time, body}` | 9.8 |
| `Goto{target}` (nur in RUN wirksam) | 9.2 | `Cancel{output}` | 9.8 |
| `Abort{msg}` | 5.4 | `Raise{signal}` | 5.8 |
| `Seq[...]` | 9.2 | `Job{handle, fn, args}` (v1.1) | 4.5 |
| `If{cond, then, else}` | 9.2 | `Every{d, counter, body}` | 7.2 |
| `ForRange{var, n, body}` | 9.2 | `Break` | 9.2 |
| `ForWindow{var, stream, body}` | 9.2, 9.6 | `Observe{kind: Alert \| Log \| Measure \| Verify \| Verdict, …}` | 5.6, 13.x |
| `Match{subject, arms: [{pattern: Variant{fields} \| Values \| Wild, body}]}` | 9.2 | `Arm{trigger, on: bool}` (v1.2) | 7.5 |
| `Return{value}` (nur in `Fn`) | 3.9 | `Pass` | — |

`pulse o = v for d` ist kein Knoten: das Lowering erzeugt `Assign; At{now + d, Assign(latch)}`
(9.2). `var x = e` ist ein `Assign` auf einen deklarierten Slot (Definite Assignment prüft
M3). `Place` ist `Var \| Output \| Field(place) \| Index(place, i) \| Index2(place, i, j)`.

### 2.6 Ausdrücke (typisiert, total)

`Expr{kind, ty, span, range: Option<Range>}` mit `kind`:

| Gruppe | Knoten |
|---|---|
| Literale | `Bool`, `Int`, `Float`, `Duration`, `Str`, `None`, `Default`, `Variant{enum, variant, fields}`, `Record{fields}`, `Array`, `Tuple` (Stützstelle), `InstanceArray` |
| Zugriffe | `Var`, `Param`, `Tunable`, `Input{channel}` (mit Qualitätsdominanz als Flag), `Published{machine, var}`, `StateOf{machine}`, `Signal`, `Builtin{Now \| Tick \| TimeInState \| LastFault \| Event}`, `Field`, `Index`, `Index2`, `Slice`, `Accessor{Valid \| Suspect \| Stale \| Age \| Reason \| Ok \| Err \| Done \| Result \| Len \| Count \| Armed \| Fired \| …}` (die 60 reservierten Membernamen, je Typ erlaubt) |
| Operatoren | `Unary{Neg \| Not \| BitNot}`, `Binary{…}` (alle Operatoren aus 2.3, klassenweise), `Cmp`, `Cond{c, a, b}`, `Cast{to}`, `Convert{unit_to}` (`to`, `to_float`, `as(unit)`), `Matches{subject, pattern, binding}`, `Has` |
| Aufrufe | `Call{fn, args}`, `NativeCall`, `BlockStep{instance, args}`, `BlockMethod`, `MatOp{Mul \| Transpose \| Inv \| Det \| Solve \| Cholesky}` |
| Eingefügt (M3) | `Checked{op, kind: DivZero \| Overflow \| Index \| Range \| Convert}` — jede implizite Prüfung als Knoten, mit Warnung 4 |
| Eigenschaften (v1.1) | `TProp`: `Always`, `Never`, `Eventually{d}`, `Stable{d}`, `Once{d}`, `Implies`, plus `Expr` als Atom (2.3 `tprop_atom := … \| cmp_expr`) |

Ausdrücke sind seiteneffektfrei (4.4); `eval` ist total mit `FAULT` als Wert (9.2).

### 2.7 Muster, Format, Adressen

| Knoten | Referenz |
|---|---|
| `Pattern::Text{pieces: Text \| Capture{name, kind: Int \| Hex \| Float \| Word \| Str{n}} \| Any}` (aus `subtext::pattern_text`) und `Pattern::Record{ty, fields}`; `Dfa` mit Alphabetklassen als spätere Annotation (11.2) | 8.7 |
| `Format{pieces: Text \| Expr{e, spec}}` in Meldungen; `len_max` statisch | 3.9, 16 |
| `Address{segments}` aus `subtext::address_text`; die Bindung an Treiber kommt aus der Hardware-Konfiguration | 8.1, 8.10 |

### 2.8 Laufzeitzustand und Speicher (Beschreibung, nicht Code)

Die MIR beschreibt Σ (9.1) als Layout je Maschine: Slots der Variablen, Overlay-Gruppen
der Geschwisterzustände (11.2), Blockzustände, Timer, Zähler, Cursor, Sendepuffer,
gespeicherte Pfade (`resume`), Trigger-Flags, Job-Handles, Property-Puffer. `takt size`
(11.5) liest nur diese Beschreibung. Der Interpreter legt Σ danach an; der Codegen
erzeugt daraus die `*_state`-Structs (11.2).

## 3. Abbildung der Inventur

Jede SEM-, G-, PAR-, RULE-, FN-, DESUGAR- und MEM-Zeile der Inventur wird einem Knoten,
einer Desugaring-Regel, einer Sema-Prüfung oder einem Runtime-Teil zugeordnet — als
maschinell geprüfte Tabelle, nicht als Prosa:

```
plan/mir_map.csv:   ID, Ziel (Knoten|Regel|Prüfung|Runtime|Rationale), Name, Bemerkung
plan/check_mir_map.py: jede nicht-rationale ID kommt vor; jeder genannte Knoten existiert
                        als `pub struct`/`pub enum`-Variante in crates/takt-mir/src
```

Das ist dieselbe Disziplin wie `productions.rs` für Parser und Formatter: eine Grammatik-
oder Semantikzeile ohne MIR-Ziel bricht den Test. Der Review ist erst fertig, wenn der
Checker leer meldet.

## 4. Serialisierung und Hash

**Abwägung: Serde oder eigenes Format?** Die serialisierte MIR ist ein Vertrag für die VM
(v2), den Logik-Hash und das Replay; Leser müssen ältere Versionen ihres Formats
akzeptieren (11.3). Serde-Derivate binden das Format an die Feldreihenfolge der
Rust-Typen; jede Umstellung bräuchte Migrationscode außerhalb des Formats. Ein eigenes,
dokumentiertes Format mit nummerierten Feldern (Tag–Länge–Wert, wie Protobuf) versioniert
sich selbst: neue Felder bekommen neue Nummern, alte Leser überspringen Unbekanntes.
**Entscheidung: eigenes binäres Format `TAKT-MIR`**, Kopf `{magic, format_version,
edition, compiler_version}`, Knoten als Tag–Länge–Wert, Strings und Tabellen einmal
interniert; Spezifikation in `grammar/mir-format.md` (wie `lexer.md` und `format.md`
Testvektoren). Keine Abhängigkeit; der Leser ist `no_std`-fähig (VM).

**Logik-Hash**: SHA-256 (eigene Implementierung in `takt-diag` oder `takt-mir`, 100
Zeilen, ohne Abhängigkeit) über die kanonische Serialisierung ohne Bindungen (`hw`/`sim`)
und ohne Metadaten (2.5); enthält Edition und `float_width`. Der Programm-Hash nimmt alles.

Roundtrip-Test: jede Knotenart mindestens einmal in einem konstruierten Programm,
schreiben, lesen, gleich; ein Leser der Version n liest Vektoren der Versionen ≤ n.

## 5. Umsetzung in Schritten

1. Dieses Dokument gegen die Referenz durchgehen: für jeden Abschnitt 3 bis 9 und 12.9,
   13.3 prüfen, ob ein Knoten fehlt (Review-Runde mit dem Autor der Referenz).
2. `crates/takt-mir`: Typen aus Abschnitt 2 als Rust-Typen, Seitentabellen, Ids;
   `desugar()` nach 6.2 mit dem Beispiel 6.3 als Test (Baum gleich dem Erwarteten);
   `mir_map.csv` und `check_mir_map.py`.
3. `grammar/mir-format.md`, Schreiber und Leser, Roundtrip-Tests, Logik-Hash.
4. Review gegen die Inventur, Checker leer, dann **Freeze**: Änderungen an der MIR nur
   mit Eintrag in `plan/mir.md` (Abschnitt 6) und Format-Versionssprung.
5. Danach M1: Sema lowert den AST in die MIR; der Interpreter führt sie aus.

Größenordnung: Typen 900 Zeilen, Desugaring 250, Format und Hash 600, Abbildung und
Checker 150 plus die CSV mit rund 450 Zeilen.

## 6. Änderungsgeschichte der MIR

| Datum | Formatversion | Änderung |
|---|---|---|
| 2026-09-10 | 3 | `StmtKind::Skip(StreamRef)`: `s.skip()` verwirft das Fenster (8.6). Es war als `Method::Skip` vorgesehen, aber eine Methode braucht eine `Place` als Empfaenger, und ein Strom ist keine Stelle; `Method::Skip` entfaellt darum ersatzlos. Tag 20. |
| 2026-09-10 | 3 | `ExprKind::Stream(StreamId)`: ein interner Strom als Wert — Subjekt eines Guards und Traeger der Zaehler `.count`, `.dropped`, `.overflowed`, `.malformed` (8.6). Ein Stream-Channel steht als `Input` da; dem internen Strom fehlt die `ChannelId`. Tag 41. |
| 2026-09-10 | 3 | `Guard::Match::kind`: `MatchKind` unterscheidet `until s matches P` von `until s has P` (8.7); ohne das Feld waeren beide Guards derselbe Knoten. Feld 4, `one`. |
| 2026-09-10 | 3 | `RecordDef::wire_size`: die Gesamtlaenge eines `layout`-Records nach `align` (3.7). Der Byteplan entsteht einmal im Sema und ist danach die einzige Quelle fuer Interpreter und Codegen; ohne das Feld muesste jede Seite die Groesse verschachtelter Records neu rechnen (plan/m2.md 1.8). Feld 6, `opt`. |
| 2026-09-10 | 2 | `ExprKind::Lift` (T nach T?, 3.8), `ExprKind::Ok`/`Err` (Konstruktoren von T!E), `ExprKind::Intrinsic` mit `Intrinsic` (Primitive mit eigener Fault-Semantik: `sqrt`, `round`, `fma`, `interp`, `rotl`, `wrapping_add`, …); gefunden beim Entwurf des Lowerings (plan/m1.md, Abschnitt 2). |
| 2026-09-10 | 1 | Freeze: Knoten aus Abschnitt 2 als Rust-Typen, `desugar` nach 6.2, Format `TAKT-MIR` (grammar/mir-format.md), Logik-Hash, Abbildung `plan/mir_map.csv` (Prüfer leer). Änderungen ab hier nur mit Zeile in dieser Tabelle; Feldnummern werden nie umvergeben, neue Felder sind `opt` oder `rep`, ein neues Pflichtfeld verlangt einen Versionssprung. |

## 7. Stand nach der Umsetzung

Abweichungen vom Entwurf in Abschnitt 2, jeweils mit Grund:

- **`BlockStep`/`BlockMethod` sind keine Ausdrücke**, sondern `StmtKind::MethodCall`
  (`target = receiver.method(args)`), ebenso `push`, `insert`, `remove`, `clear`, `skip`,
  `reset`. Ausdrücke bleiben damit seiteneffektfrei (4.4); `Accessor` enthält nur die reinen
  Zugriffe, `ConvertKind` die Konversionen, `MatOp` die Matrixoperationen.
- **`Tunable` ist kein eigener Knoten**: `Param.tunable` (8.4); `ExprKind::Param` liest beides,
  der Interpreter behandelt Tunables als Input mit Halte-Semantik.
- **`Input` trägt `dominated`** (statisch unter `.valid`, 3.5); die implizite Validitätsprüfung
  ist sonst ein `Checked{Valid}`.
- **Eigenschaften** sind ein eigener Typ `TProp` mit `Atom(Expr)`, nicht Teil von `ExprKind`,
  damit Monitore (13.3) ihre Zähler je Operator anlegen können.
- **Maschinenrollen** über `MachineKind` (Regular, Template, Instance, Scenario); Instanzen
  sind eigene Maschineneinträge, `MachineRef` adressiert Instanz-Arrays mit Indexausdruck.
- **`Layout`** beschreibt Σ ohne Slot-Tabelle: der Overlay folgt aus `VarScope::State`/`Lifted`;
  Timer, `every`-Zähler, `viol`-Stellen, Blockinstanzen, `resume`-Pfade, Trigger-Flags,
  Ausgabewarteschlangen und Cursor sind Tabellen.
- **Der Logik-Hash ist kein Feld** von `Program`, sondern `hash::logic_hash(&Program)`; er
  läuft über die Serialisierung ohne `meta`-Felder (Positionen, Bindungen, Metadaten).
  Analyse-Annotationen (bewiesene Ranges, Kosten, Budget, Timer-Breite) sind Teil des Hashs;
  ein Programm hat damit je Toolchain-Version einen Hash (11.3 „Reproduzierbare Builds").
- **Desugaring** (`desugar.rs`): Segmente heißen `ELTERN.Si`; `repeat` beginnt ein neues
  Segment (`when true`, ein Tick), sofern das aktuelle nicht leer ist, und setzt den Zähler
  beim Verlassen zurück, damit verschachtelte `repeat`s ohne Zusatzzustand korrekt sind;
  `expect` bekommt eine zustandslokale Flagvariable; Zweige einer `if`-Kette erhalten ihre
  exakte Bedingung als Guard (statt `when true` am Ende); `->` ist nur als letzte Anweisung
  eines Zweigs erlaubt, sonst Fehler `MIR`; ein Zustand mit Sequenz und Kindern ist ein Fehler.
  Voraussetzung aus dem Lowering (M1): `var`, Captures und `repeat`-Zähler sind gehoben.
- **Abbildung der Inventur**: `plan/mir_map.csv` kennt neben Knoten, Regel, Prüfung, Runtime
  und Rationale die Ziele *Lowering* (Konstrukt wird auf einen bestehenden Knoten abgebildet,
  etwa `pulse` → `At`) und *Frontend* (vor der MIR erledigt, etwa `import`). Der Prüfer
  `plan/check_mir_map.py` verlangt, dass jeder Knoten im Code existiert, jede Regel eine
  Funktion ist, jede Prüfung eine SC-Zeile und jede Komponente in 11.1 steht.
- **Tests**: `tests/desugar.rs` (6.3 und je eine Regel der Tabelle), `tests/roundtrip.rs`
  (Beispielprogramm mit jeder Knotenart, Kopf, unbekannte Felder, Hashes, Membernamen gegen
  2.5), `tests/format_vectors.rs` (Vektoren aus `grammar/mir-format.md`).
