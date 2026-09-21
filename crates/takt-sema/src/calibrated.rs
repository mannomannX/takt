//! Die Prüfungen, die eine Kalibrierung brauchen (12, 32; 7.2, 9.4.3).
//!
//! **Warum sie nicht in `checks.rs` stehen.** Jede andere Prüfung
//! entscheidet aus dem Programm allein. Diese beiden brauchen eine zweite
//! Eingabe — die gemessene Kostentabelle aus 13.8 —, und die hat nicht
//! jeder Aufrufer: Ein `takt check` ohne Hardware-Konfiguration ist der
//! Normalfall, kein Mangel. Sie als Pflichtfeld in [`crate::Options`] zu
//! führen hieße, dreiundfünfzig Aufrufstellen eine Angabe abzuverlangen,
//! die sie nicht machen können.
//!
//! Darum laufen sie hinterher und über ein fertiges Programm. Wer
//! kalibriert hat, ruft sie; wer nicht, bekommt weiterhin den Hinweis aus
//! `checks.rs`, der sagt, was fehlt.
//!
//! **Was sie melden, wenn etwas fehlt.** Nicht nichts. Eine Prüfung, die
//! bei fehlender Eingabe schweigt, ist von einer bestandenen Prüfung
//! nicht zu unterscheiden — genau der Zustand, den FB-136 festhielt. Eine
//! unvollständige Tabelle nennt darum die Klassen, die ihr fehlen, und
//! ein fehlendes `tick_source` sagt, dass Prüfung 32 es verlangt.

use takt_diag::{Diagnostic, Severity, Span};
use takt_mir::analysis::schedulability::{self, Load};
use takt_mir::expr::{Expr, ExprKind};
use takt_mir::hardware::{Hardware, HwChannel, Target};
use takt_mir::machine::MachineKind;
use takt_mir::program::{Binding, Direction, OverrunPolicy, Program, Sweep};
use takt_mir::review::Review;
use takt_mir::stmt::{Place, StmtKind};
use takt_mir::types::{Const, Type};

use crate::checks::{SC12, SC28, SC29, SC31, SC32, SC39, SC59, SC60};

/// Prüft Kostenbudget und Schedulability gegen eine Kalibrierung.
///
/// `span` ist die Stelle, an der ein Fehler angezeigt wird — sinnvoll ist
/// der `system:`-Block, weil `tick` und `tick_source` dort stehen.
pub fn check(p: &Program, target: &Target, span: Span) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let load = schedulability::load(p);

    // Prüfung 32 verlangt zweierlei: die Ungleichung *und* dass
    // `tick_source` in der Hardware-Konfiguration steht. Das Zweite ist
    // unabhängig von der Kalibrierung und wird darum zuerst geprüft.
    if p.config.tick_source.is_none() {
        out.push(
            Diagnostic::new(
                Severity::Warning,
                SC32,
                span,
                "`tick_source` fehlt; Prüfung 32 verlangt sie in der Hardware-Konfiguration (7.1)".to_string(),
            )
            .with_suggestion("`system: tick_source = hw(\"…\")` nennt die Quelle, aus der der Tick kommt".to_string()),
        );
    }

    out.extend(journal_blocking(p, target, span));

    let Some(verdict) = load.judge(&target.c_target, target.t_io_ps) else {
        let fehlend: Vec<&str> = target.c_target.missing().iter().map(|c| c.name()).collect();
        out.push(
            Diagnostic::new(
                Severity::Warning,
                SC32,
                span,
                format!(
                    "die Kalibrierung für `{}` ist unvollständig: {} ohne Messwert",
                    target.name,
                    fehlend.join(", ")
                ),
            )
            .with_suggestion(
                "`takt bench` misst die fehlenden Klassen; eine Null wäre kein Messwert, sondern eine Lücke, \
                 und die Schedulability rechnete dann zu günstig"
                    .to_string(),
            ),
        );
        return out;
    };

    if !verdict.fits() {
        out.push(
            Diagnostic::error(
                SC32,
                span,
                format!(
                    "die Rechenlast eines Ticks passt nicht: {} ns nötig, {} ns verfügbar ({} %)",
                    ns(verdict.needed_ps),
                    ns(verdict.available_ps),
                    verdict.utilisation_percent()
                ),
            )
            .with_suggestion(
                "Periode erhöhen, Phase verschieben oder Fault-Pfade verkleinern (7.2); `takt cost` zeigt, \
                 welcher Zustand welche Klasse treibt"
                    .to_string(),
            ),
        );
    }

    out.extend(declared_budgets(p, target, &load));
    out.extend(memory_budget(p, target, span));
    out
}

