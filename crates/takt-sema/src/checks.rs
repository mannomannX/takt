//! Pruefungen auf der MIR (plan/m1.md 3.10): Fault-Wald (9), Single-Writer
//! und Bindungen (7), Erreichbarkeit (10), Terminierung (11), Simulation (13),
//! ungenutzte Channels (15), Definite Assignment gehobener Variablen (6, 25).
//!
//! Sie laufen nach dem Lowering, aber vor dem Desugaring: dort sind Namen
//! aufgeloest, und die Sequenz-Oberflaeche zeigt noch ihre Segmentgrenzen.

use std::collections::{HashMap, HashSet};

use takt_diag::{Diagnostic, Span};
use takt_mir::expr::{BinaryOp, Expr, ExprKind, StreamRef};
use takt_mir::machine::*;
use takt_mir::program::{Binding, Direction, RuntimeProfile};
use takt_mir::stmt::*;
use takt_mir::sys::{self, SysType};
use takt_mir::types::Type;
use takt_mir::*;

use crate::lower::Lowerer;

/// Single-Writer, Richtung, Bindungen.
pub const SC7: &str = "SC-7";
/// Stroeme: Kapazitaeten, Konsumentenmenge, Elementtyp (8.6).
pub const SC17: &str = "SC-17";
/// Muster: Wohlgeformtheit, Mehrdeutigkeit, Capture-Typen (8.7).
pub const SC18: &str = "SC-18";
/// `send`: Summe der Hoechstlaengen je Aktivierung (8.8).
pub const SC20: &str = "SC-20";
/// `at`/`pulse`: nur Zuweisungen an skalare Outputs, `T` ist Duration (7.5).
pub const SC21: &str = "SC-21";
/// Interne Streams: genau ein Schreiber (8.6).
pub const SC43: &str = "SC-43";
/// `T!E`: Dominanz durch `.ok`, `match` erschoepfend (3.8).
pub const SC45: &str = "SC-45";
/// `layout`: Offsets, Bitfelder, Konstantenfelder, Diskriminanten (3.7).
pub const SC46: &str = "SC-46";
/// `inout`: kein Aliasing (3.9).
pub const SC47: &str = "SC-47";
/// Pruefung 25: Definite Assignment zustandslokaler und gehobener Variablen
/// je Eintritt (Sequenz-`var`, Captures). Pruefung 6 ist die allgemeine
/// Regel.
///
/// Der Code steht, ist aber derzeit nicht ausloesbar: `VarDef::init` ist nur
/// an einer Stelle `None` — der Musterbindung in `lower/pattern.rs` —, und
/// jeden Lesezugriff darauf faengt schon Pruefung 6 ab. Ein Sequenz-`var`
/// ohne Initialisierer, der zweite Fall der Regel, ist grammatisch gar nicht
/// schreibbar (2.3: `var_decl` verlangt `=`). Erst wenn die Grammatik ihn
/// zulaesst, bekommt 25 einen eigenen Fall; bis dahin haelt
/// `tests/analysis.rs` den Grund fest.
pub const SC25: &str = "SC-25";
/// Maschinenregeln.
pub const SC8: &str = "SC-8";
/// Fault-Wald.
pub const SC9: &str = "SC-9";
/// Erreichbarkeit und tote Uebergaenge.
pub const SC10: &str = "SC-10";
/// Terminierung.
pub const SC11: &str = "SC-11";
/// Simulation.
pub const SC13: &str = "SC-13";
/// Performance-Lint: `int` ohne Range auf schmalen Kernen (3.4).
pub const SC40: &str = "SC-40";
/// Performance-Lint: `float = f64` ohne f64-Hardware (4.2).
pub const SC41: &str = "SC-41";
/// Irreversible Outputs (12.7).
pub const SC48: &str = "SC-48";
/// Laengenpraefixierte Felder (3.7).
pub const SC37: &str = "SC-37";
/// `within d`: die geforderte Safe-State-Latenz wird eingehalten (9.4.5).
pub const SC61: &str = "SC-61";
/// `budget = {ram = …}`: das deklarierte Budget wird eingehalten (7.2).
pub const SC62: &str = "SC-62";
/// Kostenbudget je Maschine, Schedulability und Stack-Schranke (9.4.3, 7.2).
pub const SC12: &str = "SC-12";
/// Native Funktionen: kuratierte Menge, Signatur, `cost`/`stack`/`total`,
/// `duration` nur an Jobs (4.5).
pub const SC31: &str = "SC-31";
/// `property`: Fenster positiv und ein Vielfaches des Ticks, `always`/`never`
/// nicht unter einem beschraenkten Operator, Atome `bool` (13.3).
pub const SC56: &str = "SC-56";
/// `map<K, V, N>`: der Schluessel ist POD mit Gleichheit — ohne
/// Fliesskomma (3.9).
pub const SC57: &str = "SC-57";
/// Registerports: nur in einer `driver machine` (12.10).
pub const SC64: &str = "SC-64";
/// Gepolltes Geraet laeuft zwischen zwei Ticks nicht ueber (12.10).
pub const SC59: &str = "SC-59";
/// `tunable param` steht nicht, wo eine Compile-Zeit-Konstante verlangt
/// ist (8.4): Array-Groessen, Kapazitaeten, `repeat`.
pub const SC35: &str = "SC-35";
/// Jobs: `native job` als Ziel, hoechstens `K_j` Handles je Maschine, ein
/// Handle je Native, nur in einer Maschine (4.5).
pub const SC44: &str = "SC-44";
/// Schedulability mit Abort-Phase, klassenweise (7.2).
pub const SC32: &str = "SC-32";
/// `idle`: kein `loop:`/Handler, Guards nur ueber Wake-Quellen (5.10).
pub const SC22: &str = "SC-22";
/// `resume`: nur an zusammengesetzten Zustaenden mit `initial`, nicht an
/// `idle` (5.12).
pub const SC54: &str = "SC-54";
/// Trigger: Guard und Outputs knotenlokal, `arm`/`disarm` nur aus der
/// deklarierenden Maschine, `d >= bound > 0` (7.5).
pub const SC55: &str = "SC-55";
/// Gescopte Instanzen: kein Selbst-Scoping (5.11).
///
/// Die uebrigen Teile von 53 fallen woanders: Single-Writer global ist
/// Pruefung 7, die keine Ausnahme fuer Exklusivitaet kennt — und genau
/// das will 5.11. Kein Scope in einem `idle`-Zustand ist Pruefung 22,
/// die `idle` ohnehin auf einen leeren Schritt prueft. Zugriff nur im
/// Scope ist Namensaufloesung: Der Name steht im Zustand, ausserhalb ist
/// er nicht sichtbar.
pub const SC53: &str = "SC-53";
/// `persist var`: POD-Typ, Maschinenebene, nicht in Szenarien (5.9).
pub const SC23: &str = "SC-23";
/// `at` gegen den gemessenen Jitter eines Outputs (7.5, 8.1).
pub const SC28: &str = "SC-28";
/// `mat`: Form bei `+ - *`, `inv`/`solve` nur quadratisch, Indizes in Range (3.11).
pub const SC30: &str = "SC-30";
/// Dimensionierte Matrizen: Einheitentupel passen (3.11).
pub const SC34: &str = "SC-34";
/// Kampagne: Sweep-Schritt eines in `at` verwendeten Parameters gegen
/// 2·jitter des Outputs (13.7).
pub const SC29: &str = "SC-29";
/// Speicherbudget gegen `ram`/`flash` der Hardware-Konfiguration (11.5).
pub const SC39: &str = "SC-39";
/// Channel-Bindung gegen die Hardware-Konfiguration (8.10).
pub const SC60: &str = "SC-60";
/// Lints: `alert`-Polaritaet, Profil-Vollstaendigkeit (5.6, 4.6).
pub const SC63: &str = "SC-63";
/// `follows` (7.2): Kanten azyklisch; zwei Warnungen (FB-18, FB-38).
pub const SC33: &str = "SC-33";
/// Szenarien schreiben nur `sim`-Outputs; Single-Writer gegenueber
/// Modellmaschinen (13.6).
pub const SC26: &str = "SC-26";
/// Lints zu Matrizen und Stroemen (3.11, 8.6).
pub const SC42: &str = "SC-42";
/// Ungenutzte Channels.
pub const SC15: &str = "SC-15";
/// Definite Assignment (allgemeine Regel). Noch ohne Fundstelle: jedes `var`
/// traegt einen Initialisierer (2.3, `var_decl`), und die einzige
/// uninitialisierte Bindung kommt aus `until … matches` (M2). Der Fall der
/// gehobenen Sequenzvariablen laeuft unter [`SC25`].
pub const SC6: &str = "SC-6";

