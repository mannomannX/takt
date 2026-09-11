//! Pruefungen auf der MIR (plan/m1.md 3.10): Fault-Wald (9), Single-Writer
//! und Bindungen (7), Erreichbarkeit (10), Terminierung (11), Simulation (13),
//! ungenutzte Channels (15), Definite Assignment gehobener Variablen (6, 25).
//!
//! Sie laufen nach dem Lowering, aber vor dem Desugaring: dort sind Namen
//! aufgeloest, und die Sequenz-Oberflaeche zeigt noch ihre Segmentgrenzen.

use std::collections::{HashMap, HashSet};

use takt_diag::{Diagnostic, Span};
use takt_mir::expr::{Expr, ExprKind, StreamRef};
use takt_mir::machine::*;
use takt_mir::program::{Binding, Direction};
use takt_mir::stmt::*;
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
/// Lints zu Matrizen und  (3.11, 8.6).
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
        self.performance_lints();
        self.check_writers();
        self.check_fault_forest();
        self.check_latency();
        self.check_declared_budget();
        self.check_reachability();
        self.check_termination();
        self.check_simulation();
        self.check_unused();
        self.check_definite_assignment();
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
        let mut worst = 0u64;
        for m in &self.program.machines {
            if matches!(m.kind, MachineKind::Template) {
                continue;
            }
            let mut n = 0u64;
            for_each_stmt(m, &mut |s| {
                if let StmtKind::Send { stream, .. } = &s.kind {
                    if *stream == target {
                        n += 1;
                    }
                }
            });
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
        let narrow_core = matches!(self.program.config.target.as_deref(), Some("baremetal" | "boot"));
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
        let mut diags = Vec::new();
        for (c, list) in &writers {
            if list.len() > 1 {
                let name = self.program.channels[c.index()].name.clone();
                let first = &self.program.machines[list[0].0.index()].name;
                let mut d = Diagnostic::error(SC7, list[1].1, format!("Output `{name}` hat mehrere Schreiber"))
                    .with_note(list[0].1, format!("auch in `{first}` geschrieben"))
                    .with_suggestion("jeder Output gehoert genau einer Maschine (1.4)");
                d.span = list[1].1;
                diags.push(d);
            }
        }
        for (c, list) in writers {
            self.program.channels[c.index()].owner = Some(list[0].0);
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
            for t in &m.faulted.transitions {
                if let TransTrigger::When(Guard::Expr(e)) = &t.trigger {
                    if has_checked(e) {
                        diags.push(
                            Diagnostic::error(
                                SC9,
                                t.span,
                                format!("Guard aus `FAULTED` von `{}` enthaelt eine implizite Pruefung", m.name),
                            )
                            .with_suggestion(
                                "Channel nur unter `.valid` oder mit `.or(...)` lesen; keine Range- oder \
                                 Arithmetik-Pruefung (5.3)",
                            ),
                        );
                    }
                }
            }
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

fn check_seq_items(
    items: &[SeqItem],
    assigned: &mut HashSet<VarId>,
    lifted: &HashSet<VarId>,
    m: &Machine,
    diags: &mut Vec<Diagnostic>,
) {
    for item in items {
        match item {
            SeqItem::Stmt(s) => {
                for_each_expr_stmt(s, &mut |e| {
                    if let ExprKind::Var(v) = &e.kind {
                        if lifted.contains(v) && !assigned.contains(v) {
                            diags.push(
                                Diagnostic::error(
                                    SC25,
                                    e.span,
                                    format!("`{}` wird gelesen, bevor sie zugewiesen ist", m.vars[v.index()].name),
                                )
                                .with_suggestion("Zuweisung in ein frueheres Segment legen (6.2, 5.8)"),
                            );
                        }
                    }
                });
                if let StmtKind::Assign { target: Place::Var(v), .. } = &s.kind {
                    assigned.insert(*v);
                }
            }
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
        Place::Var(_) => None,
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
    let visit_block = |b: &Block, f: &mut dyn FnMut(&Stmt, u32)| walk_stmts(&b.stmts, 0, f);
    visit_block(&m.loop_block, f);
    // Ein Handler-Rumpf ist gewoehnlicher Code (8.7): er schreibt Outputs und
    // liest Channels wie jeder andere Block.
    for h in &m.handlers {
        visit_block(&h.body, f);
    }
    for t in &m.faulted.transitions {
        visit_block(&t.actions, f);
    }
    for s in &m.states {
        visit_block(&s.enter, f);
        visit_block(&s.exit, f);
        visit_block(&s.loop_block, f);
        for h in &s.handlers {
            visit_block(&h.body, f);
        }
        for t in &s.transitions {
            visit_block(&t.actions, f);
        }
        if let Some(seq) = &s.sequence {
            walk_seq(&seq.items, f);
        }
    }
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
            Observe::Alert { cond, message, confirm } => {
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
        Place::Var(_) | Place::Output(_) => {}
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

/// Enthaelt der Ausdruck eine implizite Pruefung (5.3, Pruefung 9)?
fn has_checked(e: &Expr) -> bool {
    let mut found = false;
    walk_expr(e, &mut |x| {
        if matches!(x.kind, ExprKind::Checked { .. }) {
            found = true;
        }
    });
    found
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