/// Prüfung 32, zweite Klausel: Ein blockierendes NVM kostet den Tick
/// Perioden (12.3); ob das Programm sie tragen kann, sagen `overrun`
/// (7.3) und seine `idle`-Zustände (9.9).
fn journal_blocking(p: &Program, target: &Target, span: Span) -> Vec<Diagnostic> {
    if !takt_mir::persist::any(p) {
        return Vec::new();
    }
    let undecidable = |what: &str| {
        vec![
            Diagnostic::new(
                Severity::Warning,
                SC32,
                span,
                format!("`persist` auf `{}`, aber {what} fehlt in der Hardware-Konfiguration (8.10)", target.name),
            )
            .with_suggestion(
                "ob das Journal den Tick anhält, ist damit nicht entscheidbar; `nvm_blocking`, `nvm_erase_ns` und \
                 `nvm_program_ns` sagen es (12.3)"
                    .to_string(),
            ),
        ]
    };
    let Some(nvm) = target.nvm else {
        return if target.memory.iram.is_some() { undecidable("`nvm_blocking`") } else { Vec::new() };
    };
    match nvm.blocking {
        None => return if target.memory.iram.is_some() { undecidable("`nvm_blocking`") } else { Vec::new() },
        Some(false) => return Vec::new(),
        Some(true) => {}
    }
    let Some(cost) = takt_mir::persist::journal_cost(p, &nvm) else {
        return undecidable("`nvm_erase_ns` oder `nvm_program_ns`");
    };
    let ms = cost.write_ns / 1_000_000;
    let has_idle = p.machines.iter().any(|m| m.states.iter().any(|s| s.idle));
    let cost_text = format!(
        "ein `persist`-Schreibvorgang hält den Tick um {} Perioden an ({ms} ms auf `{}`)",
        cost.periods, target.name
    );
    vec![match (p.config.overrun, has_idle) {
        (OverrunPolicy::Fault, false) => Diagnostic::error(
            SC32,
            span,
            format!(
                "{cost_text}; unter `overrun = fault` wäre jeder Schreibvorgang ein Fault, und ohne `idle`-Zustand \
                 gibt es kein Schlaffenster dafür"
            ),
        )
        .with_suggestion(format!(
            "`system: overrun = alert` nimmt die Überläufe an; ein `idle`-Zustand mit Frist über {ms} ms lässt das \
             Journal im Schlaf schreiben (9.9); oder ein Ziel mit `nvm_blocking = false` (12.3)"
        )),
        (OverrunPolicy::Fault, true) => Diagnostic::new(
            Severity::Note,
            SC32,
            span,
            format!("{cost_text}; das Journal schreibt nur in Schlaffenstern darüber und vor `reboot`/Deep Sleep"),
        ),
        (OverrunPolicy::Alert, _) => Diagnostic::new(
            Severity::Warning,
            SC32,
            span,
            format!("{cost_text}; unter `overrun = alert` ist jeder Schreibvorgang ein Overrun-Alert (7.3)"),
        ),
    }]
}

