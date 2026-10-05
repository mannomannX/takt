//! Was die Runtime vorgefunden hat — und was daraus folgt (12.2, 11.3).
//!
//! **Eine Zeitgarantie, die still ausfaellt, ist schlimmer als eine, die
//! fehlt.** Ohne `SCHED_FIFO` laeuft der Tick-Thread als gewoehnlicher
//! Prozess; er rechnet dasselbe, aber jede Messung an ihm misst den
//! Scheduler mit. Wer das nicht weiss, haelt die Zahl fuer eine Zusage.
//!
//! Darum steht hier keine Pruefung, die den Lauf abbricht, sondern ein
//! Befund, der ihn begleitet: [`Guarantee`] geht in den Lauf-Header
//! (11.3) und in die Aufzeichnung (12.5).

/// Die Schedulingklasse des Tick-Threads (12.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheduling {
    /// `SCHED_FIFO`: Echtzeit, wie 12.2 verlangt.
    Realtime {
        /// Die Echtzeitprioritaet (1 bis 99).
        priority: i32,
    },
    /// `SCHED_RR`: Echtzeit mit Zeitscheiben unter gleicher Prioritaet.
    RoundRobin {
        /// Die Echtzeitprioritaet (1 bis 99).
        priority: i32,
    },
    /// `SCHED_OTHER`: der gewoehnliche Scheduler. Der Lauf ist gueltig,
    /// die Zeitgarantie nicht.
    Normal,
    /// `SCHED_BATCH`: gewoehnlich, fuer Hintergrundlast; keine Zeitgarantie.
    Batch,
    /// `SCHED_IDLE`: nur, wenn sonst nichts laeuft; keine Zeitgarantie.
    Idle,
    /// Nicht ermittelbar (kein Linux, `/proc` nicht lesbar, unbekannte Policy).
    Unknown,
}

impl Scheduling {
    /// Traegt diese Klasse die Zeitgarantie aus 12.2?
    pub fn is_realtime(self) -> bool {
        matches!(self, Scheduling::Realtime { .. } | Scheduling::RoundRobin { .. })
    }

    /// Der Name fuer den Lauf-Header.
    pub fn name(self) -> &'static str {
        match self {
            Scheduling::Realtime { .. } => "fifo",
            Scheduling::RoundRobin { .. } => "rr",
            Scheduling::Normal => "other",
            Scheduling::Batch => "batch",
            Scheduling::Idle => "idle",
            Scheduling::Unknown => "unbekannt",
        }
    }
}

/// Was die Runtime beim Start vorgefunden und erreicht hat (12.2).
///
/// Jedes Feld ist eine der vier Zusagen aus 12.2. Zusammen sagen sie, ob
/// eine Messung an diesem Lauf die Zeitgarantie traegt — und das gehoert
/// in den Lauf-Header, nicht in die Erinnerung des Bedieners.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Guarantee {
    /// Die Schedulingklasse des Tick-Threads.
    pub scheduling: Scheduling,
    /// `mlockall` ist gelungen: Keine Seite wird ausgelagert.
    pub locked: bool,
    /// Auf wie viele Kerne der Thread gebunden ist.
    ///
    /// Einer heisst: isoliert, wie 12.2 es verlangt. Mehr heisst, dass
    /// der Scheduler ihn verschieben darf, und jede Verschiebung kostet
    /// einen kalten Cache.
    pub cpus: usize,
}

impl Guarantee {
    /// Traegt dieser Lauf die Zeitgarantie aus 12.2 vollstaendig?
    ///
    /// Alle drei Teile oder keiner: Ein Thread mit Echtzeitprioritaet auf
    /// einem Kern, dessen Seiten ausgelagert werden koennen, hat sie
    /// nicht — ein Seitenfehler kostet Millisekunden.
    pub fn is_complete(&self) -> bool {
        self.scheduling.is_realtime() && self.locked && self.cpus == 1
    }

    /// Was fehlt, als Text fuer die Meldung beim Start.
    ///
    /// Die Meldung nennt den Weg: `chrt` fuer die Prioritaet, `taskset`
    /// fuer die Bindung. Ein Befund ohne Abhilfe ist eine Klage.
    pub fn missing(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.scheduling.is_realtime() {
            out.push(format!(
                "keine Echtzeitprioritaet (Klasse `{}`, 12.2): \
                 mit `chrt -f 80 …` starten; das Programm setzt sie nicht selbst, \
                 weil es dafuer CAP_SYS_NICE braeuchte",
                self.scheduling.name()
            ));
        }
        if !self.locked {
            out.push(
                "`mlockall` ist nicht gelungen (12.2): ein Seitenfehler kostet Millisekunden; \
                 RLIMIT_MEMLOCK pruefen"
                    .to_string(),
            );
        }
        if self.cpus != 1 {
            out.push(format!(
                "auf {} Kerne gebunden statt auf einen (12.2, `isolcpus`): \
                 mit `taskset -c <kern>` binden",
                self.cpus
            ));
        }
        out
    }