impl Lowerer<'_> {
    /// Fuehrt alle MIR-Pruefungen aus.
    pub fn run_mir_checks(&mut self) {
        self.default_max_age();
        self.stream_capacities();
        self.check_send_budget();
        self.check_irreversible();
        self.check_sys_channels();
        self.performance_lints();
        self.check_writers();
        self.check_scenarios();
        self.check_fault_forest();
        self.check_follows();
        self.check_latency();
        self.check_declared_budget();
        self.check_cost_budget();
        self.check_idle_states();
        self.check_resume_states();
        self.check_triggers();
        self.check_persist();
        self.check_alert_polarity();
        self.check_profile_completeness();
        self.check_reachability();
        self.check_termination();
        self.check_simulation();
        self.check_unused();
        self.check_definite_assignment();
    }

    /// Pruefung 33 (7.2): `follows` azyklisch; dazu die zwei Warnungen aus
    /// plan/feedback-design.md 3 — das fehlende `follows` (FB-18) und die
    /// stille Bedeutungsaenderung auf dem Fault-Pfad (FB-38).
    fn check_follows(&mut self) {
        use takt_mir::analysis::schedule;
        let p = &self.program;
        let mut diags = Vec::new();
        if let Err(cycle) = schedule::order(p) {
            let names: Vec<String> = cycle.iter().map(|id| format!("`{}`", p.machines[id.index()].name)).collect();
            for id in &cycle {
                diags.push(
                    Diagnostic::error(
                        SC33,
                        p.machines[id.index()].span,
                        format!("`follows` bildet einen Zyklus: {}", names.join(", ")),
                    )
                    .with_suggestion("die Kanten muessen einen azyklischen Graphen bilden (7.2)".to_string()),
                );
            }
        }
        for (i, a) in p.machines.iter().enumerate() {
            if matches!(a.kind, MachineKind::Template) {
                continue;
            }
            let id = MachineId(i as u32);
            let mut reads: Vec<(MachineId, String, Span)> = Vec::new();
            for_each_expr_machine(a, &mut |e| {
                if let Some((b, name)) = published_read(p, e) {
                    reads.push((b, name, e.span));
                }
            });
            // Der Fault-Pfad: `enter` und `loop:` der Fault-Ziele (5.3, 5.4).
            let mut fault_reads: Vec<(MachineId, String, Span)> = Vec::new();
            let targets = std::iter::once(a.fault_target).chain(a.states.iter().filter_map(|s| s.fault_target));
            for t in targets {
                let FaultTarget::State(s) = t else { continue };
                for b in [&a.states[s.index()].enter, &a.states[s.index()].loop_block] {
                    for_each_expr_block(b, &mut |e| {
                        if let Some((m, name)) = published_read(p, e) {
                            fault_reads.push((m, name, e.span));
                        }
                    });
                }
            }
            let mut seen = HashSet::new();
            for (b, name, span) in &reads {
                // 13.6: ein Szenario liest mit Unit-Delay, das ist sein Vertrag.
                if *b == id || a.follows.contains(b) || a.kind == MachineKind::Scenario {
                    continue;
                }
                let bm = &p.machines[b.index()];
                if !schedule::same_tick(a, bm) || !seen.insert((*b, name.clone())) {
                    continue;
                }
                diags.push(
                    Diagnostic::warning(
                        SC33,
                        *span,
                        format!(
                            "`{}` liest `{name}` mit einem Tick Verzoegerung, obwohl `{}` im selben Tick laeuft",
                            a.name, bm.name
                        ),
                    )
                    .with_suggestion(format!(
                        "`follows {}` ergaenzen, wenn die frische Groesse gemeint ist (7.2)",
                        bm.name
                    )),
                );
            }
            for (b, name, span) in &fault_reads {
                let regular = reads
                    .iter()
                    .any(|(m, n, sp)| m == b && n == name && !fault_reads.iter().any(|(_, _, fsp)| fsp == sp));
                if a.follows.contains(b) && regular {
                    diags.push(
                        Diagnostic::warning(
                            SC33,
                            *span,
                            format!("`{name}` ist in der Schrittphase frisch, auf dem Fault-Pfad aber der Wert des vorigen Ticks (5.4)"),
                        )
                        .with_suggestion("in der Abort-Phase gilt Psi_k, nicht der frische Wert (7.2)".to_string()),
                    );
                }
            }
        }
        self.diags.extend(diags);
    }

    /// Traegt den Default fuer `max_age` ein (3.5): das Doppelte der
    /// kuerzesten Periode unter den Maschinen, die den Channel lesen. Die
    /// Zusicherung „dieser Wert ist frisch genug" gilt damit fuer jeden
    /// Leser, auch den schnellsten; ohne Default wurde ein toter Sensor nie
    /// `Stale`, und der implizite Validitaets-Check griff nie.
    fn default_max_age(&mut self) {
        let mut fastest: HashMap<ChannelId, u32> = HashMap::new();
        for m in &self.program.machines {
            if matches!(m.kind, MachineKind::Template) || m.states.is_empty() {
                continue;
            }
            let period = m.period.max(1);
            for_each_expr_machine(m, &mut |e| {
                if let ExprKind::Input { channel, .. } = &e.kind {
                    let slot = fastest.entry(*channel).or_insert(period);
                    *slot = (*slot).min(period);
                }
            });
        }
        let tick = self.program.config.tick;
        for (i, c) in self.program.channels.iter_mut().enumerate() {
            if c.dir != Direction::Input || c.attrs.max_age.is_some() {
                continue;
            }
            if let Some(period) = fastest.get(&ChannelId(i as u32)) {
                c.attrs.max_age = Some(2 * i64::from(*period) * tick);
            }
        }
    }

    /// Kapazitaeten der Stroeme und Pruefung 17 (8.6, Lemma 9.6.1).
    ///
    /// Der Default fuer `capacity` haengt an der groessten Periode unter den
    /// Konsumenten, die Pruefung an der Periode jedes einzelnen — beides ist
    /// erst bekannt, wenn alle Maschinen gelowert sind, also ein Nachlauf.
    fn stream_capacities(&mut self) {
        let tick = self.program.config.tick;
        let readers = self.stream_readers();

        // --- Channels mit Stream-Typ ------------------------------------
        for i in 0..self.program.channels.len() {
            let id = ChannelId(i as u32);
            let c = &self.program.channels[i];
            if !matches!(self.program.types.list.get(c.ty.index()), Some(Type::Stream(_))) {
                continue;
            }
            if c.dir != Direction::Input {
                continue;
            }
            let (name, span, elem) = (c.name.clone(), c.span, stream_elem(&self.program, c.ty));
            let periods: Vec<u32> = readers.get(&StreamRef::Channel(id)).cloned().unwrap_or_default();
            // 8.6: `max_rate` ist Pflicht an einem `hw`-Stream.
            let Some(max_rate) = self.rate_hz(i) else {
                self.error_hint(
                    SC17,
                    span,
                    format!("Stream `{name}` braucht `max_rate`"),
                    "`with max_rate = 2000 Hz` ergaenzen (8.6)",
                );
                continue;
            };
            // MAXPT = ceil(max_rate * T0): so viele Elemente treffen je
            // Basis-Tick hoechstens ein.
            let maxpt = ceil_div(max_rate.saturating_mul(tick as u64), 1_000_000_000).max(1);
            let p_max = periods.iter().copied().max().unwrap_or(1);
            let default_cap = (2 * maxpt.saturating_mul(u64::from(p_max))).clamp(1, u64::from(u32::MAX));
            let cap = match self.program.channels[i].attrs.capacity {
                Some(n) => u64::from(n),
                None => {
                    let n = default_cap as u32;
                    self.program.channels[i].attrs.capacity = Some(n);
                    u64::from(n)
                }
            };
            let elem_bytes = elem.and_then(|t| self.elem_bytes(t)).unwrap_or(1);
            let cap_bytes = match self.program.channels[i].attrs.capacity_bytes {
                Some(n) => u64::from(n),
                None => {
                    // 8.6: Default `capacity * N`, mit `expect_len` statt N,
                    // wenn deklariert — dann warnt der Compiler einmal.
                    let per = match self.program.channels[i].attrs.expect_len {
                        Some(n) => {
                            // Pruefung 42, zweite Klausel: die Annahme
                            // schwaecht Lemma 9.6.1 (8.6).
                            self.warn_hint(
                                SC42,
                                span,
                                format!("`expect_len = {n}` an `{name}`: Lemma 9.6.1 gilt nur unter dieser Annahme"),
                                "mittlere Laenge ueber `capacity` Elemente hoechstens `expect_len` (8.6)",
                            );
                            u64::from(n)
                        }
                        None => u64::from(elem_bytes),
                    };
                    let n = (cap.saturating_mul(per)).clamp(1, u64::from(u32::MAX)) as u32;
                    self.program.channels[i].attrs.capacity_bytes = Some(n);
                    u64::from(n)
                }
            };
            self.check_maxpt(&name, span, maxpt, &periods, Caps { cap, cap_bytes, elem_bytes: u64::from(elem_bytes) });
        }

        // --- interne Streams --------------------------------------------
        for i in 0..self.program.streams.len() {
            let def = &self.program.streams[i];
            let (name, span, elem) = (def.name.clone(), def.span, def.elem);
            let cap = u64::from(def.capacity);
            let periods: Vec<u32> = readers.get(&StreamRef::Internal(StreamId(i as u32))).cloned().unwrap_or_default();
            let elem_bytes = u64::from(self.elem_bytes(elem).unwrap_or(1));
            let cap_bytes = match self.program.streams[i].capacity_bytes {
                Some(n) => u64::from(n),
                None => {
                    let per = self.program.streams[i].expect_len.map_or(elem_bytes, u64::from);
                    let n = (cap.saturating_mul(per)).clamp(1, u64::from(u32::MAX)) as u32;
                    self.program.streams[i].capacity_bytes = Some(n);
                    u64::from(n)
                }
            };
            // 8.6: MAXPT eines internen Stroms ist die statische Hoechstzahl
            // von `send` je Aktivierung des Schreibers.
            let maxpt = self.max_sends(StreamRef::Internal(StreamId(i as u32)));
            self.check_maxpt(&name, span, maxpt, &periods, Caps { cap, cap_bytes, elem_bytes });
        }
    }

    /// Pruefung 17: `MAXPT * n_m <= CAP` je Konsument, dazu die Byte-Variante
    /// aus Lemma 9.6.1.
    fn check_maxpt(&mut self, name: &str, span: Span, maxpt: u64, periods: &[u32], caps: Caps) {
        let Caps { cap, cap_bytes, elem_bytes } = caps;
        for n_m in periods {
            let need = maxpt.saturating_mul(u64::from(*n_m));
            if need > cap {
                self.error_hint(
                    SC17,
                    span,
                    format!("Stream `{name}`: {need} Elemente je Aktivierung, `capacity` ist {cap}"),
                    format!("`with capacity = {need}` setzen oder die Periode des Konsumenten senken (8.6)"),
                );
                return;
            }
            let need_bytes = need.saturating_mul(elem_bytes);
            if need_bytes > cap_bytes {
                self.error_hint(
                    SC17,
                    span,
                    format!("Stream `{name}`: {need_bytes} Byte je Aktivierung, `capacity_bytes` ist {cap_bytes}"),
                    format!("`with capacity_bytes = {need_bytes}` setzen (8.6, Lemma 9.6.1)"),
                );
                return;
            }
        }
    }

    /// Perioden der Konsumenten je Stream, aus `Layout::cursors`.
    fn stream_readers(&self) -> HashMap<StreamRef, Vec<u32>> {
        let mut out: HashMap<StreamRef, Vec<u32>> = HashMap::new();
        for m in &self.program.machines {
            if matches!(m.kind, MachineKind::Template) || m.states.is_empty() {
                continue;
            }
            for r in &m.layout.cursors {
                out.entry(*r).or_default().push(m.period.max(1));
            }
        }
        out
    }

    /// Statische Hoechstzahl von `send` auf einen Stream je Aktivierung des
    /// Schreibers (8.6, Pruefung 43).
    fn max_sends(&self, target: StreamRef) -> u64 {
        let count = |stmts: &[Stmt]| {
            let mut n = 0u64;
            walk_stmts(stmts, 0, &mut |s, _| {
                if let StmtKind::Send { stream, .. } = &s.kind
                    && *stream == target
                {
                    n += 1;
                }
            });
            n
        };
        let mut worst = 0u64;
        for m in &self.program.machines {
            if matches!(m.kind, MachineKind::Template) {
                continue;
            }
            let mut n = 0u64;
            for_each_block(m, &mut |b| n += count(&b.stmts));
            // 6.2: Eine Sequenz laeuft je Tick ein Segment; ihre `send`
            // zaehlen je Segment, nicht in der Summe (FB-186).
            for s in &m.states {
                if let Some(seq) = &s.sequence {
                    n += segment_sends(&seq.items, &count);
                }
            }
            worst = worst.max(n);
        }
        worst.max(1)
    }

    /// Pruefungen 40 und 41 (3.4, 4.2): zwei Hinweise, die nur auf schmalen
    /// Kernen etwas kosten. Der Kostenanteil, den die Referenz nennt, kommt
    /// mit der kalibrierten Tabelle (13.8); bis dahin nennt der Lint die
    /// Stelle und den Ausweg.
    fn performance_lints(&mut self) {
        // 12.8: `baremetal` und `boot` laufen auf MCUs. `linux_rt` und
        // `rtos` sagen ueber die Breite nichts, also schweigt der Lint dort.
        let narrow_core =
            matches!(self.program.config.runtime_profile(), Some(RuntimeProfile::Baremetal | RuntimeProfile::Boot));
        if !narrow_core {
            return;
        }
        let mut diags = Vec::new();

        // 41: eine programmweite Entscheidung, also eine Meldung.
        if self.program.config.float_width == takt_mir::types::FloatWidth::F64 {
            // Eine programmweite Entscheidung hat keine Stelle im Text.
            let span = takt_diag::Span::default();
            diags.push(
                Diagnostic::warning(SC41, span, "`float = f64` auf einem Kern, der f64 in Software rechnet")
                    .with_suggestion("`system: float = f32` erwaegen; f64 kostet dort zwei Groessenordnungen (4.2)"),
            );
        }

        // 40: `int` ohne Range in einer Schleife bleibt 64 Bit (3.4).
        for m in &self.program.machines {
            if matches!(m.kind, MachineKind::Template) {
                continue;
            }
            for_each_stmt_ctx(m, &mut |s, depth| {
                if depth == 0 {
                    return;
                }
                let StmtKind::Assign { target: Place::Var(v), .. } = &s.kind else { return };
                let Some(def) = m.vars.get(v.index()) else { return };
                if !matches!(self.program.types.list.get(def.ty.index()), Some(Type::Int { width, range: None, .. }) if width.bits() == 64)
                {
                    return;
                }
                let name = def.name.clone();
                diags.push(
                    Diagnostic::warning(
                        SC40,
                        s.span,
                        format!("`{name}` hat keine Range und bleibt in einer Schleife 64 Bit"),
                    )
                    .with_suggestion("mit `in a..b` deklarieren; der Compiler rechnet dann in 32 Bit (3.4)"),
                );
            });
        }
        self.diags.extend(diags);
    }

    /// Pruefung 61 (9.4.5): `check c, "…" within d` haelt seine Zusage.
    ///
    /// Verglichen wird in **Ticks**, nicht in Nanosekunden: `within 5 ms`
    /// bei `T0 = 1 ms` heisst „in hoechstens fuenf Ticks". Die Tickzahl
    /// folgt aus Periode, Bestaetigungszeit und Fault-Wald und ist exakt;
    /// die Umrechnung in Zeit setzt voraus, dass jeder Tick eingehalten
    /// wird, und das prueft erst die Schedulability (7.2). Die Regel
    /// gewinnt dadurch spaeter an Aussage, ohne sich zu aendern.
    ///
    /// Die Meldung nennt die Aufschluesselung, weil die Zahl sonst nicht
    /// zu verbessern ist: Wer nur „zu langsam" liest, weiss nicht, ob die
    /// Periode, die Bestaetigungszeit oder der Fault-Wald schuld ist.
    fn check_latency(&mut self) {
        let lat = takt_mir::analysis::latency::latency(&self.program);
        let tick = self.program.config.tick;
        if tick <= 0 {
            return;
        }
        let mut diags = Vec::new();
        for site in &lat.sites {
            let Some(want_ns) = site.within else { continue };
            // Aufrunden: Eine Forderung von 2,5 Ticks ist mit zwei Ticks
            // erfuellt — der dritte laeuft erst nach der Frist an.
            let want_ticks = want_ns / tick;
            let have = site.ticks() + lat.commit;
            if i128::from(have) > i128::from(want_ticks) {
                let m = &self.program.machines[site.machine.index()];
                diags.push(
                    Diagnostic::error(
                        SC61,
                        site.span,
                        format!(
                            "`within {}` nicht eingehalten: {} Ticks statt {want_ticks} \
                             ({} erkennen + {} bestaetigen + {} Fault-Pfad + {} Commit)",
                            takt_mir::dump::duration(want_ns),
                            have,
                            site.detect,
                            site.confirm,
                            site.fault,
                            lat.commit,
                        ),
                    )
                    .with_suggestion(format!(
                        "Periode von `{}` senken, Bestaetigungszeit kuerzen oder den Fault-Wald flacher machen (5.3)",
                        m.name
                    )),
                );
            }
        }
        self.diags.extend(diags);
    }

    /// Pruefung 12 (9.4.3, 7.2): Kostenbudget, Schedulability, Stack.
    ///
    /// **Sie sagt, warum sie nicht urteilen kann, statt zu schweigen.**
    /// Die Pruefung hat drei Teile, und alle drei brauchen dieselbe
    /// Eingabe: `c_target`, die kalibrierte Kostentabelle aus 13.8. Ohne
    /// sie sind `B_m` und `F_m` Operationszahlen und keine Zeiten, und
    /// eine Schedulability ohne Zeiten ist keine.
    ///
    /// Bis dahin schwieg sie ganz — und ein Nutzer konnte nicht wissen, ob
    /// sein Programm geprueft wurde oder ob es nichts zu beanstanden gab.
    /// Das ist der schlechtere von zwei Zustaenden: `wcet` im Budget wird
    /// seit je mit Stufenhinweis abgelehnt, und genau dieses Muster ist
    /// hier richtig (FB-136).
    ///
    /// Gemeldet wird als Hinweis, nicht als Warnung: Ein Programm ohne
    /// Kalibrierung ist nicht fehlerhaft, es ist ungemessen.
    fn check_cost_budget(&mut self) {
        // Nur wo jemand ein Budget deklariert hat: Wer keines nennt, hat
        // nichts erwartet, und ein Hinweis auf eine fehlende Pruefung
        // waere dort Rauschen.
        // Nur wo ein `wcet` deklariert ist: `ram` prueft SC-62 ohne
        // Kalibrierung, und ein Hinweis dort waere falsch.
        let spans: Vec<(Span, String)> = self
            .program
            .machines
            .iter()
            .filter_map(|m| m.declared_budget.filter(|b| b.wcet_ns.is_some()).map(|b| (b.span, m.name.clone())))
            .collect();
        for (span, name) in spans {
            self.diags.push(
                Diagnostic::new(
                    takt_diag::Severity::Note,
                    SC12,
                    span,
                    format!("`{name}`: Kostenbudget und Schedulability sind noch nicht entscheidbar"),
                )
                .with_suggestion(
                    "Die Umrechnung von Operationen in Zeit braucht die kalibrierte Kostentabelle `c_target` \n                     (9.4.3, 13.8); `takt cost` zeigt die gerechneten Vektoren schon heute"
                        .to_string(),
                ),
            );
        }
    }

    /// Pruefung 22 (5.10): die statischen Regeln fuer `idle`.
    ///
    /// Sie sichern Satz 9.9.1 — in einem `idle`-Zustand ist der Schritt die
    /// Identitaet, und nur darum darf die Runtime Ticks ueberspringen.
    fn check_idle_states(&mut self) {
        let mut diags = Vec::new();
        for m in &self.program.machines {
            if m.kind == MachineKind::Template {
                continue;
            }
            for (i, s) in m.states.iter().enumerate() {
                if !s.idle {
                    continue;
                }
                let id = StateId(i as u32);
                // Zustand, Vorfahren und Kinder: Ein `loop:` irgendwo in der
                // Kette macht den Schritt nicht-leer.
                for other in idle_scope(m, id) {
                    let o = &m.states[other.index()];
                    let was = if !o.loop_block.stmts.is_empty() {
                        "`loop:`"
                    } else if !o.handlers.is_empty() {
                        "einen Handler"
                    } else if o.sequence.is_some() {
                        "eine Sequenz"
                    } else if !o.instances.is_empty() {
                        "gescopte Instanzen"
                    } else {
                        continue;
                    };
                    let site = if other == id { String::new() } else { format!(" ueber `{}`", o.name) };
                    diags.push(
                        Diagnostic::error(SC22, s.span, format!("`idle`-Zustand `{}` hat{site} {was}", s.name))
                            .with_suggestion(
                                "In `idle` ist der Schritt die Identitaet (Satz 9.9.1); was rechnet, gehoert in \
                             einen Betriebszustand"
                                    .to_string(),
                            ),
                    );
                }
                for t in &s.transitions {
                    if let Some(bad) = non_wake_read(&self.program, t) {
                        diags.push(
                            Diagnostic::error(
                                SC22,
                                t.span,
                                format!("Guard in `{}` liest `{bad}`, was im Schlaf nicht weckt", s.name),
                            )
                            .with_suggestion(
                                "`with wake = true` am Channel oder Command deklarieren, oder den Uebergang \
                                 nach `after` verlegen (5.10)"
                                    .to_string(),
                            ),
                        );
                    }
                }
            }
        }
        self.diags.extend(diags);
    }

    /// Pruefung 54 (5.12): `resume` nur an zusammengesetzten Zustaenden mit
    /// `initial` und nicht an `idle`.
    fn check_resume_states(&mut self) {
        let mut diags = Vec::new();
        for m in self.program.machines.iter().filter(|m| m.kind != MachineKind::Template) {
            for s in m.states.iter().filter(|s| s.resume) {
                if s.initial.is_none() {
                    diags.push(
                        Diagnostic::error(
                            SC54,
                            s.span,
                            format!("`resume` an `{}`, das keine Kindzustaende hat", s.name),
                        )
                        .with_suggestion(
                            "`resume` merkt sich den zuletzt aktiven Kindpfad; ohne `initial` gibt es keinen (5.12)"
                                .to_string(),
                        ),
                    );
                }
                if s.idle {
                    diags.push(
                        Diagnostic::error(SC54, s.span, format!("`{}` ist `idle` und `resume` zugleich", s.name))
                            .with_suggestion(
                                "Ein `idle`-Zustand hat keine Kinder, die aktiv waeren (5.10, 5.12)".to_string(),
                            ),
                    );
                }
            }
        }
        self.diags.extend(diags);
    }
    /// Pruefung 55 (7.5): Trigger sind Knotenregeln.
    ///
    /// Die Form des Guards — ein Muster ueber genau einem Strom — steht
    /// schon in `trigger_decl`; ohne sie entstuende kein Trigger. Hier
    /// bleiben die Regeln ueber dem fertigen Knoten: `when` liest nur das
    /// Ereignis, `then` schreibt nur Outputs, `d >= bound > 0`, und
    /// `arm` kommt aus genau einer Maschine.
    fn check_triggers(&mut self) {
        let mut diags = Vec::new();
        for t in &self.program.triggers {
            if t.bound <= 0 {
                diags.push(Diagnostic::error(
                    SC55,
                    t.span,
                    format!("`bound` von `{}` muss positiv sein (7.5)", t.name),
                ));
            }
            // 7.5: `d >= bound`, damit die Ausgabe nie in der Vergangenheit
            // liegt. `d` ist der Abstand zu `event.t`; nur eine konstante
            // Differenz ist statisch entscheidbar.
            if let Some(d) = delay_of(&t.time) {
                if d < t.bound {
                    diags.push(
                        Diagnostic::error(
                            SC55,
                            t.span,
                            format!(
                                "`{}` plant {} nach dem Ereignis, sagt aber {} zu (7.5)",
                                t.name,
                                takt_mir::dump::duration(d),
                                takt_mir::dump::duration(t.bound)
                            ),
                        )
                        .with_suggestion("`d >= bound`: fruehestens nach der zugesagten Reaktionszeit".to_string()),
                    );
                }
            }
            // 7.5: `then` schreibt nur Outputs — keine Variablen, kein
            // Zustand. Der Trigger hat keinen.
            for s in &t.then.stmts {
                if !matches!(s.kind, StmtKind::Assign { target: Place::Output(_), .. }) {
                    diags.push(
                        Diagnostic::error(SC55, s.span, format!("`then` von `{}` schreibt mehr als Outputs", t.name))
                            .with_suggestion(
                            "ein Trigger hat keinen Zustand ausser `armed` (7.5); Variablen gehoeren in die Maschine"
                                .to_string(),
                        ),
                    );
                }
            }
            // 7.5: `when` liest nur das Ereignis, Konstanten und Parameter.
            if let takt_mir::machine::Guard::Match {
                pattern: takt_mir::pattern::Pattern::Record { fields, .. }, ..
            } = &t.guard
            {
                if fields.iter().any(|(_, e)| reads_state(e)) {
                    diags.push(
                        Diagnostic::error(SC55, t.span, format!("`when` von `{}` liest Zustand (7.5)", t.name))
                            .with_suggestion("knotenlokal heisst: das Ereignis, Konstanten und Parameter".to_string()),
                    );
                }
            }
        }
        // 7.5: `armed` ist Zustand *einer* Maschine.
        let mut armers: HashMap<TriggerId, Vec<(String, Span)>> = HashMap::new();
        for m in &self.program.machines {
            if matches!(m.kind, MachineKind::Template) {
                continue;
            }
            for_each_stmt(m, &mut |s| {
                if let StmtKind::Arm { trigger, .. } = &s.kind {
                    let list = armers.entry(*trigger).or_default();
                    if !list.iter().any(|(n, _)| *n == m.name) {
                        list.push((m.name.clone(), s.span));
                    }
                }
            });
        }
        for (id, list) in &armers {
            if list.len() > 1 {
                let name = self.program.triggers[id.index()].name.clone();
                diags.push(
                    Diagnostic::error(SC55, list[1].1, format!("`{name}` wird aus mehreren Maschinen armiert (7.5)"))
                        .with_note(list[0].1, format!("auch in `{}`", list[0].0))
                        .with_suggestion("`armed` ist Zustand genau einer Maschine".to_string()),
                );
            }
        }
        self.diags.extend(diags);
    }
    /// Pruefung 23 (5.9): `persist var` — POD-Typ, Maschinenebene, nicht in
    /// Szenarien, Typ-Hash eindeutig.
    ///
    /// Der Hash ist per Konstruktion stabil (er faellt aus Name und
    /// Typstruktur, [`takt_mir::persist::type_hash`]); pruefbar ist seine
    /// Eindeutigkeit im Programm. Zwei Variablen mit demselben Schluessel
    /// laesen einander still.
    ///
    /// Die Kollisionspruefung hat absichtlich keinen Korpusfall: Gleiche
    /// Namen verbietet schon die Namensaufloesung, also braeuchte es eine
    /// SHA-256-Kollision. Sie steht hier als Netz fuer einen kuenftigen
    /// Fehler im Hash, nicht fuer ein erreichbares Programm.
    fn check_persist(&mut self) {
        let mut diags = Vec::new();
        let mut non_pod = Vec::new();
        let mut seen: HashMap<u64, String> = HashMap::new();
        for m in &self.program.machines {
            if m.kind == MachineKind::Template {
                continue;
            }
            for pv in &m.persist {
                let v = &m.vars[pv.var.index()];
                // Noch nicht erreichbar: Szenarien sind selbst v1.1-gestuft
                // (SC-26), und die Stufenmeldung stoppt vor den MIR-Pruefungen.
                if m.kind == MachineKind::Scenario {
                    diags.push(
                        Diagnostic::error(SC23, v.span, format!("`persist var {}` in einem Szenario", v.name))
                            .with_suggestion(
                                "Szenarien beschreiben einen Lauf, nicht das Geraet; `persist` gehoert in die \
                                 Maschine (5.9)"
                                    .to_string(),
                            ),
                    );
                    continue;
                }
                if !takt_mir::persist::is_pod(&self.program, v.ty) {
                    non_pod.push((v.ty, v.span, v.name.clone()));
                }
                let key = format!("{}.{}", m.name, v.name);
                if let Some(other) = seen.insert(pv.type_hash, key.clone()) {
                    diags.push(Diagnostic::error(
                        SC23,
                        v.span,
                        format!("`{key}` und `{other}` teilen denselben persist-Schluessel"),
                    ));
                }
            }
        }
        for (ty, span, name) in non_pod {
            let what = self.type_name(ty);
            diags.push(
                Diagnostic::error(SC23, span, format!("`persist var {name}` hat den Nicht-POD-Typ {what}"))
                    .with_suggestion(
                        "Erlaubt sind Skalare, Records, Arrays und Enums; Streams, Bloecke und Optionale haben \
                         keine stabile Byte-Form (5.9)"
                            .to_string(),
                    ),
            );
        }
        self.diags.extend(diags);
    }

    /// Pruefung 62 (7.2): `with budget = {ram = …}` wird eingehalten.
    ///
    /// Ohne Deklaration prueft erst die Integration, ob die Summe passt —
    /// in einem Projekt mit mehreren Teams faellt die Ueberschreitung dann
    /// auf, wenn sie teuer ist. Die Deklaration macht daraus einen lokalen,
    /// sofortigen Fehler und damit einen Vertrag zwischen Teams.
    fn check_declared_budget(&mut self) {
        let mut diags = Vec::new();
        for m in &self.program.machines {
            let Some(b) = m.declared_budget else { continue };
            let Some(want) = b.ram else { continue };
            let have = takt_mir::analysis::size::machine_bytes(&self.program, m);
            if have > want {
                diags.push(
                    Diagnostic::error(
                        SC62,
                        b.span,
                        format!("`{}` braucht {have} Byte, deklariert sind {want}", m.name),
                    )
                    .with_suggestion(
                        "Budget anheben, Variablen verkleinern oder Zustaende zusammenlegen (das Overlay teilt \
                         den Speicher exklusiver Zustaende, 11.2)"
                            .to_string(),
                    ),
                );
            }
        }
        self.diags.extend(diags);
    }

    /// Pruefung 63, zweiter Teil (4.6): Ein `profile` nennt jeden `param`.
    ///
    /// Ein fehlender Parameter nimmt still seinen Default. Das ist
    /// semantisch richtig, aber beim Lesen nicht von der Absicht zu
    /// unterscheiden: Ein Profil, das einen Parameter vergisst, sieht aus
    /// wie eines, das den Default will. Die Warnung zwingt zu nichts — wer
    /// den Default meint, schreibt ihn hin und hat es dokumentiert.
    ///
    /// Zur Uebersetzungszeit, nicht zur Laufzeit: Es ist eine Eigenschaft
    /// des Profils, und im Lauf waere die Meldung zu spaet.
    fn check_profile_completeness(&mut self) {
        let mut diags = Vec::new();
        for pr in &self.program.profiles {
            let named: Vec<u32> = pr.assignments.iter().map(|(id, _)| id.0).collect();
            let missing: Vec<&str> = self
                .program
                .params
                .iter()
                .enumerate()
                .filter(|(i, _)| !named.contains(&(*i as u32)))
                .map(|(_, p)| p.name.as_str())
                .collect();
            if missing.is_empty() {
                continue;
            }
            diags.push(
                Diagnostic::warning(
                    SC63,
                    pr.span,
                    format!(
                        "Profil `{}` nennt {} von {} Parametern nicht",
                        pr.name,
                        missing.len(),
                        self.program.params.len()
                    ),
                )
                .with_suggestion(format!(
                    "sie nehmen ihren Default; ausdruecklich setzen macht die Absicht sichtbar ({})",
                    missing.join(", ")
                )),
            );
        }
        self.diags.extend(diags);
    }

    /// Pruefung 63, erster Teil (5.6): Ein `alert` nennt das zu *meldende
    /// Ereignis*, ein `check` die *einzuhaltende Invariante* — die
    /// Polaritaet ist entgegengesetzt, und genau deshalb wird sie
    /// verwechselt.
    ///
    /// Gemeldet wird der Fall, der sich beweisen laesst: Dieselbe Groesse,
    /// dieselbe Schranke, dieselbe Vergleichsrichtung in einem `check` und
    /// einem `alert` desselben Blocks. Die Referenz nennt beide
    /// ausdruecklich nebeneinander im selben `loop:` (14.1) — dort faellt
    /// der Fehler sonst niemandem auf, weil beide Zeilen gleich aussehen.
    fn check_alert_polarity(&mut self) {
        let mut diags = Vec::new();
        for m in &self.program.machines {
            if matches!(m.kind, MachineKind::Template) {
                continue;
            }
            let states = m.states.iter().flat_map(|s| [&s.enter, &s.loop_block, &s.exit]);
            for b in std::iter::once(&m.loop_block).chain(states) {
                Self::alert_polarity_in(b, &mut diags);
            }
        }
        self.diags.extend(diags);
    }

    /// Vergleicht die `check`- und `alert`-Bedingungen eines Blocks.
    fn alert_polarity_in(b: &takt_mir::stmt::Block, diags: &mut Vec<Diagnostic>) {
        let mut checks: Vec<(BinaryOp, &Expr, &Expr)> = Vec::new();
        for s in &b.stmts {
            if let StmtKind::Check { cond, kind: takt_mir::stmt::CheckKind::Check, .. } = &s.kind {
                if let Some(c) = comparison(cond) {
                    checks.push(c);
                }
            }
        }
        if checks.is_empty() {
            return;
        }
        for s in &b.stmts {
            let StmtKind::Observe(takt_mir::stmt::Observe::Alert { cond, .. }) = &s.kind else { continue };
            let Some((op, lhs, rhs)) = comparison(cond) else { continue };
            if !checks.iter().any(|(o, l, r)| *o == op && l.same_as(lhs) && r.same_as(rhs)) {
                continue;
            }
            diags.push(
                Diagnostic::warning(
                    SC63,
                    s.span,
                    "`alert` und `check` haben dieselbe Bedingung; die Polaritaet ist entgegengesetzt gemeint (5.6)",
                )
                .with_suggestion(
                    "ein `check` nennt die einzuhaltende Invariante, ein `alert` das zu meldende Ereignis — \
                     der Vergleich des Alerts gehoert vermutlich umgedreht"
                        .to_string(),
                ),
            );
        }
    }

    /// Pruefung 60 fuer das eingebaute Geraet `sys` (12.7, 7.4): Adresse,
    /// Richtung und Typ der System-Channels kennt der Compiler selbst; eine
    /// `sim`-Bindung darf nur einen `sys`-Input speisen (8.3).
    fn check_sys_channels(&mut self) {
        let mut diags = Vec::new();
        for c in &self.program.channels {
            let (address, simulated) = match &c.binding {
                Binding::Hw(a) => (a.text(), false),
                Binding::Sim(a) => (a.text(), true),
                Binding::None => continue,
            };
            if !sys::is_sys(&address) {
                continue;
            }
            let Some(entry) = sys::channel(&address) else {
                let known: Vec<&str> = sys::SYS.iter().map(|s| s.address).collect();
                diags.push(Diagnostic::error(
                    SC60,
                    c.span,
                    format!("das Geraet `sys` kennt `{address}` nicht (12.7); Kanaele: {}", known.join(", ")),
                ));
                continue;
            };
            let dir_ok = if simulated {
                c.dir == Direction::Output && entry.dir == Direction::Input
            } else {
                c.dir == entry.dir
            };
            if !dir_ok {
                let side = if entry.dir == Direction::Input { "ein Input" } else { "ein Output" };
                let message = if simulated {
                    format!(
                        "`{}`: eine `sim`-Bindung speist einen Input, `{address}` ist am Geraet `sys` {side} (8.3, 12.7)",
                        c.name
                    )
                } else {
                    format!("`{}`: `{address}` ist am Geraet `sys` {side} (12.7)", c.name)
                };
                diags.push(Diagnostic::error(SC60, c.span, message));
                continue;
            }
            if !self.fits_sys(c.ty, entry.ty) {
                diags.push(Diagnostic::error(
                    SC60,
                    c.span,
                    format!(
                        "`{}` hat Typ `{}`, `{address}` verlangt `{}` (12.7)",
                        c.name,
                        self.type_name(c.ty),
                        entry.ty.name()
                    ),
                ));
            }
        }
        self.diags.extend(diags);
    }

    fn fits_sys(&self, ty: TypeId, want: SysType) -> bool {
        match (want, self.ty(ty)) {
            (SysType::Enum(n), Type::Enum(e)) => self.program.enums[e.index()].name == n,
            (SysType::Record(n), Type::Record(r)) => self.program.records[r.index()].name == n,
            (SysType::Int, Type::Int { .. }) => true,
            (SysType::U8, Type::Int { width: takt_mir::types::IntWidth::U8, .. }) => true,
            (SysType::Bool, Type::Bool) => true,
            (SysType::BoolArray(n), Type::Array { elem, len }) => *len == n && matches!(self.ty(*elem), Type::Bool),
            (SysType::Duration, Type::Duration { .. }) => true,
            _ => false,
        }
    }

    /// Pruefung 48 (12.7): Ein `irreversible`-Output wird nur in einer
    /// Sequenz geschrieben, und dort unmittelbar nach einem `expect`. Es gibt
    /// keinen Rueckweg — die Voraussetzung muss sichtbar geprueft sein.
    fn check_irreversible(&mut self) {
        let marked: HashSet<ChannelId> = self
            .program
            .channels
            .iter()
            .enumerate()
            .filter(|(_, c)| c.attrs.irreversible)
            .map(|(i, _)| ChannelId(i as u32))
            .collect();
        if marked.is_empty() {
            return;
        }
        let mut diags = Vec::new();
        for m in &self.program.machines {
            if matches!(m.kind, MachineKind::Template) {
                continue;
            }
            // Erlaubt ist nur der Platz unmittelbar hinter einem `expect`.
            let mut allowed: HashSet<(u32, u32)> = HashSet::new();
            for s in &m.states {
                if let Some(seq) = &s.sequence {
                    mark_after_expect(&seq.items, &mut allowed);
                }
            }
            for_each_stmt(m, &mut |s| {
                let StmtKind::Assign { target, .. } = &s.kind else { return };
                let Some(c) = output_of(target) else { return };
                if !marked.contains(&c) {
                    return;
                }
                if allowed.contains(&(s.span.file.0, s.span.start)) {
                    return;
                }
                let name = self.program.channels[c.index()].name.clone();
                diags.push(
                    Diagnostic::error(SC48, s.span, format!("`{name}` ist irreversibel und braucht ein `expect`"))
                        .with_suggestion("die Zuweisung steht in einer Sequenz unmittelbar nach `expect` (12.7)"),
                );
            });
        }
        self.diags.extend(diags);
    }

    /// Pruefung 20 (8.8): „statische Summe der Hoechstlaengen je Aktivierung
    /// <= `capacity`". Gerechnet wird je Maschine ueber alle erreichbaren
    /// `send`, weil der Sendepuffer erst beim Commit geleert wird.
    fn check_send_budget(&mut self) {
        let mut diags = Vec::new();
        for (i, c) in self.program.channels.iter().enumerate() {
            let id = ChannelId(i as u32);
            if !matches!(self.program.types.list.get(c.ty.index()), Some(Type::Stream(_))) {
                continue;
            }
            if c.dir != Direction::Output {
                continue;
            }
            let cap = u64::from(c.attrs.capacity.unwrap_or(256));
            for m in &self.program.machines {
                if matches!(m.kind, MachineKind::Template) {
                    continue;
                }
                let mut sum = 0u64;
                let mut site = None;
                for_each_stmt(m, &mut |s| {
                    if let StmtKind::Send { stream: StreamRef::Channel(t), len_max, .. } = &s.kind {
                        if *t == id {
                            sum += u64::from(*len_max);
                            site.get_or_insert(s.span);
                        }
                    }
                });
                if sum > cap {
                    let span = site.unwrap_or(c.span);
                    diags.push(
                        Diagnostic::error(
                            SC20,
                            span,
                            format!("`{}`: {sum} Byte je Aktivierung, `capacity` ist {cap}", c.name),
                        )
                        .with_suggestion(format!("`with capacity = {sum}` setzen oder weniger senden (8.8)")),
                    );
                }
            }
        }
        self.diags.extend(diags);
    }

    /// `max_rate` eines Channels in Hz, als ganze Zahl.
    fn rate_hz(&self, index: usize) -> Option<u64> {
        let e = self.program.channels[index].attrs.max_rate.as_ref()?;
        match &e.kind {
            ExprKind::Int(n) => u64::try_from(*n).ok(),
            ExprKind::Float(f) if *f >= 0.0 => Some(*f as u64),
            _ => None,
        }
    }

    /// Bytelast eines Elementtyps: die deklarierte Hoechstlaenge (8.6).
    fn elem_bytes(&self, ty: TypeId) -> Option<u32> {
        match self.program.types.list.get(ty.index())? {
            Type::Bytes { cap } | Type::Line { cap } | Type::Str { cap } => Some(*cap),
            Type::Int { width, .. } => Some(width.bits() / 8),
            Type::Record(r) => self.program.records[r.index()].wire_size.or(Some(1)),
            // 8.9: Kopf plus `N` Abtastwerte.
            Type::Capture { .. } => takt_mir::bytes::max_size(&self.program, ty).ok().or(Some(1)),
            _ => Some(1),
        }
    }

    /// Pruefung 7 und 15 (Teil): Besitzer je Output, `pub var` nur vom
    /// Besitzer, Inputs nie Ziel.
    fn check_writers(&mut self) {
        let mut writers: HashMap<ChannelId, Vec<(MachineId, Span)>> = HashMap::new();
        for (mi, m) in self.program.machines.iter().enumerate() {
            if matches!(m.kind, MachineKind::Template) {
                continue;
            }
            let mut seen: HashSet<ChannelId> = HashSet::new();
            for_each_stmt(m, &mut |s| {
                // `send o, e` schreibt einen Ausgabestrom genauso wie eine
                // Zuweisung ein Latch (8.8); auch er hat genau einen Besitzer.
                let target = match &s.kind {
                    StmtKind::Assign { target, .. } => output_of(target),
                    StmtKind::Send { stream: StreamRef::Channel(c), .. } => Some(*c),
                    // `o = b.push(x)` und `o = inst.step(x)` schreiben `o`
                    // genauso wie eine gewoehnliche Zuweisung (5.7, 3.9).
                    StmtKind::MethodCall { target: Some(t), .. } => output_of(t),
                    _ => None,
                };
                if let Some(c) = target {
                    if seen.insert(c) {
                        writers.entry(c).or_default().push((MachineId(mi as u32), s.span));
                    }
                }
            });
        }
        // 7.5: Die Outputs im `then` eines Triggers gehoeren seiner
        // armierenden Maschine — der Trigger handelt fuer sie, und
        // Single-Writer bleibt eine Aussage ueber Maschinen.
        for t in &self.program.triggers {
            let Some(owner) = t.owner else { continue };
            for s in &t.then.stmts {
                if let StmtKind::Assign { target: Place::Output(c), .. } = &s.kind {
                    let list = writers.entry(*c).or_default();
                    if !list.iter().any(|(m, _)| *m == owner) {
                        list.push((owner, s.span));
                    }
                }
            }
        }
        let mut diags = Vec::new();
        let is_scenario = |m: MachineId| self.program.machines[m.index()].kind == MachineKind::Scenario;
        for (c, list) in &writers {
            // 13.6: Szenarien laufen je einzeln; zwei Szenarien duerfen
            // denselben `sim`-Output stellen, ein Szenario und ein Modell nicht.
            let scenarios = list.iter().filter(|(m, _)| is_scenario(*m)).count();
            if list.len() > 1 && scenarios < list.len() {
                let (second, first) = if scenarios > 0 && !is_scenario(list[1].0) && is_scenario(list[0].0) {
                    (&list[0], &list[1])
                } else {
                    (&list[1], &list[0])
                };
                let name = self.program.channels[c.index()].name.clone();
                let code = if scenarios > 0 { SC26 } else { SC7 };
                let other = &self.program.machines[first.0.index()].name;
                let mut d = Diagnostic::error(code, second.1, format!("Output `{name}` hat mehrere Schreiber"))
                    .with_note(first.1, format!("auch in `{other}` geschrieben"))
                    .with_suggestion("jeder Output gehoert genau einer Maschine (1.4); ein Szenario stellt nur, was kein Modell stellt (13.6)");
                d.span = second.1;
                diags.push(d);
            }
        }
        for (c, list) in writers {
            // Der Besitzer ist die Maschine, nicht das Szenario — es laeuft
            // nur in seinem eigenen Lauf.
            let owner = list.iter().find(|(m, _)| !is_scenario(*m)).unwrap_or(&list[0]).0;
            self.program.channels[c.index()].owner = Some(owner);
        }
        self.diags.extend(diags);
    }

    /// Pruefung 26 (13.6): ein Szenario schreibt nur `sim`-Outputs.
    fn check_scenarios(&mut self) {
        let mut diags = Vec::new();
        for m in &self.program.machines {
            if m.kind != MachineKind::Scenario {
                continue;
            }
            for_each_stmt(m, &mut |s| {
                let target = match &s.kind {
                    StmtKind::Assign { target, .. } => output_of(target),
                    StmtKind::Send { stream: StreamRef::Channel(c), .. } => Some(*c),
                    StmtKind::MethodCall { target: Some(t), .. } => output_of(t),
                    _ => None,
                };
                let Some(c) = target else { return };
                let channel = &self.program.channels[c.index()];
                if !matches!(channel.binding, Binding::Sim(_)) {
                    diags.push(
                        Diagnostic::error(
                            SC26,
                            s.span,
                            format!("Szenario `{}` schreibt `{}`, keinen `sim`-Output", m.name, channel.name),
                        )
                        .with_suggestion("ein Szenario stellt die Umgebung, nicht die Anlage (13.6)".to_string()),
                    );
                }
            });
        }
        self.diags.extend(diags);
    }

    /// Pruefung 9: Fault-Wald azyklisch, `φ(s) ≠ s`, `FAULTED` erreichbar.
    fn check_fault_forest(&mut self) {
        let mut diags = Vec::new();
        for m in &self.program.machines {
            if matches!(m.kind, MachineKind::Template) || m.states.is_empty() {
                continue;
            }
            // 5.3: damit `FAULTED` nie scheitern kann, duerfen die Guards
            // seiner Transitionen keine impliziten Pruefungen enthalten.
            for (i, s) in m.states.iter().enumerate() {
                let id = StateId(i as u32);
                let mut seen = HashSet::new();
                let mut cur = id;
                loop {
                    if !seen.insert(cur) {
                        diags.push(
                            Diagnostic::error(
                                SC9,
                                m.states[cur.index()].span,
                                format!("Fault-Wald hat einen Zyklus ueber `{}`", m.states[cur.index()].name),
                            )
                            .with_suggestion("`fault -> X` so waehlen, dass jeder Pfad bei `FAULTED` endet (5.3)"),
                        );
                        break;
                    }
                    match fault_target_of(m, cur) {
                        FaultTarget::Faulted => break,
                        FaultTarget::State(next) => {
                            if next == cur {
                                diags.push(
                                    Diagnostic::error(
                                        SC9,
                                        m.states[cur.index()].span,
                                        format!("`{}` ist sein eigenes Fault-Ziel", m.states[cur.index()].name),
                                    )
                                    .with_suggestion("φ(s) ≠ s (5.3)"),
                                );
                                break;
                            }
                            cur = next;
                        }
                    }
                }
                let _ = s;
            }
        }
        self.diags.extend(diags);
    }

    /// Pruefung 10: Erreichbarkeit im Uebergangsgraphen, Zustaende ohne Ausgang.
    fn check_reachability(&mut self) {
        let mut diags = Vec::new();
        for m in &self.program.machines {
            if matches!(m.kind, MachineKind::Template) || m.states.is_empty() {
                continue;
            }
            let mut reached: HashSet<StateId> = HashSet::new();
            let mut stack = vec![m.initial];
            // Fault-Ziele sind ebenfalls erreichbar
            for (i, _) in m.states.iter().enumerate() {
                if let FaultTarget::State(t) = fault_target_of(m, StateId(i as u32)) {
                    stack.push(t);
                }
            }
            for t in &m.faulted.transitions {
                if let Target::State(s) = t.target {
                    stack.push(s);
                }
            }
            while let Some(s) = stack.pop() {
                if !reached.insert(s) {
                    continue;
                }
                // Kinder und Eltern gehoeren zur Konfiguration
                if let Some(init) = m.states[s.index()].initial {
                    stack.push(init);
                }
                if let Some(p) = m.states[s.index()].parent {
                    stack.push(p);
                }
                let mut targets = Vec::new();
                collect_targets(m, s, &mut targets);
                stack.extend(targets);
            }
            for (i, s) in m.states.iter().enumerate() {
                let id = StateId(i as u32);
                if !reached.contains(&id) {
                    diags.push(
                        Diagnostic::warning(SC10, s.span, format!("Zustand `{}` ist nicht erreichbar", s.name))
                            .with_suggestion("Uebergang ergaenzen oder Zustand entfernen"),
                    );
                }
                let mut targets = Vec::new();
                collect_targets(m, id, &mut targets);
                let leaf = s.children.is_empty();
                if leaf
                    && targets.is_empty()
                    && s.sequence.is_none()
                    && !s.name.contains("DONE")
                    && !s.name.contains("SAFE")
                {
                    diags.push(
                        Diagnostic::warning(SC10, s.span, format!("Zustand `{}` hat keinen Ausgang", s.name))
                            .with_suggestion("`when`/`after` ergaenzen, wenn er nicht endgueltig ist"),
                    );
                }
            }
        }
        self.diags.extend(diags);
    }

    /// Pruefung 11: Aufrufgraph azyklisch, `step` hoechstens einmal je Instanz
    /// und Aktivierung.
    fn check_termination(&mut self) {
        let mut diags = Vec::new();
        // Aufrufgraph der Funktionen
        let mut edges: Vec<Vec<FnId>> = vec![Vec::new(); self.program.fns.len()];
        for (i, f) in self.program.fns.iter().enumerate() {
            let mut callees = Vec::new();
            for_each_expr_block(&f.body, &mut |e| {
                if let ExprKind::Call { callee, .. } = &e.kind {
                    callees.push(*callee);
                }
            });
            edges[i] = callees;
        }
        let mut state = vec![0u8; edges.len()];
        for i in 0..edges.len() {
            if state[i] == 0 && has_cycle(i, &edges, &mut state) {
                diags.push(
                    Diagnostic::error(
                        SC11,
                        self.program.fns[i].span,
                        format!("Aufrufgraph von `{}` ist zyklisch", self.program.fns[i].name),
                    )
                    .with_suggestion("Rekursion gibt es nicht (9.4.2, T3)"),
                );
            }
        }
        // step je Instanz und Block
        for m in &self.program.machines {
            if matches!(m.kind, MachineKind::Template) {
                continue;
            }
            let mut in_loop = Vec::new();
            for_each_stmt_ctx(m, &mut |s, depth| {
                if let StmtKind::MethodCall { receiver, method: Method::Step, .. } = &s.kind {
                    if depth > 0 {
                        if let Place::Var(v) = receiver {
                            let array = m.layout.block_instances.iter().any(|b| b.var == *v && b.count > 1);
                            if !array {
                                in_loop.push(s.span);
                            }
                        } else {
                            in_loop.push(s.span);
                        }
                    }
                }
            });
            for span in in_loop {
                diags.push(Diagnostic::error(SC11, span, "`step` in einer Schleife".to_string()).with_suggestion(
                    "jede Instanz steppt hoechstens einmal je Tick; Arrays von Instanzen verwenden (5.7)",
                ));
            }
        }
        self.diags.extend(diags);
    }

    /// Pruefung 13: `hw`-Inputs ohne `sim`-Quelle (Festlegung 6: Warnung in
    /// `takt sim`, Fehler ab `takt test`).
    fn check_simulation(&mut self) {
        if self.options.build != crate::Build::Sim {
            return;
        }
        let sims: HashSet<String> = self
            .program
            .channels
            .iter()
            .filter(|c| c.dir == Direction::Output)
            .filter_map(|c| match &c.binding {
                Binding::Sim(a) => Some(address_key(a)),
                _ => None,
            })
            .collect();
        let mut diags = Vec::new();
        for c in &self.program.channels {
            if c.dir != Direction::Input {
                continue;
            }
            if let Binding::Hw(a) = &c.binding {
                if !sims.contains(&address_key(a)) {
                    diags.push(
                        Diagnostic::warning(SC13, c.span, format!("Input `{}` hat keine `sim`-Quelle", c.name))
                            .with_suggestion(format!("`output {}_sim : … @ sim(\"…\")` oder Stimulus (8.3)", c.name)),
                    );
                }
            }
        }
        self.diags.extend(diags);
    }

    /// Pruefung 15: Outputs ohne Schreiber, Inputs ohne Leser.
    fn check_unused(&mut self) {
        let mut read: HashSet<ChannelId> = HashSet::new();
        for m in &self.program.machines {
            for_each_expr_machine(m, &mut |e| {
                if let ExprKind::Input { channel, .. } = &e.kind {
                    read.insert(*channel);
                }
            });
            // Ein Handler nennt seinen Strom nicht als Ausdruck; wer ihn liest,
            // steht in `Layout::cursors` (9.6).
            for r in &m.layout.cursors {
                if let StreamRef::Channel(c) = r {
                    read.insert(*c);
                }
            }
        }
        // 7.5: Ein Trigger liest seinen Quellstrom und schreibt die
        // Outputs seines `then` — beides ohne Maschine.
        for t in &self.program.triggers {
            if let takt_mir::machine::Guard::Match { subject, .. } = &t.guard {
                if let ExprKind::Input { channel, .. } = &subject.kind {
                    read.insert(*channel);
                }
            }
        }
        let mut diags = Vec::new();
        for (i, c) in self.program.channels.iter().enumerate() {
            let id = ChannelId(i as u32);
            match c.dir {
                Direction::Output if c.owner.is_none() && !matches!(c.binding, Binding::Sim(_)) => {
                    diags.push(
                        Diagnostic::warning(SC15, c.span, format!("Output `{}` wird nie geschrieben", c.name))
                            .with_suggestion("Zuweisung ergaenzen oder Channel entfernen"),
                    );
                }
                Direction::Input if !read.contains(&id) => {
                    diags.push(
                        Diagnostic::warning(SC15, c.span, format!("Input `{}` wird nie gelesen", c.name))
                            .with_suggestion("Verwendung ergaenzen oder Channel entfernen"),
                    );
                }
                _ => {}
            }
        }
        self.diags.extend(diags);
    }

    /// Pruefung 25: eine gehobene Variable wird in einem Segment gelesen,
    /// bevor ein frueheres sie zuweist. Pruefung 6 ist die allgemeine Regel
    /// derselben Flussanalyse (10, Zeilen 6 und 25).
    fn check_definite_assignment(&mut self) {
        let mut diags = Vec::new();
        for m in &self.program.machines {
            if matches!(m.kind, MachineKind::Template) {
                continue;
            }
            for s in &m.states {
                let Some(seq) = &s.sequence else { continue };
                let mut assigned: HashSet<VarId> = HashSet::new();
                let mut lifted: HashSet<VarId> = HashSet::new();
                for (i, v) in m.vars.iter().enumerate() {
                    if matches!(v.scope, VarScope::Lifted(x) if m.states[x.index()].name == s.name) && v.init.is_none()
                    {
                        lifted.insert(VarId(i as u32));
                    }
                }
                check_seq_items(&seq.items, &mut assigned, &lifted, m, &mut diags);
            }
        }
        self.diags.extend(diags);
    }
}