/// Prüfung 39: `takt size` gegen `ram` und `flash` des Ziels (11.5).
///
/// Verglichen wird nur Belastbares; ein offener Posten macht die Summe zur
/// Untergrenze, und das sagt die Meldung. Ein Ziel ohne Speicherangaben
/// bekommt kein Urteil.
fn memory_budget(p: &Program, target: &Target, span: Span) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let size = takt_mir::analysis::size::size(p).with_hardware(target);
    let note = if size.has_open() { " (offene Posten nicht gezählt, die Summe ist eine Untergrenze)" } else { "" };
    for (what, have, limit) in
        [("RAM", size.ram_total(), target.memory.ram), ("Flash", size.flash_total(), target.memory.flash)]
    {
        let Some(limit) = limit else { continue };
        if have > limit {
            out.push(
                Diagnostic::error(
                    SC39,
                    span,
                    format!("{what}: {have} Byte gerechnet, das Ziel `{}` hat {limit}{note}", target.name),
                )
                .with_suggestion(
                    "`takt size` nennt die Posten; Kapazitäten verkleinern, `expect_len` setzen oder `float = f32` \
                     (11.5)"
                        .to_string(),
                ),
            );
        }
    }
    out
}

/// Prüfungen 60 und 28: die Bindungen des Programms gegen die Kanäle der
/// Konfiguration (8.10).
///
/// **Ohne Konfiguration kein Urteil** — das ist der Normalfall, und darum
/// läuft dies wie [`check`] hinterher. Mit Konfiguration ist eine Adresse,
/// die sie nicht kennt, ein Fehler: 8.10 sagt, welche Channels eine
/// Plattform anbietet, steht in ihrer Konfiguration.
pub fn check_bindings(p: &Program, hw: &Hardware) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let tick = p.config.tick;
    for c in &p.channels {
        let Binding::Hw(addr) = &c.binding else { continue };
        let address = addr.text();
        // Das Geraet `sys` ist eingebaut (12.7); `checks.rs` kennt es.
        if takt_mir::sys::is_sys(&address) {
            continue;
        }
        let Some(entry) = hw.channel(&address) else {
            out.push(
                Diagnostic::error(SC60, c.span, format!("die Konfiguration kennt `{address}` nicht"))
                    .with_suggestion(format!("`[channel {address}]` eintragen oder die Adresse im Programm ändern")),
            );
            continue;
        };
        if let Some(dir) = entry.direction {
            if dir != c.dir {
                let (want, have) = (dir_name(c.dir), dir_name(dir));
                out.push(Diagnostic::error(
                    SC60,
                    c.span,
                    format!("`{}` ist im Programm {want}, in der Konfiguration {have}", c.name),
                ));
            }
        }
        let elem = element_type(p, c.ty);
        if let (Some(cfg_unit), Some(unit)) = (&entry.unit, unit_name(p, elem)) {
            if cfg_unit != &unit {
                out.push(
                    Diagnostic::error(
                        SC60,
                        c.span,
                        format!("`{}` bindet `[{unit}]`, die Konfiguration führt `{cfg_unit}`", c.name),
                    )
                    .with_suggestion(
                        "3.2: Einheiten sind nominal; die Konfiguration nennt, was der Treiber liefert".to_string(),
                    ),
                );
            }
        }
        if let (Some((lo, hi)), Some((plo, phi))) = (entry.range, declared_range(p, elem)) {
            if plo < lo || phi > hi {
                out.push(
                    Diagnostic::error(
                        SC60,
                        c.span,
                        format!("`{}` verlangt {plo}..{phi}, das Gerät liefert {lo}..{hi}", c.name),
                    )
                    .with_suggestion(
                        "die Range des Programms muss innerhalb der Geräte-Range liegen (3.5, 12.6)".to_string(),
                    ),
                );
            }
        }
        if let (Some(cfg_safe), Some(safe)) = (&entry.safe, c.attrs.safe.as_ref()) {
            if safe_matches(p, safe, cfg_safe) == Some(false) {
                let shown = literal_text(p, safe).unwrap_or_default();
                out.push(
                    Diagnostic::error(
                        SC60,
                        c.span,
                        format!("`{}` hat `safe = {shown}`, die Konfiguration `{cfg_safe}`", c.name),
                    )
                    .with_suggestion("12.4: der Treiber kennt denselben Safe-Wert wie das Programm".to_string()),
                );
            }
        }
        out.extend(jitter_check(p, c, entry, tick));
        out.extend(sweep_check(p, c, entry));
    }
    out
}

