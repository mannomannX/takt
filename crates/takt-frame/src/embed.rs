//! Die Lieferform fuer einen Wirt in Rust (12.11, M11 Schritt 8): das
//! Modul `P.rs` zur Bibliothek `libP.a`.
//!
//! Es nennt, was der Wirt braucht, um das Programm zu binden: die
//! Konstanten, die Arena als Typ, den Trait `Drivers` mit dem Kleber zum
//! Treiberobjekt des Wirts und die sichere Huelle `Program`, die die
//! Einstiege der Bibliothek dem Kern ohne Warten gibt
//! (`takt_embed::rt::Program`).
//!
//! **Was die Huelle zusichert.** Sie haelt Arena und Treiberobjekt als
//! Ausleihe `&'a mut`: Zwei Programme auf derselben Arena, ein Zugriff des
//! Wirts hinein oder ein Verschieben waehrend des Laufs uebersetzen nicht
//! (12.11). Bibliothek und Huelle muessen zueinander passen; die Huelle
//! liest dafuer die Symbole `P_abi_<n>` und `P_logic_<hash>`, und ohne sie
//! bindet das Programm nicht.

use std::fmt::Write as _;

use takt_llvm::symbols::Prefix;

use crate::drivers::Driver;

/// Die Version der Schnittstelle zwischen Bibliothek und Huelle (12.11):
/// Sie steht im Symbol `P_abi_<n>` und im Manifest.
pub const ABI: u32 = 4;

/// Was das Modul braucht.
pub struct Module<'a> {
    /// Das Praefix der Einstiege.
    pub prefix: &'a Prefix,
    /// Die Konstanten des Programms als Rust (`takt build --emit consts-rs`).
    pub consts: &'a str,
    /// Groesse und Ausrichtung der Arena ([`crate::mcu::arena_layout`]).
    pub arena: (u64, u64),
    /// Die Groesse des Job-Stacks ([`crate::mcu::job_stack_bytes`]).
    pub job_stack_bytes: u64,
    /// Der Logik-Hash des Programms ([`crate::mcu::logic_hex`]).
    pub logic: &'a str,
    /// Die Treiber des Programms.
    pub drivers: &'a [Driver],
    /// Der Rust-Typ, der `Drivers` erfuellt; ohne ihn stellt der Wirt die
    /// Treiber in C, und die Huelle reicht keinen Zeiger.
    pub drivers_type: Option<&'a str>,
}

/// Das Modul `P.rs`.
pub fn rust_module(m: &Module<'_>) -> String {
    let x = m.prefix;
    let (bytes, align) = m.arena;
    let mut s = String::new();
    let _ = writeln!(s, "// Erzeugt von `takt build --emit embed` (12.11); nicht von Hand aendern.\n");
    s.push_str(m.consts);
    let _ = writeln!(s);
    s.push_str(&crate::mcu::rust_arena(bytes, align));
    let _ = writeln!(s);
    s.push_str(&crate::mcu::rust_job_stack(m.job_stack_bytes));
    let _ = writeln!(s);
    s.push_str(&ffi(x, m.logic));
    let _ = writeln!(s);
    s.push_str(&crate::drivers::rust_trait(m.drivers));
    if let Some(ty) = m.drivers_type {
        let _ = writeln!(s);
        s.push_str(&crate::drivers::rust_glue(m.drivers, x, ty));
    }
    s.push_str(&hull(x, m.drivers_type, m.logic));
    s
}

