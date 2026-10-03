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
//! liest dafuer das Symbol `P_abi_<n>`, und ohne es bindet das Programm
//! nicht.

use std::fmt::Write as _;

use takt_llvm::symbols::Prefix;

use crate::drivers::Driver;

/// Die Version der Schnittstelle zwischen Bibliothek und Huelle (12.11):
/// Sie steht im Symbol `P_abi_<n>` und im Manifest.
pub const ABI: u32 = 1;

/// Was das Modul braucht.
pub struct Module<'a> {
    /// Das Praefix der Einstiege.
    pub prefix: &'a Prefix,
    /// Die Konstanten des Programms als Rust (`takt build --emit consts-rs`).
    pub consts: &'a str,
    /// Groesse und Ausrichtung der Arena ([`crate::mcu::arena_layout`]).
    pub arena: (u64, u64),
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
    s.push_str(&ffi(x));
    let _ = writeln!(s);
    s.push_str(&crate::drivers::rust_trait(m.drivers));
    if let Some(ty) = m.drivers_type {
        let _ = writeln!(s);
        s.push_str(&crate::drivers::rust_glue(m.drivers, x, ty));
    }
    s.push_str(&hull(x, m.drivers_type));
    s
}

/// Die Einstiege der Bibliothek (C-ABI).
fn ffi(x: &Prefix) -> String {
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
        format!("pub fn {x}_persist_snapshot(a: *mut c_void, out: *mut c_void, cap: i32) -> i32;"),
        format!("pub fn {x}_persist_restore(a: *mut c_void, bytes: *const c_void, len: i32) -> i32;"),
        format!("pub fn {x}_dump(a: *mut c_void, all: i32);"),
        format!("pub fn {x}_pc(a: *mut c_void);"),
        format!("pub fn {x}_next_run(a: *mut c_void, delay: *mut i64) -> i32;"),
        format!("pub fn {x}_end(a: *mut c_void);"),
        format!("pub fn {x}_overrun(a: *mut c_void);"),
        format!("pub fn {x}_hardware(a: *mut c_void);"),
        format!("pub fn {x}_tolerance(ns: *mut i64, runs: *mut u32);"),
        format!("pub fn {x}_job_dispatch(a: *mut c_void) -> i32;"),
        format!("pub fn {x}_job_work(a: *mut c_void);"),
        format!("pub static {x}_abi_{ABI}: u8;"),
    ];
    for line in lines {
        let _ = writeln!(s, "        {line}");
    }
    let _ = writeln!(s, "    }}\n}}");
    s
}

/// Die sichere Huelle `Program` und der Griff `Jobs`.
fn hull(x: &Prefix, drivers_type: Option<&str>) -> String {
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
    borrow: {borrow},
}}