/// Prüfung 29: Sweep-Schritte gegen den gemessenen Jitter eines Outputs,
/// den eine `at`-Anweisung mit dem Parameter stellt (13.7).
fn sweep_check(p: &Program, c: &takt_mir::program::Channel, entry: &HwChannel) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let Some(jitter) = entry.jitter_ns else { return out };
    let Some(i) = p.channels.iter().position(|x| std::ptr::eq(x, c)) else { return out };
    let params = at_params(p, takt_mir::ChannelId(i as u32));
    for campaign in &p.campaigns {
        for sweep in &campaign.sweeps {
            let Sweep::Range { param, step, .. } = sweep else { continue };
            let ExprKind::Duration(ns) = &step.kind else { continue };
            if params.contains(param) && *ns < 2 * jitter {
                out.push(Diagnostic::error(
                    SC29,
                    campaign.span,
                    format!(
                        "Kampagne `{}`: Sweep-Schritt {} von `{}` liegt unter 2·jitter = {} von `{}` (`at`, 13.7)",
                        campaign.name,
                        takt_mir::dump::duration(*ns),
                        p.params[param.index()].name,
                        takt_mir::dump::duration(2 * jitter),
                        c.name
                    ),
                ));
            }
        }
    }
    out
}

/// Parameter in der Zeit der `at`-Anweisungen, die diesen Output stellen.
fn at_params(p: &Program, c: takt_mir::ChannelId) -> Vec<takt_mir::ParamId> {
    let mut out = Vec::new();
    for m in &p.machines {
        for b in m.blocks() {
            b.walk(&mut |s| {
                let StmtKind::At { time, body } = &s.kind else { return };
                let sets = body
                    .stmts
                    .iter()
                    .any(|x| matches!(&x.kind, StmtKind::Assign { target: Place::Output(o), .. } if *o == c));
                if sets {
                    params_in(time, &mut out);
                }
            });
        }
    }
    out
}

fn params_in(e: &Expr, out: &mut Vec<takt_mir::ParamId>) {
    if let ExprKind::Param(id) = &e.kind {
        out.push(*id);
    }
    for child in e.children() {
        params_in(child, out);
    }
}

/// Prüfung 28: gemessener Jitter gegen `at` und gegen die Anforderung.
fn jitter_check(p: &Program, c: &takt_mir::program::Channel, entry: &HwChannel, tick: i64) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let Some(jitter) = entry.jitter_ns else { return out };
    if c.dir != Direction::Output {
        return out;
    }
    if let Some(want) = c.attrs.jitter {
        if jitter > want {
            out.push(Diagnostic::error(
                SC28,
                c.span,
                format!("`{}` verlangt jitter ≤ {want} ns, gemessen sind {jitter} ns", c.name),
            ));
        }
    }
    let scheduled =
        p.channels.iter().position(|x| std::ptr::eq(x, c)).is_some_and(|i| {
            p.machines.iter().any(|m| m.layout.output_queues.contains(&takt_mir::ChannelId(i as u32)))
        });
    if scheduled && jitter >= tick {
        out.push(
            Diagnostic::new(
                Severity::Warning,
                SC28,
                c.span,
                format!("`{}`: gemessener Jitter {jitter} ns ≥ Tick {tick} ns, `at` wirkt tick-granular (7.5)", c.name),
            )
            .with_suggestion("einen Timer-Compare-Pin nehmen oder die Anforderung streichen".to_string()),
        );
    }
    out
}

fn dir_name(d: Direction) -> &'static str {
    match d {
        Direction::Input => "Input",
        Direction::Output => "Output",
    }
}

/// Der Elementtyp eines Channel-Arrays, sonst der Typ selbst.
fn element_type(p: &Program, ty: takt_mir::TypeId) -> takt_mir::TypeId {
    match p.types.list.get(ty.index()) {
        Some(Type::Array { elem, .. }) => *elem,
        _ => ty,
    }
}