    /// Der Befund als Zeilen fuer den Lauf-Header (11.3).
    pub fn header_lines(&self) -> Vec<String> {
        vec![
            format!("scheduling {}", self.scheduling.name()),
            format!("mlockall {}", if self.locked { "ja" } else { "nein" }),
            format!("cpus {}", self.cpus),
            format!("echtzeit {}", if self.is_complete() { "ja" } else { "nein" }),
        ]
    }
}

/// Bereitet den Tick-Thread vor und meldet, was erreicht wurde (12.2).
///
/// **Die Runtime setzt, was sie darf, und misst den Rest.** `mlockall`
/// und die Kernbindung stehen jedem Prozess offen; die Prioritaet nicht,
/// und sie zu erzwingen hiesse, privilegiert laufen zu muessen.
///
/// `cpu` bindet den Thread an einen Kern; `None` laesst die Bindung, wie
/// sie ist (etwa, weil `taskset` sie schon gesetzt hat).
#[cfg(target_os = "linux")]
pub fn prepare(cpu: Option<usize>) -> Guarantee {
    // 12.2: „`mlockall`, vorab beruehrte Seiten". Beide Flags: CURRENT
    // fuer das, was schon da ist, FUTURE fuer alles Weitere — ohne FUTURE
    // waere jede spaetere Seite wieder auslagerbar.
    let locked = rustix::mm::mlockall(rustix::mm::MlockAllFlags::CURRENT | rustix::mm::MlockAllFlags::FUTURE).is_ok();

    if let Some(n) = cpu {
        let mut set = rustix::thread::CpuSet::new();
        set.set(n);
        let _ = rustix::thread::sched_setaffinity(None, &set);
    }
    let cpus = rustix::thread::sched_getaffinity(None).map_or(0, |s| s.count() as usize);

    Guarantee { scheduling: scheduling(), locked, cpus }
}

/// Auf Nicht-Linux gibt es die Zusagen aus 12.2 nicht; der Befund sagt es.
#[cfg(not(target_os = "linux"))]
pub fn prepare(_cpu: Option<usize>) -> Guarantee {
    Guarantee { scheduling: Scheduling::Unknown, locked: false, cpus: 0 }
}

/// Liest die Schedulingklasse aus `/proc/self/stat` (12.2).
///
/// **Gelesen, nicht gesetzt.** `rustix` bietet `sched_setscheduler` nicht
/// an, und das trifft sich mit der Entscheidung: Die Klasse ist
/// Betriebskonfiguration wie `isolcpus` daneben.
///
/// Feld 41 von `/proc/self/stat` ist die Policy (`sched(7)`): 0 ist
/// `SCHED_OTHER`, 1 `SCHED_FIFO`, 2 `SCHED_RR`, 3 `SCHED_BATCH`, 5
/// `SCHED_IDLE`. Feld 40 ist die Echtzeitprioritaet; Feld 18 (`priority`)
/// waere unter Echtzeit `-1 - rt_priority`.
#[cfg(target_os = "linux")]
fn scheduling() -> Scheduling {
    std::fs::read_to_string("/proc/self/stat").map_or(Scheduling::Unknown, |stat| scheduling_of(&stat))
}

/// Die Schedulingklasse aus dem Text von `/proc/<pid>/stat` (`proc(5)`).
pub fn scheduling_of(stat: &str) -> Scheduling {
    // Der Prozessname steht in Klammern und darf Leerzeichen enthalten;
    // gezaehlt wird darum ab der schliessenden Klammer.
    let Some(rest) = stat.rfind(')').map(|i| &stat[i + 1..]) else {
        return Scheduling::Unknown;
    };
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // Nach der Klammer ist Feld 0 der Zustand, also Feld n der
    // (n + 3)-te des Formats.
    let policy = fields.get(38).and_then(|s| s.parse::<u32>().ok());
    let priority = fields.get(37).and_then(|s| s.parse::<i32>().ok()).unwrap_or(0);
    match policy {
        Some(0) => Scheduling::Normal,
        Some(1) => Scheduling::Realtime { priority },
        Some(2) => Scheduling::RoundRobin { priority },
        Some(3) => Scheduling::Batch,
        Some(5) => Scheduling::Idle,
        _ => Scheduling::Unknown,
    }
}