/// Eine Anweisung und die in ihr steckenden, in Programmreihenfolge.
///
/// **Lesen und Zuweisen muessen verschraenkt laufen.** Wer erst alle
/// Lesungen einer Anweisung prueft und danach ihre Zuweisungen vormerkt,
/// meldet `if c: var k = 7; y = k` als Fehler — Deklaration und Nutzung
/// stecken dort in *einer* Anweisung, und der Stand „davor" kennt die
/// Zuweisung noch nicht (FB-123).
///
/// Ein bedingter Zweig zaehlt dabei als Zuweisung, obwohl er sie nicht
/// immer haelt. Das ist Absicht: SC-25 prueft die Reihenfolge der
/// Segmente (6.2) — ob ein Wert auf *jedem* Pfad entsteht, ist die Frage
/// der Flussanalyse (3.4) und hat ihre eigene Pruefung.
fn check_stmt(
    s: &Stmt,
    assigned: &mut HashSet<VarId>,
    lifted: &HashSet<VarId>,
    m: &Machine,
    diags: &mut Vec<Diagnostic>,
) {
    // Die Ausdruecke dieser Anweisung selbst — ohne die verschachtelten
    // Bloecke, die danach einzeln drankommen.
    stmt_exprs(s, &mut |e| {
        walk_expr(e, &mut |x| {
            if let ExprKind::Var(v) = &x.kind
                && lifted.contains(v)
                && !assigned.contains(v)
            {
                diags.push(
                    Diagnostic::error(
                        SC25,
                        x.span,
                        format!("`{}` wird gelesen, bevor sie zugewiesen ist", m.vars[v.index()].name),
                    )
                    .with_suggestion("Zuweisung in ein frueheres Segment legen (6.2, 5.8)"),
                );
            }
        });
    });
    if let StmtKind::Assign { target: Place::Var(v), .. } = &s.kind {
        assigned.insert(*v);
    }
    // Dann die Bloecke darunter, jeder in seiner Reihenfolge.
    match &s.kind {
        StmtKind::If { then, otherwise, .. } => {
            for inner in &then.stmts {
                check_stmt(inner, assigned, lifted, m, diags);
            }
            for inner in &otherwise.stmts {
                check_stmt(inner, assigned, lifted, m, diags);
            }
        }
        StmtKind::ForRange { body, .. }
        | StmtKind::ForEach { body, .. }
        | StmtKind::Every { body, .. }
        | StmtKind::At { body, .. } => {
            for inner in &body.stmts {
                check_stmt(inner, assigned, lifted, m, diags);
            }
        }
        StmtKind::Match { arms, .. } => {
            for a in arms {
                for inner in &a.body.stmts {
                    check_stmt(inner, assigned, lifted, m, diags);
                }
            }
        }
        _ => {}
    }
}