fn unit_name(p: &Program, ty: takt_mir::TypeId) -> Option<String> {
    let unit = match p.types.list.get(ty.index())? {
        Type::Int { unit, .. } | Type::Float { unit, .. } => (*unit)?,
        _ => return None,
    };
    p.units.get(unit.index()).map(|u| u.name.clone())
}

fn declared_range(p: &Program, ty: takt_mir::TypeId) -> Option<(f64, f64)> {
    let range = match p.types.list.get(ty.index())? {
        Type::Int { range, .. } | Type::Float { range, .. } => range.as_ref()?,
        _ => return None,
    };
    Some((const_f64(&range.lo)?, const_f64(&range.hi)?))
}

fn const_f64(c: &Const) -> Option<f64> {
    match c {
        Const::Int(n) => Some(*n as f64),
        Const::Float(f) => Some(*f),
        _ => None,
    }
}

/// Stimmt das `safe`-Literal des Programms mit dem Text der Konfiguration
/// ueberein? Zahlen numerisch (`0` und `0.0` sind dasselbe), Wahrheitswerte
/// und Varianten beim Namen; `None`, wenn das Literal keine Form hat, die
/// sich vergleichen laesst.
fn safe_matches(p: &Program, e: &takt_mir::expr::Expr, cfg: &str) -> Option<bool> {
    Some(match &e.kind {
        ExprKind::Bool(b) => cfg == b.to_string(),
        ExprKind::Int(n) => cfg.parse::<f64>().ok()? == *n as f64,
        ExprKind::Float(f) => cfg.parse::<f64>().ok()? == *f,
        ExprKind::Variant { .. } => literal_text(p, e)? == cfg,
        _ => return None,
    })
}

/// Ein `safe`-Literal als Text, wie die Konfiguration ihn schreibt.
fn literal_text(p: &Program, e: &takt_mir::expr::Expr) -> Option<String> {
    Some(match &e.kind {
        ExprKind::Bool(b) => b.to_string(),
        ExprKind::Int(n) => n.to_string(),
        ExprKind::Float(f) => format!("{f:?}"),
        ExprKind::Variant { enum_id, variant, fields } if fields.is_empty() => {
            p.enums.get(enum_id.index())?.variants.get(*variant as usize)?.name.clone()
        }
        _ => return None,
    })
}

/// Prüfung 12: `with budget = {wcet = …}` je Maschine.
///
/// **Je Aktivierung, nicht je Tick.** 9.4.3 nennt `B_m` „eine obere
/// Schranke der pro Aktivierung von m ausgeführten abstrakten
/// Operationen"; eine Maschine mit `n_m = 10` belastet den Tick nur jeden
/// zehnten, aber ihr deklariertes `wcet` gilt für den Durchlauf. `T_IO`
/// geht hier nicht ein — es ist eine Eigenschaft des Ticks, nicht der
/// Maschine.
fn declared_budgets(p: &Program, target: &Target, _load: &Load) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for m in p.machines.iter().filter(|m| m.kind != MachineKind::Template) {
        let (Some(declared), Some(budget)) = (m.declared_budget, m.budget) else { continue };
        let Some(want) = declared.wcet_ns else { continue };
        let have_ps = target.c_target.duration_ps(budget.activation + budget.fault_path);
        let want_ps = (want.max(0) as u64).saturating_mul(1000);
        if have_ps > want_ps {
            out.push(
                Diagnostic::error(
                    SC12,
                    declared.span,
                    format!("`{}` braucht {} ns je Aktivierung, deklariert sind {want} ns", m.name, ns(have_ps)),
                )
                .with_suggestion(
                    "Budget anheben, Schleifen verkürzen oder Fault-Pfade verkleinern (9.4.3); der Fault-Pfad \
                     zählt mit, weil er im selben Tick läuft (5.4)"
                        .to_string(),
                ),
            );
        }
    }
    out
}