/// Die Einstiege der Bibliothek (C-ABI).
fn ffi(x: &Prefix, logic: &str) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "/// Die Einstiege der Bibliothek (C-ABI, 12.11); die Huelle `Program` ruft sie.");
    let _ = writeln!(s, "mod ffi {{");
    let _ = writeln!(s, "    use core::ffi::c_void;\n");
    let _ = writeln!(s, "    unsafe extern \"C\" {{");
    let lines = [
        format!("pub fn {x}_init_with(a: *mut c_void, user: *mut c_void, persist: *const c_void, len: i32) -> i32;"),
        format!("pub fn {x}_init(a: *mut c_void, user: *mut c_void);"),
        format!("pub fn {x}_tick(a: *mut c_void, k: i64);"),
        format!("pub fn {x}_commit(a: *mut c_void);"),
        format!("pub fn {x}_idle(a: *mut c_void) -> u8;"),
        format!("pub fn {x}_deadline(a: *mut c_void) -> i64;"),
        format!("pub fn {x}_advance(a: *mut c_void, n: i64);"),
        format!("pub fn {x}_tune(a: *mut c_void, k: i64, param: u32, value: *const c_void, len: i32) -> i32;"),
        format!("pub fn {x}_woken(a: *mut c_void, k: i64) -> u8;"),
        format!("pub fn {x}_wake_sources() -> u8;"),
        format!("pub fn {x}_persist_snapshot(a: *mut c_void, out: *mut c_void, cap: i32) -> i32;"),
        format!("pub fn {x}_persist_restore(a: *mut c_void, bytes: *const c_void, len: i32) -> i32;"),
        format!("pub fn {x}_dump(a: *mut c_void, all: i32);"),
        format!("pub fn {x}_pc(a: *mut c_void);"),
        format!("pub fn {x}_next_run(a: *mut c_void, delay: *mut i64) -> i32;"),
        format!("pub fn {x}_end(a: *mut c_void);"),
        format!("pub fn {x}_overrun(a: *mut c_void);"),
        format!("pub fn {x}_hardware(a: *mut c_void);"),
        format!("pub fn {x}_tolerance(ns: *mut i64, runs: *mut u32);"),
        format!("pub fn {x}_output_timing() -> u8;"),
        format!("pub fn {x}_job_dispatch(a: *mut c_void) -> i32;"),
        format!("pub fn {x}_job_work(a: *mut c_void);"),
        format!("pub fn {x}_output(a: *mut c_void, index: i32) -> i64;"),
        format!("pub static {x}_abi_{ABI}: u8;"),
        format!("pub static {x}_logic_{logic}: u8;"),
    ];
    for line in lines {
        let _ = writeln!(s, "        {line}");
    }
    let _ = writeln!(s, "    }}\n}}");
    s
}

/// Die sichere Huelle `Program` und der Griff `Jobs`.
fn hull(x: &Prefix, drivers_type: Option<&str>, logic: &str) -> String {
    let (param, user, borrow) = match drivers_type {
        Some(ty) => (
            format!(", drivers: &'a mut {ty}"),
            "core::ptr::from_mut(drivers).cast()".to_string(),
            format!("core::marker::PhantomData<(&'a mut Arena, &'a mut {ty})>"),
        ),
        None => {
            (String::new(), "core::ptr::null_mut()".to_string(), "core::marker::PhantomData<&'a mut Arena>".to_string())
        }
    };
    format!(
        r#"
/// Das Programm auf seiner Arena (12.11), fuer den Kern ohne Warten
/// (`takt_embed::rt::Runtime::service`). Es haelt Arena und Treiberobjekt
/// geliehen, solange es lebt.
pub struct Program<'a> {{
    arena: *mut core::ffi::c_void,
    user: *mut core::ffi::c_void,
    initialized: bool,
    jobs_taken: bool,
    dispatch_taken: bool,
    borrow: {borrow},
}}