fn check_seq_items(
    items: &[SeqItem],
    assigned: &mut HashSet<VarId>,
    lifted: &HashSet<VarId>,
    m: &Machine,
    diags: &mut Vec<Diagnostic>,
) {
    for item in items {
        match item {
            SeqItem::Stmt(s) => check_stmt(s, assigned, lifted, m, diags),
            SeqItem::Repeat { body, counter, .. } => {
                assigned.insert(*counter);
                check_seq_items(body, assigned, lifted, m, diags);
            }
            SeqItem::Step { body, .. } => check_seq_items(body, assigned, lifted, m, diags),
            SeqItem::Until { guard, .. } => {
                if let Guard::Match { binding: Some(b), .. } | Guard::Next { binding: b, .. } = guard {
                    assigned.insert(*b);
                }
            }
            SeqItem::Wait(_) | SeqItem::Expect { .. } => {}
        }
    }
}

fn address_key(a: &takt_mir::pattern::Address) -> String {
    a.segments.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join("/")
}

fn output_of(p: &Place) -> Option<ChannelId> {
    match p {
        Place::Output(c) => Some(*c),
        Place::Field(b, _) | Place::Index(b, _) | Place::Index2(b, _, _) => output_of(b),
        Place::Var(_) | Place::Port(_) => None,
    }
}