impl<'a> Program<'a> {{
    /// Das Programm vor Tick 0: Das Journal darf noch laden
    /// (`takt_embed::rt::Persist::load`), sonst beginnt `ensure_init`.
    pub fn new(arena: &'a mut Arena{param}) -> Program<'a> {{
        // Bibliothek und Huelle muessen dieselbe Schnittstelle sprechen; ohne
        // `{x}_abi_{ABI}` bindet das Programm nicht (12.11).
        // SAFETY: liest eine Konstante der Bibliothek.
        let _ = unsafe {{ core::ptr::read_volatile(&raw const ffi::{x}_abi_{ABI}) }};
        Program {{ arena: core::ptr::from_mut(arena).cast(), user: {user}, initialized: false, borrow: core::marker::PhantomData }}
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
    /// `jobs` meldet.
    pub fn jobs(&self) -> Jobs {{
        Jobs(self.arena)
    }}
}}

/// Der Griff des Job-Kontexts auf die Arena (4.5): `work` rechnet den Auftrag,
/// den `service` gegeben hat, und ist durch `service` unterbrechbar.
#[derive(Clone, Copy, Debug)]
pub struct Jobs(*mut core::ffi::c_void);

// SAFETY: Schrittkontext und Job-Kontext teilen die Arena nach dem Protokoll
// des Rahmens: Den Auftrag schreibt `service` nur, solange der Kontext ruht.
unsafe impl Send for Jobs {{}}

impl Jobs {{
    /// Rechnet den Auftrag, nur im Job-Kontext.
    pub fn work(self) {{
        // SAFETY: siehe `Send`.
        unsafe {{ ffi::{x}_job_work(self.0) }}
    }}
}}

impl takt_embed::Program for Program<'_> {{
    type Jobs = Jobs;

    fn jobs(&self) -> Jobs {{
        Program::jobs(self)
    }}
}}

impl takt_embed::Jobs for Jobs {{
    fn work(self) {{
        Jobs::work(self)
    }}
}}

impl takt_embed::rt::Program for Program<'_> {{
    fn tick(&mut self, k: u64, _now: i64) {{
        // Der Kern zaehlt ab 0; der Rahmen nennt den ersten Tick nach dem
        // Start `t=1` (Tick 0 ist der Start).
        // SAFETY: die Arena aus `new`.
        unsafe {{ ffi::{x}_tick(self.arena, k as i64 + 1) }}
    }}

    fn raise_overrun(&mut self) {{
        // SAFETY: wie oben.
        unsafe {{ ffi::{x}_overrun(self.arena) }}
    }}

    fn raise_hardware(&mut self) {{
        // SAFETY: wie oben.
        unsafe {{ ffi::{x}_hardware(self.arena) }}
    }}

    fn tick_tolerance(&self) -> Option<takt_embed::rt::Tolerance> {{
        let (mut ns, mut runs) = (0i64, 0u32);
        // SAFETY: schreibt zwei Zahlen an die uebergebenen Stellen.
        unsafe {{ ffi::{x}_tolerance(&mut ns, &mut runs) }};
        Some(takt_embed::rt::Tolerance {{ ns, runs }})
    }}

    fn sleep_allowed(&self) -> bool {{
        // SAFETY: liest die Arena.
        unsafe {{ ffi::{x}_idle(self.arena) != 0 }}
    }}

    fn next_deadline(&self) -> Option<i64> {{
        // SAFETY: liest die Arena.
        let ns = unsafe {{ ffi::{x}_deadline(self.arena) }};
        (ns >= 0).then_some(ns)
    }}

    fn advance(&mut self, ticks: u64) {{
        // SAFETY: die Arena aus `new`.
        unsafe {{ ffi::{x}_advance(self.arena, ticks as i64) }}
    }}

    fn persist_snapshot(&mut self, out: &mut [u8]) -> usize {{
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
        let mut delay = 0i64;
        // SAFETY: liest die Arena und schreibt `delay`.
        match unsafe {{ ffi::{x}_next_run(self.arena, &mut delay) }} {{
            1 => Some(takt_embed::rt::NextRun::Now),
            2 => Some(takt_embed::rt::NextRun::After(delay)),
            3 => Some(takt_embed::rt::NextRun::OnWake),
            4 => Some(takt_embed::rt::NextRun::OnStart),
            _ => None,
        }}
    }}

    fn commit(&mut self) {{
        // SAFETY: gibt den Latch der Arena an die Treiber.
        unsafe {{ ffi::{x}_commit(self.arena) }}
    }}

    fn trace(&mut self, outputs: takt_embed::rt::Outputs) {{
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
        // SAFETY: schreibt die Zeile `end` und die `safe`-Werte in den Latch.
        unsafe {{ ffi::{x}_end(self.arena) }}
    }}

    fn dispatch_job(&mut self) -> bool {{
        // SAFETY: die Arena aus `new`.
        unsafe {{ ffi::{x}_job_dispatch(self.arena) != 0 }}
    }}
}}
"#,
        args = if drivers_type.is_some() { ", drivers" } else { "" },
    )
}