/// Prüfung 59: Ein gepolltes Gerät läuft zwischen zwei Ticks nicht über.
///
/// `fifo_depth[d] / byte_rate[d] >= P_m + jitter[tick_source] + wcet_poll[d]`.
/// Die linke Seite ist die Zeit, in der das FIFO volläuft; die rechte, wie
/// lange der Treiber schlimmstenfalls nicht hinsieht.
///
/// **Welche Maschine ein Gerät pollt.** Ein Port ist im Sim-Build das
/// Channelpaar `mmio/ADR/r` und `mmio/ADR/w` (12.9), und ein Channel nennt
/// sein Gerät (8.10). Damit ist die Zuordnung schon da: kein zweiter
/// Bindungsweg, dieselbe Regel wie für jeden anderen Kanal — die Adresse
/// ist ein Schlüssel in die Konfiguration.
///
/// **Was gemeldet wird, wenn eine Größe fehlt.** Ein Fehler, keine Stille:
/// Eine Prüfung, die bei fehlender Eingabe schweigt, ist von einer
/// bestandenen nicht zu unterscheiden. `with polling = unchecked` an der
/// Maschine ist der ausdrückliche Verzicht; er erscheint im Lauf-Header.
pub fn polling(p: &Program, hw: &Hardware, target: Option<&Target>) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let jitter_ns = tick_jitter(p, hw);
    for (name, device) in &hw.devices {
        if device.fifo_depth.is_none() && device.byte_rate.is_none() {
            continue;
        }
        let Some(m) = polling_machine(p, hw, name) else { continue };
        let machine = &p.machines[m.index()];
        if machine.polling_unchecked {
            continue;
        }
        let span = machine.span;
        let mut missing: Vec<&str> = Vec::new();
        if device.fifo_depth.is_none() {
            missing.push("fifo_depth");
        }
        if device.byte_rate.is_none() {
            missing.push("byte_rate");
        }
        if jitter_ns.is_none() {
            missing.push("jitter_ns der Tickquelle");
        }
        let wcet_ns = target.and_then(|t| poll_wcet_ns(machine, t));
        if wcet_ns.is_none() {
            missing.push("wcet_poll (Kalibrierung, 13.8)");
        }
        if !missing.is_empty() {
            out.push(
                Diagnostic::error(
                    SC59,
                    span,
                    format!(
                        "`{}` pollt `{name}`, aber Prüfung 59 ist nicht entscheidbar: {} fehlt",
                        machine.name,
                        missing.join(", ")
                    ),
                )
                .with_suggestion(
                    "die Größen in die Hardware-Konfiguration eintragen (8.10) beziehungsweise `takt bench` \
                     laufen lassen (13.8); `with polling = unchecked` an der Maschine verzichtet \
                     ausdrücklich darauf und erscheint im Lauf-Header"
                        .to_string(),
                ),
            );
            continue;
        }
        let (depth, rate) = (device.fifo_depth.expect("geprüft") as u64, device.byte_rate.expect("geprüft"));
        if rate == 0 {
            continue;
        }
        // Volllaufzeit in Nanosekunden, ohne Fliesskomma: die Rechnung
        // gehört zu einem Urteil und muss auf jedem Ziel gleich ausfallen.
        let fill_ns = depth.saturating_mul(1_000_000_000) / rate;
        let period_ns = (machine.period as i64).saturating_mul(p.config.tick).max(0) as u64;
        let need_ns = period_ns.saturating_add(jitter_ns.expect("geprüft")).saturating_add(wcet_ns.expect("geprüft"));
        if fill_ns < need_ns {
            out.push(
                Diagnostic::error(
                    SC59,
                    span,
                    format!(
                        "`{}` pollt `{name}` zu selten: das FIFO läuft nach {fill_ns} ns voll, \
                         der Treiber sieht erst nach {need_ns} ns wieder hin",
                        machine.name
                    ),
                )
                .with_suggestion(
                    "Periode senken, das Gerät in die TCB geben (12.6) oder DMA statt Polling verwenden".to_string(),
                ),
            );
        }
    }
    out
}