/// Ein Lesevorgang einer fremden Groesse (`m.x`, `m.state`, Signal): die
/// Maschine und der Name, wie die Meldung ihn nennt.
fn published_read(p: &Program, e: &Expr) -> Option<(MachineId, String)> {
    match &e.kind {
        ExprKind::Published { machine, var } => {
            let m = &p.machines[machine.machine.index()];
            Some((machine.machine, format!("{}.{}", m.name, m.vars[var.index()].name)))
        }
        ExprKind::StateOf(machine) => {
            Some((machine.machine, format!("{}.state", p.machines[machine.machine.index()].name)))
        }
        ExprKind::Signal { machine, signal } => {
            let m = &p.machines[machine.machine.index()];
            Some((machine.machine, format!("{}.{}", m.name, m.signals[signal.index()].name)))
        }
        _ => None,
    }
}

/// Fault-Ziel φ(s) nach 5.3. Die Rechnung steht in `takt-mir`, weil sie
/// reine MIR-Logik ist und die Latenzanalyse (9.4.5) sie ebenfalls braucht.
pub fn fault_target_of(m: &Machine, s: StateId) -> FaultTarget {
    m.fault_target_of(s)
}

fn collect_targets(m: &Machine, s: StateId, out: &mut Vec<StateId>) {
    let state = &m.states[s.index()];
    for t in &state.transitions {
        if let Target::State(x) = t.target {
            out.push(x);
        }
    }
    let mut push_goto = |b: &Block| {
        for_each_stmt_block(b, &mut |st| {
            if let StmtKind::Goto(Target::State(x)) = &st.kind {
                out.push(*x);
            }
            if let StmtKind::Check { target: Some(Target::State(x)), .. } = &st.kind {
                out.push(*x);
            }
        });
    };
    push_goto(&state.loop_block);
    push_goto(&state.enter);
    if let Some(seq) = &state.sequence {
        collect_seq_targets(&seq.items, out);
    }
    let _ = m;
}