impl<'a> Program<'a> {{
    /// Das Programm vor Tick 0: Das Journal darf noch laden
    /// (`takt_embed::rt::Persist::load`), sonst beginnt `ensure_init`.
    pub fn new(arena: &'a mut Arena{param}) -> Program<'a> {{
        // Bibliothek und Huelle muessen dieselbe Schnittstelle sprechen und
        // dasselbe Programm meinen; ohne `{x}_abi_{ABI}` und
        // `{x}_logic_{logic}` bindet das Programm nicht (12.11).
        // SAFETY: liest zwei Konstanten der Bibliothek.
        let _ = unsafe {{ core::ptr::read_volatile(&raw const ffi::{x}_abi_{ABI}) }};
        let _ = unsafe {{ core::ptr::read_volatile(&raw const ffi::{x}_logic_{logic}) }};
        Program {{
            arena: core::ptr::from_mut(arena).cast(),
            user: {user},
            initialized: false,
            jobs_taken: false,
            dispatch_taken: false,
            borrow: core::marker::PhantomData,
        }}
    }}

    /// Das Programm nach Tick 0, ohne Journal (9.4).
    pub fn init(arena: &'a mut Arena{param}) -> Program<'a> {{
        let mut p = Program::new(arena{args});
        p.ensure_init();
        p
    }}

    /// Tick 0 ohne geladene Werte, wenn das Journal keine hatte.
    pub fn ensure_init(&mut self) {{
        if !self.initialized {{
            // SAFETY: die Arena und das Treiberobjekt aus `new`, geliehen fuer `'a`.
            unsafe {{ ffi::{x}_init(self.arena, self.user) }};
            self.initialized = true;
        }}
    }}

    /// Der Griff fuer den Job-Kontext (4.5): Er rechnet, was `service` als
    /// `jobs` meldet. Es gibt ihn einmal je Programm, damit nie zwei
    /// Job-Kontexte dieselbe Arena rechnen; `None` beim zweiten Aufruf. Er
    /// rechnet nie auf einer Arena vor `init`: Das Programm beginnt hier,
    /// wenn es noch nicht begonnen hat.
    pub fn jobs(&mut self) -> Option<Jobs<'a>> {{
        if core::mem::replace(&mut self.jobs_taken, true) {{
            return None;
        }}
        self.ensure_init();
        Some(Jobs {{ arena: self.arena, borrow: core::marker::PhantomData }})
    }}

    /// Der Griff, der dem ruhenden Job-Kontext den naechsten Auftrag gibt
    /// (4.5), fuer einen Port, der zwischen den Ticks selbst verteilt statt
    /// ueber `service`: einmal je Programm, nur im Kontext des Schritts.
    pub fn dispatch(&mut self) -> Option<Dispatch<'a>> {{
        if core::mem::replace(&mut self.dispatch_taken, true) {{
            return None;
        }}
        self.ensure_init();
        Some(Dispatch {{ arena: self.arena, borrow: core::marker::PhantomData }})
    }}

    /// Ein Ausgang als Bitmuster, nach seiner Stellung (`OUT_*`): fuer
    /// Diagnose und Messung, nicht fuer Treiber.
    pub fn output(&self, index: i32) -> i64 {{
        if !self.initialized {{
            return 0;
        }}
        // SAFETY: liest einen Latch der Arena nach `init`; ein fremder Index liefert 0.
        unsafe {{ ffi::{x}_output(self.arena, index) }}
    }}
}}

/// Der Griff, der verteilt (4.5): Er gibt dem ruhenden Job-Kontext den
/// aeltesten wartenden Job. Nicht `Send`: Er bleibt im Kontext des Schritts.
#[derive(Debug)]
pub struct Dispatch<'a> {{
    arena: *mut core::ffi::c_void,
    borrow: core::marker::PhantomData<&'a mut Arena>,
}}

impl Dispatch<'_> {{
    /// Gibt den naechsten Auftrag; wahr, wenn der Job-Kontext zu rechnen hat.
    /// Nur, solange er ruht, und nicht zugleich mit `service`.
    pub fn next(&mut self) -> bool {{
        // SAFETY: Den Auftrag schreibt nur der Kontext des Schritts, solange
        // der Job-Kontext ruht; den Griff gibt es einmal (`Program::dispatch`).
        unsafe {{ ffi::{x}_job_dispatch(self.arena) != 0 }}
    }}
}}