/// Die Maschine, deren Port an einem Channel dieses Geräts hängt.
fn polling_machine(p: &Program, hw: &Hardware, device: &str) -> Option<takt_mir::MachineId> {
    p.ports.iter().find_map(|port| {
        let owner = port.owner?;
        let keys = [format!("mmio/{:#x}/r", port.address), format!("mmio/{:#x}/w", port.address)];
        keys.iter().filter_map(|k| hw.channel(k)).any(|c| c.device.as_deref() == Some(device)).then_some(owner)
    })
}

/// `jitter[tick_source]`: der gemessene Jitter der Tickquelle (13.8).
fn tick_jitter(p: &Program, hw: &Hardware) -> Option<u64> {
    let source = p.config.tick_source.as_ref()?;
    let jitter = hw.channel(&source.text())?.jitter_ns?;
    u64::try_from(jitter).ok()
}

/// `wcet_poll`: was eine Aktivierung der Treibermaschine kostet (9.4.3).
fn poll_wcet_ns(m: &takt_mir::machine::Machine, target: &Target) -> Option<u64> {
    let budget = m.budget?;
    if !target.c_target.missing().is_empty() {
        return None;
    }
    Some(ns(target.c_target.duration_ps(budget.activation + budget.fault_path)))
}

/// Prüfung 31, zweite Klausel: `tcb_policy = reviewed(…)` (4.5, v1.2).
///
/// Ein Projekt-Native erweitert die TCB. `allowlist` sagt, dass das
/// Programm es weiß; `reviewed` verlangt zusätzlich, dass jemand
/// hingesehen hat. Der Schlüssel ist der Hash der Quelle, nicht ihr Name:
/// Eine geänderte Implementierung ist eine andere und braucht einen
/// eigenen Eintrag.
///
/// **Warum hier und nicht in `checks.rs`.** Wie die Prüfungen darüber
/// braucht sie eine zweite Eingabe — die Review-Datei und die Quellen —,
/// und die hat nicht jeder Aufrufer. Ohne `reviewed` im Programm tut sie
/// nichts.
pub fn reviewed(p: &Program, review: &Review, source_of: &dyn Fn(&str) -> Option<Vec<u8>>) -> Vec<Diagnostic> {
    if !p.config.tcb_reviewed {
        return Vec::new();
    }
    let mut out = Vec::new();
    for n in p.natives.iter().filter(|n| n.from.is_some()) {
        let from = n.from.as_deref().unwrap_or_default();
        let Some(source) = source_of(from) else {
            out.push(
                Diagnostic::error(SC31, n.span, format!("`{}`: `{from}` ist nicht lesbar", n.name))
                    .with_suggestion("`reviewed` prüft den Hash der Quelle; ohne sie ist nichts zu prüfen".to_string()),
            );
            continue;
        };
        let hash = takt_mir::review::hash_of(&source);
        if review.entry(&n.name, &hash).is_some() {
            continue;
        }
        // Eine Zeile mit anderem Hash ist der haeufige Fall: Die Quelle
        // wurde nach dem Review geaendert. Das zu sagen ist hilfreicher
        // als „nicht geprueft".
        let stale = review.any_for(&n.name);
        let mut d = Diagnostic::error(
            SC31,
            n.span,
            if stale.is_empty() {
                format!("`{}` ist nicht geprüft; `natives.review` nennt es nicht", n.name)
            } else {
                format!("`{}` wurde seit dem Review geändert ({from})", n.name)
            },
        );
        d = d.with_suggestion(format!(
            "`takt tcb review <datei> --native {} --by <name> --date <tag>` schreibt die Zeile, \
             nachdem jemand hingesehen hat (4.5)",
            n.name
        ));
        out.push(d);
    }
    out
}

/// Pikosekunden als Nanosekunden, kaufmännisch gerundet.
fn ns(ps: u64) -> u64 {
    ps.saturating_add(500) / 1000
}