fn collect_seq_targets(items: &[SeqItem], out: &mut Vec<StateId>) {
    for item in items {
        match item {
            SeqItem::Stmt(s) => {
                if let StmtKind::Goto(Target::State(x)) = &s.kind {
                    out.push(*x);
                }
                for_each_stmt_block(&Block::new(vec![s.clone()]), &mut |st| {
                    if let StmtKind::Goto(Target::State(x)) = &st.kind {
                        out.push(*x);
                    }
                });
            }
            SeqItem::Until { timeout: Some(t), .. } => match &t.action {
                TimeoutAction::Goto(Target::State(x)) => out.push(*x),
                TimeoutAction::Else(b) => for_each_stmt_block(b, &mut |st| {
                    if let StmtKind::Goto(Target::State(x)) = &st.kind {
                        out.push(*x);
                    }
                }),
                _ => {}
            },
            SeqItem::Repeat { body, .. } | SeqItem::Step { body, .. } => collect_seq_targets(body, out),
            _ => {}
        }
    }
}

fn has_cycle(i: usize, edges: &[Vec<FnId>], state: &mut [u8]) -> bool {
    state[i] = 1;
    for c in &edges[i] {
        let j = c.index();
        if j >= state.len() {
            continue;
        }
        if state[j] == 1 || (state[j] == 0 && has_cycle(j, edges, state)) {
            return true;
        }
    }
    state[i] = 2;
    false
}