impl takt_embed::Dispatch for Dispatch<'_> {{
    fn next(&mut self) -> bool {{
        Dispatch::next(self)
    }}
}}

/// Der Griff des Job-Kontexts auf die Arena (4.5): `work` rechnet den Auftrag,
/// den `service` gegeben hat, und ist durch `service` unterbrechbar. Er lebt
/// nicht laenger als die Arena.
#[derive(Debug)]
pub struct Jobs<'a> {{
    arena: *mut core::ffi::c_void,
    borrow: core::marker::PhantomData<&'a mut Arena>,
}}

// SAFETY: Schrittkontext und Job-Kontext teilen die Arena nach dem Protokoll
// des Rahmens: Den Auftrag schreibt `service` nur, solange der Kontext ruht.
// Den Griff gibt es nur einmal (`Program::jobs`).
unsafe impl Send for Jobs<'_> {{}}

impl Jobs<'_> {{
    /// Rechnet den Auftrag, nur im Job-Kontext.
    pub fn work(&mut self) {{
        // SAFETY: siehe `Send`.
        unsafe {{ ffi::{x}_job_work(self.arena) }}
    }}
}}

impl<'a> takt_embed::Program for Program<'a> {{
    type Jobs = Jobs<'a>;
    type Dispatch = Dispatch<'a>;

    fn jobs(&mut self) -> Option<Jobs<'a>> {{
        Program::jobs(self)
    }}

    fn dispatch(&mut self) -> Option<Dispatch<'a>> {{
        Program::dispatch(self)
    }}
}}

impl takt_embed::Jobs for Jobs<'_> {{
    fn work(&mut self) {{
        Jobs::work(self)
    }}
}}