// ------------------------------------------------------------ Durchlaeufe

/// Jede Anweisung einer Maschine (auch geschachtelte Bloecke).
pub fn for_each_stmt(m: &Machine, f: &mut impl FnMut(&Stmt)) {
    for_each_stmt_ctx(m, &mut |s, _| f(s));
}

/// Wie `for_each_stmt`, mit Schleifentiefe.
pub fn for_each_stmt_ctx(m: &Machine, f: &mut impl FnMut(&Stmt, u32)) {
    for_each_block(m, &mut |b| walk_stmts(&b.stmts, 0, f));
    for s in &m.states {
        if let Some(seq) = &s.sequence {
            walk_seq(&seq.items, f);
        }
    }
}

/// Die Bloecke einer Maschine ausserhalb ihrer Sequenzen.
///
/// Ein Handler-Rumpf ist gewoehnlicher Code (8.7): er schreibt Outputs und
/// liest Channels wie jeder andere Block.
pub fn for_each_block(m: &Machine, f: &mut impl FnMut(&Block)) {
    f(&m.loop_block);
    for h in &m.handlers {
        f(&h.body);
    }
    for t in &m.faulted.transitions {
        f(&t.actions);
    }
    for s in &m.states {
        f(&s.enter);
        f(&s.exit);
        f(&s.loop_block);
        for h in &s.handlers {
            f(&h.body);
        }
        for t in &s.transitions {
            f(&t.actions);
        }
    }
}

/// Das Maximum der `send` eines Segments (6.2): Grenzen sind `wait`,
/// `until`, eine Anweisung mit `->` und das Ende eines `repeat`-Koerpers.
fn segment_sends(items: &[SeqItem], count: &dyn Fn(&[Stmt]) -> u64) -> u64 {
    let (mut best, mut cur) = (0u64, 0u64);
    for item in items {
        match item {
            SeqItem::Stmt(s) => {
                cur += count(std::slice::from_ref(s));
                if Block::new(vec![s.clone()]).has_goto() {
                    best = best.max(cur);
                    cur = 0;
                }
            }
            SeqItem::Wait(_) => {
                best = best.max(cur);
                cur = 0;
            }
            SeqItem::Until { timeout, .. } => {
                best = best.max(cur);
                cur = 0;
                if let Some(TimeoutAction::Else(b)) = timeout.as_ref().map(|t| &t.action) {
                    best = best.max(count(&b.stmts));
                }
            }
            SeqItem::Expect { .. } => {}
            SeqItem::Repeat { body, .. } | SeqItem::Step { body, .. } => {
                best = best.max(cur).max(segment_sends(body, count));
                cur = 0;
            }
        }
    }
    best.max(cur)
}

fn walk_seq(items: &[SeqItem], f: &mut dyn FnMut(&Stmt, u32)) {
    for item in items {
        match item {
            SeqItem::Stmt(s) => walk_stmts(std::slice::from_ref(s), 0, f),
            SeqItem::Until { timeout: Some(t), .. } => {
                if let TimeoutAction::Else(b) = &t.action {
                    walk_stmts(&b.stmts, 0, f);
                }
            }
            SeqItem::Repeat { body, .. } | SeqItem::Step { body, .. } => walk_seq(body, f),
            _ => {}
        }
    }
}

fn walk_stmts(stmts: &[Stmt], depth: u32, f: &mut dyn FnMut(&Stmt, u32)) {
    for s in stmts {
        f(s, depth);
        match &s.kind {
            StmtKind::If { then, otherwise, .. } => {
                walk_stmts(&then.stmts, depth, f);
                walk_stmts(&otherwise.stmts, depth, f);
            }
            StmtKind::ForRange { body, .. } | StmtKind::ForEach { body, .. } => walk_stmts(&body.stmts, depth + 1, f),
            StmtKind::Every { body, .. } | StmtKind::At { body, .. } => walk_stmts(&body.stmts, depth, f),
            StmtKind::Match { arms, .. } => {
                for a in arms {
                    walk_stmts(&a.body.stmts, depth, f);
                }
            }
            _ => {}
        }
    }
}

/// Jede Anweisung eines Blocks.
pub fn for_each_stmt_block(b: &Block, f: &mut impl FnMut(&Stmt)) {
    walk_stmts(&b.stmts, 0, &mut |s, _| f(s));
}

/// Jeder Ausdruck einer Anweisung.
pub fn for_each_expr_stmt(s: &Stmt, f: &mut impl FnMut(&Expr)) {
    walk_stmts(std::slice::from_ref(s), 0, &mut |s, _| stmt_exprs(s, &mut |e| walk_expr(e, f)));
}

/// Jeder Ausdruck eines Blocks.
pub fn for_each_expr_block(b: &Block, f: &mut impl FnMut(&Expr)) {
    walk_stmts(&b.stmts, 0, &mut |s, _| stmt_exprs(s, &mut |e| walk_expr(e, f)));
}

/// Jeder Ausdruck einer Maschine.
pub fn for_each_expr_machine(m: &Machine, f: &mut impl FnMut(&Expr)) {
    for v in &m.vars {
        if let Some(init) = &v.init {
            walk_expr(init, f);
        }
    }
    for_each_stmt(m, &mut |s| stmt_exprs(s, &mut |e| walk_expr(e, f)));
    for s in &m.states {
        for t in &s.transitions {
            trigger_exprs(&t.trigger, &mut |e| walk_expr(e, f));
        }
        if let Some(seq) = &s.sequence {
            seq_exprs(&seq.items, &mut |e| walk_expr(e, f));
        }
    }
    for t in &m.faulted.transitions {
        trigger_exprs(&t.trigger, &mut |e| walk_expr(e, f));
    }
}

fn trigger_exprs(t: &TransTrigger, f: &mut impl FnMut(&Expr)) {
    match t {
        TransTrigger::After(d) => f(d),
        TransTrigger::When(Guard::Expr(e)) => f(e),
        TransTrigger::When(Guard::Match { subject, .. }) => f(subject),
        TransTrigger::When(Guard::Next { .. }) => {}
    }
}

fn seq_exprs(items: &[SeqItem], f: &mut impl FnMut(&Expr)) {
    for item in items {
        match item {
            SeqItem::Stmt(s) => stmt_exprs(s, f),
            SeqItem::Wait(d) => f(d),
            SeqItem::Until { guard, timeout, .. } => {
                match guard {
                    Guard::Expr(e) => f(e),
                    Guard::Match { subject, .. } => f(subject),
                    Guard::Next { .. } => {}
                }
                if let Some(t) = timeout {
                    f(&t.duration);
                }
            }
            SeqItem::Expect { cond, .. } => f(cond),
            SeqItem::Repeat { count, body, .. } => {
                f(count);
                seq_exprs(body, f);
            }
            SeqItem::Step { body, .. } => seq_exprs(body, f),
        }
    }
}

fn stmt_exprs(s: &Stmt, f: &mut impl FnMut(&Expr)) {
    match &s.kind {
        StmtKind::Assign { target, value } => {
            place_exprs(target, f);
            f(value);
        }
        StmtKind::Check { cond, message, confirm, .. } => {
            f(cond);
            if let Some(m) = message {
                format_exprs(m, f);
            }
            if let Some(c) = confirm {
                f(&c.duration);
            }
        }
        StmtKind::Abort { message: Some(m) } => format_exprs(m, f),
        StmtKind::If { cond, .. } => f(cond),
        StmtKind::ForRange { count, .. } => f(count),
        StmtKind::ForEach { iter, .. } => f(iter),
        StmtKind::Match { subject, arms } => {
            f(subject);
            for a in arms {
                if let ArmPattern::Values(vals) = &a.pattern {
                    for v in vals {
                        f(&v.lo);
                        if let Some(hi) = &v.hi {
                            f(hi);
                        }
                    }
                }
            }
        }
        StmtKind::Return(e) => f(e),
        StmtKind::Send { value, .. } => f(value),
        StmtKind::At { time, .. } => f(time),
        StmtKind::Job { args, .. } => {
            for a in args {
                f(a);
            }
        }
        StmtKind::Every { period, .. } => f(period),
        StmtKind::Observe(o) => match o {
            Observe::Alert { cond, message, confirm, .. } => {
                f(cond);
                format_exprs(message, f);
                if let Some(c) = confirm {
                    f(&c.duration);
                }
            }
            Observe::Log(m) => format_exprs(m, f),
            Observe::Measure { value, .. } => f(value),
            Observe::Verify { cond, message, .. } => {
                f(cond);
                format_exprs(message, f);
            }
            Observe::Verdict { message: Some(m), .. } => format_exprs(m, f),
            Observe::Verdict { .. } => {}
        },
        StmtKind::MethodCall { target, receiver, args, .. } => {
            if let Some(t) = target {
                place_exprs(t, f);
            }
            place_exprs(receiver, f);
            for a in args {
                f(a);
            }
        }
        _ => {}
    }
}

fn format_exprs(m: &takt_mir::pattern::Format, f: &mut impl FnMut(&Expr)) {
    for p in &m.pieces {
        if let takt_mir::pattern::FormatPiece::Expr { expr, .. } = p {
            f(expr);
        }
    }
}

fn place_exprs(p: &Place, f: &mut impl FnMut(&Expr)) {
    match p {
        Place::Var(_) | Place::Output(_) | Place::Port(_) => {}
        Place::Field(b, _) => place_exprs(b, f),
        Place::Index(b, i) => {
            place_exprs(b, f);
            f(i);
        }
        Place::Index2(b, i, j) => {
            place_exprs(b, f);
            f(i);
            f(j);
        }
    }
}

/// Ausdruck und alle Teilausdruecke.
pub fn walk_expr(e: &Expr, f: &mut impl FnMut(&Expr)) {
    f(e);
    let mut sub = |x: &Expr| walk_expr(x, f);
    match &e.kind {
        ExprKind::Variant { fields, .. } | ExprKind::Record { fields, .. } => fields.iter().for_each(sub),
        ExprKind::Array(items) => items.iter().for_each(sub),
        ExprKind::Tuple(a, b) => {
            sub(a);
            sub(b);
        }
        ExprKind::BlockInit { args, .. }
        | ExprKind::Call { args, .. }
        | ExprKind::NativeCall { args, .. }
        | ExprKind::MatOp { args, .. }
        | ExprKind::Intrinsic { args, .. } => args.iter().for_each(sub),
        ExprKind::Field { base, .. } => sub(base),
        ExprKind::Index { base, index } => {
            sub(base);
            sub(index);
        }
        ExprKind::Index2 { base, row, col } => {
            sub(base);
            sub(row);
            sub(col);
        }
        ExprKind::Slice { base, from, to } => {
            sub(base);
            sub(from);
            sub(to);
        }
        ExprKind::Accessor { base, args, .. } => {
            sub(base);
            args.iter().for_each(sub);
        }
        ExprKind::Unary { expr, .. }
        | ExprKind::Cast { expr, .. }
        | ExprKind::Convert { expr, .. }
        | ExprKind::Checked { expr, .. } => sub(expr),
        ExprKind::Lift(x) | ExprKind::Ok(x) | ExprKind::Err(x) => sub(x),
        ExprKind::Binary { lhs, rhs, .. } => {
            sub(lhs);
            sub(rhs);
        }
        ExprKind::Cond { cond, then, otherwise } => {
            sub(cond);
            sub(then);
            sub(otherwise);
        }
        ExprKind::Matches { subject, .. } => sub(subject),
        ExprKind::Decode { bytes, .. } => sub(bytes),
        ExprKind::Published { machine, .. } | ExprKind::StateOf(machine) | ExprKind::Signal { machine, .. } => {
            if let Some(i) = &machine.index {
                sub(i);
            }
        }
        _ => {}
    }
    let _ = SC8;
}

/// Elementtyp eines `stream<E>`.
fn stream_elem(p: &takt_mir::Program, ty: TypeId) -> Option<TypeId> {
    match p.types.list.get(ty.index()) {
        Some(Type::Stream(e)) => Some(*e),
        _ => None,
    }
}

/// Aufrundende Division fuer positive Nenner.
fn ceil_div(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { a.div_ceil(b) }
}

/// Schranken eines Stroms fuer Pruefung 17.
struct Caps {
    /// `CAP` in Elementen.
    cap: u64,
    /// `CAPB` in Bytes.
    cap_bytes: u64,
    /// Bytelast eines Elements.
    elem_bytes: u64,
}

/// Sammelt die Stellen, die unmittelbar auf ein `expect` folgen (12.7).
/// `repeat` und `step` zaehlen als eigene Folge.
fn mark_after_expect(items: &[SeqItem], out: &mut HashSet<(u32, u32)>) {
    let mut after = false;
    for item in items {
        match item {
            SeqItem::Expect { .. } => after = true,
            SeqItem::Stmt(s) => {
                if after {
                    out.insert((s.span.file.0, s.span.start));
                }
                // Nur die *eine* Anweisung hinter dem `expect` ist gedeckt.
                after = false;
            }
            SeqItem::Repeat { body, .. } => {
                after = false;
                mark_after_expect(body, out);
            }
            SeqItem::Step { body, .. } => {
                after = false;
                mark_after_expect(body, out);
            }
            _ => after = false,
        }
    }
}

/// Zerlegt eine Bedingung in Vergleich, linke und rechte Seite — nur die
/// Ordnungsvergleiche, weil `==`/`!=` keine Polaritaet haben.
fn comparison(e: &Expr) -> Option<(BinaryOp, &Expr, &Expr)> {
    match &e.kind {
        ExprKind::Binary { op, lhs, rhs }
            if matches!(op, BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge) =>
        {
            Some((*op, lhs.as_ref(), rhs.as_ref()))
        }
        // Ein impliziter Validitaetscheck steht vor dem Vergleich (3.5).
        ExprKind::Checked { expr, .. } => comparison(expr),
        _ => None,
    }
}

/// Der Zustand selbst, seine Vorfahren und alle Nachfahren.
///
/// 5.10 nennt Zustand und Vorfahren; die Nachfahren gehoeren dazu, weil
/// ein Kind mit `loop:` den Schritt genauso fuellt.
fn idle_scope(m: &Machine, id: StateId) -> Vec<StateId> {
    let mut out = vec![id];
    let mut up = m.states[id.index()].parent;
    while let Some(p) = up {
        out.push(p);
        up = m.states[p.index()].parent;
    }
    let mut stack = vec![id];
    while let Some(cur) = stack.pop() {
        for c in &m.states[cur.index()].children {
            if !out.contains(c) {
                out.push(*c);
                stack.push(*c);
            }
        }
    }
    out
}

/// Der erste Guard-Leser, der im Schlaf nicht weckt.
///
/// Erlaubt sind Wake-Quellen, Konstanten, Params, Tunables und
/// Maschinenvariablen (5.10).
fn non_wake_read(p: &Program, t: &takt_mir::machine::Transition) -> Option<String> {
    let TransTrigger::When(guard) = &t.trigger else { return None };
    let mut bad = None;
    let mut sehen = |e: &Expr| {
        if bad.is_some() {
            return;
        }
        match &e.kind {
            ExprKind::Input { channel, .. } if !p.channels[channel.index()].attrs.wake => {
                bad = Some(p.channels[channel.index()].name.clone());
            }
            ExprKind::Command(c) if !p.commands[c.index()].wake => {
                bad = Some(p.commands[c.index()].name.clone());
            }
            ExprKind::Published { .. } => bad = Some("eine fremde `pub var`".to_string()),
            ExprKind::StateOf(_) => bad = Some("den Zustand einer anderen Maschine".to_string()),
            _ => {}
        }
    };
    match guard {
        Guard::Expr(e) => walk_guard(e, &mut sehen),
        Guard::Match { subject, .. } => walk_guard(subject, &mut sehen),
        // `s as e` liest den Strom direkt; ohne `wake` weckt er nicht.
        Guard::Next { stream, .. } => {
            if let StreamRef::Channel(c) = stream
                && !p.channels[c.index()].attrs.wake
            {
                bad = Some(p.channels[c.index()].name.clone());
            }
        }
    }
    bad
}

/// Laeuft einen Ausdrucksbaum ab.
fn walk_guard(e: &Expr, f: &mut impl FnMut(&Expr)) {
    f(e);
    for c in e.children() {
        walk_guard(c, f);
    }
}

/// Der konstante Abstand einer `at`-Zeit zu `event.t` (7.5); `None`, wenn
/// die Zeit nicht die Form `event.t + d` hat.
fn delay_of(time: &Expr) -> Option<i64> {
    let time = match &time.kind {
        ExprKind::Checked { expr, .. } => expr.as_ref(),
        _ => time,
    };
    let ExprKind::Binary { op: takt_mir::expr::BinaryOp::Add, lhs, rhs } = &time.kind else { return None };
    // `event` ist ein Record, `.t` darum ein Feld — kein `Accessor::T`.
    let is_event_t = |e: &Expr| {
        matches!(&e.kind, ExprKind::Field { base, .. }
            if matches!(base.kind, ExprKind::Builtin(takt_mir::expr::Builtin::Event)))
    };
    let (a, b) = (lhs.as_ref(), rhs.as_ref());
    let d = if is_event_t(a) {
        b
    } else if is_event_t(b) {
        a
    } else {
        return None;
    };
    match &d.kind {
        ExprKind::Duration(ns) => Some(*ns),
        _ => None,
    }
}

/// Liest ein Ausdruck Zustand? (7.5: `when` ist knotenlokal.)
fn reads_state(e: &Expr) -> bool {
    let mut found = false;
    walk_expr(e, &mut |x| {
        if matches!(
            x.kind,
            ExprKind::Var(_) | ExprKind::Published { .. } | ExprKind::StateOf(_) | ExprKind::Signal { .. }
        ) {
            found = true;
        }
    });
    found
}