impl takt_embed::rt::Program for Program<'_> {{
    fn tick(&mut self, k: u64, _now: i64) {{
        self.ensure_init();
        // Der Kern zaehlt ab 0; der Rahmen nennt den ersten Tick nach dem
        // Start `t=1` (Tick 0 ist der Start).
        // SAFETY: die Arena aus `new`.
        unsafe {{ ffi::{x}_tick(self.arena, k as i64 + 1) }}
    }}

    fn raise_overrun(&mut self) {{
        self.ensure_init();
        // SAFETY: wie oben.
        unsafe {{ ffi::{x}_overrun(self.arena) }}
    }}

    fn tune(&mut self, k: u64, param: u32, value: &[u8]) {{
        self.ensure_init();
        let len = i32::try_from(value.len()).unwrap_or(i32::MAX);
        // SAFETY: Der Rahmen liest `len` Byte ab `value`; einen Wert, den er
        // nicht annimmt, laesst er stehen (8.4) und schreibt ihn als
        // verworfen in den Trace, in Tick `k + 1` wie `tick`.
        let _ = unsafe {{ ffi::{x}_tune(self.arena, k as i64 + 1, param, value.as_ptr().cast(), len) }};
    }}

    fn woken(&mut self, k: u64) -> bool {{
        if !self.initialized {{
            return false;
        }}
        // SAFETY: die Arena nach `init`; der Rahmen tastet die Treiber ab
        // wie der Schritt von Tick `k + 1`.
        unsafe {{ ffi::{x}_woken(self.arena, k as i64 + 1) != 0 }}
    }}

    fn wake_sources(&self) -> bool {{
        // SAFETY: liest eine Konstante der Bibliothek.
        unsafe {{ ffi::{x}_wake_sources() != 0 }}
    }}

    fn raise_hardware(&mut self) {{
        self.ensure_init();
        // SAFETY: wie oben.
        unsafe {{ ffi::{x}_hardware(self.arena) }}
    }}

    fn tick_tolerance(&self) -> Option<takt_embed::rt::Tolerance> {{
        let (mut ns, mut runs) = (0i64, 0u32);
        // SAFETY: schreibt zwei Zahlen an die uebergebenen Stellen.
        unsafe {{ ffi::{x}_tolerance(&mut ns, &mut runs) }};
        Some(takt_embed::rt::Tolerance {{ ns, runs }})
    }}

    fn commit_at_boundary(&self) -> bool {{
        // SAFETY: liest eine Konstante der Bibliothek.
        unsafe {{ ffi::{x}_output_timing() == 1 }}
    }}

    fn sleep_allowed(&self) -> bool {{
        // Vor `init` ist die Arena nicht beschrieben; geschlafen wird nicht.
        // SAFETY: liest die Arena nach `init`.
        self.initialized && unsafe {{ ffi::{x}_idle(self.arena) != 0 }}
    }}

    fn next_deadline(&self) -> Option<i64> {{
        if !self.initialized {{
            return None;
        }}
        // SAFETY: liest die Arena nach `init`.
        let ns = unsafe {{ ffi::{x}_deadline(self.arena) }};
        (ns >= 0).then_some(ns)
    }}

    fn advance(&mut self, ticks: u64) {{
        self.ensure_init();
        // SAFETY: die Arena aus `new`.
        unsafe {{ ffi::{x}_advance(self.arena, ticks as i64) }}
    }}

    fn persist_snapshot(&mut self, out: &mut [u8]) -> usize {{
        self.ensure_init();
        let cap = i32::try_from(out.len()).unwrap_or(i32::MAX);
        // SAFETY: Der Rahmen schreibt hoechstens `cap` Byte an `out`.
        let n = unsafe {{ ffi::{x}_persist_snapshot(self.arena, out.as_mut_ptr().cast(), cap) }};
        usize::try_from(n).unwrap_or(0)
    }}

    fn persist_restore(&mut self, bytes: &[u8]) -> usize {{
        let len = i32::try_from(bytes.len()).unwrap_or(i32::MAX);
        // SAFETY: Der Rahmen liest `len` Byte ab `bytes`; vor dem Start
        // beginnt er damit, danach ersetzt er nur die Werte.
        let n = unsafe {{
            if self.initialized {{
                ffi::{x}_persist_restore(self.arena, bytes.as_ptr().cast(), len)
            }} else {{
                self.initialized = true;
                ffi::{x}_init_with(self.arena, self.user, bytes.as_ptr().cast(), len)
            }}
        }};
        usize::try_from(n).unwrap_or(0)
    }}

    fn next_run(&self) -> Option<takt_embed::rt::NextRun> {{
        if !self.initialized {{
            return None;
        }}
        let mut delay = 0i64;
        // SAFETY: liest die Arena und schreibt `delay`.
        let code = unsafe {{ ffi::{x}_next_run(self.arena, &mut delay) }};
        takt_embed::rt::NextRun::from_code(code, delay)
    }}

    fn commit(&mut self) {{
        self.ensure_init();
        // SAFETY: gibt den Latch der Arena an die Treiber.
        unsafe {{ ffi::{x}_commit(self.arena) }}
    }}

    fn trace(&mut self, outputs: takt_embed::rt::Outputs) {{
        self.ensure_init();
        let all = match outputs {{
            takt_embed::rt::Outputs::None => return,
            takt_embed::rt::Outputs::Changed => 0,
            takt_embed::rt::Outputs::All => 1,
        }};
        // SAFETY: liest die Arena und schreibt den Trace ueber die Leitung des Wirts.
        unsafe {{
            ffi::{x}_dump(self.arena, all);
            ffi::{x}_pc(self.arena);
        }}
    }}

    fn end(&mut self) {{
        self.ensure_init();
        // SAFETY: schreibt die Zeile `end` und die `safe`-Werte in den Latch.
        unsafe {{ ffi::{x}_end(self.arena) }}
    }}

    fn dispatch_job(&mut self) -> bool {{
        self.ensure_init();
        // SAFETY: die Arena aus `new`.
        unsafe {{ ffi::{x}_job_dispatch(self.arena) != 0 }}
    }}
}}
"#,
        args = if drivers_type.is_some() { ", drivers" } else { "" },
    )
}
