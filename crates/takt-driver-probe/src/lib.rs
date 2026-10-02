//! Das Pruefgeraet des Treiberrands als Treiber-Crate (12.6, 13.8; M10
//! Schritte 29c, 29d und 11).
//!
//! Ein Treiber-Crate stellt die Einstiege der nativen Treiberschnittstelle,
//! die der MCU-Rahmen je gebundener Adresse ruft: `takt_in_<adr>`,
//! `takt_poll_<adr>`, `takt_out_<adr>`, `takt_alive_<geraet>` (12.1, 12.6).
//! Dieses liefert die Folge aus `takt_board_support::edge_probe`: jeden
//! Verstoss gegen den Treibervertrag einmal, dazu einen unbestaetigten
//! Schreibvorgang und einen stillen Heartbeat, und die Kanaele, die
//! `recorded.takt` nicht liest (8.2). Die Programme dazu stehen in
//! `takt-conformance/tests/programs/driver_edge.takt` und `recorded.takt`.
//!
//! Beide Bring-ups und das Wirts-Bring-up binden es; ein Programm, das
//! diese Adressen nicht bindet, ruft keinen dieser Einstiege. Auf dem Wirt
//! faehrt `takt driver-test --crate` es wie jedes andere Treiber-Crate.

#![no_std]
#![allow(unsafe_code, reason = "C-ABI des Rahmens; 9.5 fuehrt Treiber in der TCB")]

use core::sync::atomic::{AtomicU32, Ordering};

use takt_board_support::edge_probe;

unsafe extern "C" {
    fn takt_mcu_current_tick() -> i64;
}

/// Der Tick, den der Rahmen gerade rechnet.
fn edge_tick() -> u64 {
    // SAFETY: liest nur eine Zahl des Rahmens.
    u64::try_from(unsafe { takt_mcu_current_tick() }).unwrap_or(0)
}

/// Eine Abtastung des Pruefgeraets.
///
/// # Safety
///
/// Drei gueltige Zeiger des Rahmens.
unsafe fn edge_reading(ch: edge_probe::Scalar, value: *mut i64, quality: *mut u8, t: *mut i64) -> bool {
    let Some((v, at)) = edge_probe::reading(ch, edge_tick()) else { return false };
    unsafe {
        *value = v;
        *quality = 0;
        if let Some(at) = at {
            *t = at;
        }
    }
    true
}

/// Der Treiber fuer `edge_a/p`.
///
/// # Safety
///
/// Drei gueltige Zeiger des Rahmens.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_in_edge_a_p(value: *mut i64, quality: *mut u8, t: *mut i64) -> bool {
    unsafe { edge_reading(edge_probe::Scalar::P, value, quality, t) }
}

/// Der Treiber fuer `edge_a/q`.
///
/// # Safety
///
/// Drei gueltige Zeiger des Rahmens.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_in_edge_a_q(value: *mut i64, quality: *mut u8, t: *mut i64) -> bool {
    unsafe { edge_reading(edge_probe::Scalar::Q, value, quality, t) }
}

/// Der Treiber fuer `edge_b/k`.
///
/// # Safety
///
/// Drei gueltige Zeiger des Rahmens.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_in_edge_b_k(value: *mut i64, quality: *mut u8, t: *mut i64) -> bool {
    unsafe { edge_reading(edge_probe::Scalar::K, value, quality, t) }
}

/// Je Strom der Tick und die Zahl der Elemente, die er darin schon geliefert hat.
static EDGE_POLLS: [(AtomicU32, AtomicU32); 2] =
    [(AtomicU32::new(u32::MAX), AtomicU32::new(0)), (AtomicU32::new(u32::MAX), AtomicU32::new(0))];

/// Wie oft die Stroeme des Pruefgeraets, die `recorded.takt` nicht liest,
/// im laufenden Tick gefragt wurden.
static UNREAD_POLLS: [(AtomicU32, AtomicU32); 3] = [
    (AtomicU32::new(u32::MAX), AtomicU32::new(0)),
    (AtomicU32::new(u32::MAX), AtomicU32::new(0)),
    (AtomicU32::new(u32::MAX), AtomicU32::new(0)),
];

/// Ein Element des Pruefgeraets: Bytes, Zeitstempel und Folgenummer, wenn eigene.
type ProbeElement = (&'static [u8], Option<i64>, Option<i64>);

/// Das naechste Element einer Folge des Pruefgeraets: `element(tick, i)`
/// liefert Bytes, Zeitstempel und Folgenummer, ohne Angabe die des Rahmens.
///
/// # Safety
///
/// `buf` zeigt auf `cap` schreibbare Bytes, die uebrigen auf je einen Platz.
unsafe fn probe_poll(
    polls: &(AtomicU32, AtomicU32),
    element: impl Fn(u64, usize) -> Option<ProbeElement>,
    buf: *mut u8,
    cap: i32,
    len: *mut i32,
    t: *mut i64,
    seq: *mut i64,
) -> bool {
    let (tick_at, polled) = polls;
    let tick = edge_tick() as u32;
    if tick_at.swap(tick, Ordering::Relaxed) != tick {
        polled.store(0, Ordering::Relaxed);
    }
    let i = polled.fetch_add(1, Ordering::Relaxed) as usize;
    let Some((bytes, at, s)) = element(u64::from(tick), i) else { return false };
    let n = bytes.len().min(usize::try_from(cap).unwrap_or(0));
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, n);
        *len = n as i32;
        if let Some(at) = at {
            *t = at;
        }
        if let Some(s) = s {
            *seq = s;
        }
    }
    true
}

/// Ein Strom des Treiberrands: Bytes und Folgenummer, der Zeitstempel ist
/// die Tickgrenze.
fn edge_element(stream: edge_probe::Stream) -> impl Fn(u64, usize) -> Option<ProbeElement> {
    move |tick, i| edge_probe::element(stream, tick, i).map(|(bytes, s)| (bytes, None, Some(s)))
}

/// Der Treiber fuer `edge_u/rx`.
///
/// # Safety
///
/// `buf` zeigt auf `cap` schreibbare Bytes, die uebrigen auf je einen Platz.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_poll_edge_u_rx(
    buf: *mut u8,
    cap: i32,
    len: *mut i32,
    t: *mut i64,
    seq: *mut i64,
) -> bool {
    let stream = edge_probe::Stream::Lines;
    unsafe { probe_poll(&EDGE_POLLS[stream as usize], edge_element(stream), buf, cap, len, t, seq) }
}

/// Der Treiber fuer `edge_c/rx`.
///
/// # Safety
///
/// `buf` zeigt auf `cap` schreibbare Bytes, die uebrigen auf je einen Platz.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_poll_edge_c_rx(
    buf: *mut u8,
    cap: i32,
    len: *mut i32,
    t: *mut i64,
    seq: *mut i64,
) -> bool {
    let stream = edge_probe::Stream::Pairs;
    unsafe { probe_poll(&EDGE_POLLS[stream as usize], edge_element(stream), buf, cap, len, t, seq) }
}

/// Ein Strom, den `recorded.takt` nicht liest (8.2).
///
/// # Safety
///
/// `buf` zeigt auf `cap` schreibbare Bytes, die uebrigen auf je einen Platz.
unsafe fn unread_poll(
    stream: edge_probe::Unread,
    buf: *mut u8,
    cap: i32,
    len: *mut i32,
    t: *mut i64,
    seq: *mut i64,
) -> bool {
    let element = move |tick, i| edge_probe::unread(stream, tick, i);
    unsafe { probe_poll(&UNREAD_POLLS[stream as usize], element, buf, cap, len, t, seq) }
}

/// Der Treiber fuer `edge_r/frames`.
///
/// # Safety
///
/// `buf` zeigt auf `cap` schreibbare Bytes, die uebrigen auf je einen Platz.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_poll_edge_r_frames(
    buf: *mut u8,
    cap: i32,
    len: *mut i32,
    t: *mut i64,
    seq: *mut i64,
) -> bool {
    unsafe { unread_poll(edge_probe::Unread::Frames, buf, cap, len, t, seq) }
}

/// Der Treiber fuer `edge_r/text`.
///
/// # Safety
///
/// `buf` zeigt auf `cap` schreibbare Bytes, die uebrigen auf je einen Platz.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_poll_edge_r_text(
    buf: *mut u8,
    cap: i32,
    len: *mut i32,
    t: *mut i64,
    seq: *mut i64,
) -> bool {
    unsafe { unread_poll(edge_probe::Unread::Text, buf, cap, len, t, seq) }
}

/// Der Treiber fuer `edge_r/raw`.
///
/// # Safety
///
/// `buf` zeigt auf `cap` schreibbare Bytes, die uebrigen auf je einen Platz.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_poll_edge_r_raw(
    buf: *mut u8,
    cap: i32,
    len: *mut i32,
    t: *mut i64,
    seq: *mut i64,
) -> bool {
    unsafe { unread_poll(edge_probe::Unread::Raw, buf, cap, len, t, seq) }
}

/// Der Ausgang `edge_o/o`: bestaetigt, ausser wenn das Pruefgeraet es nicht tut.
#[unsafe(no_mangle)]
pub extern "C" fn takt_out_edge_o_o(_value: u8) -> bool {
    edge_probe::confirms(edge_tick())
}

/// Der Heartbeat des Geraets `edge_o`.
#[unsafe(no_mangle)]
pub extern "C" fn takt_alive_edge_o() -> bool {
    edge_probe::alive(edge_tick())
}

/// Der Treiber fuer `edge_r/level`, einen Kanal, den `recorded.takt` nicht
/// liest (8.2).
///
/// # Safety
///
/// Drei gueltige Zeiger des Rahmens.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_in_edge_r_level(value: *mut i32, quality: *mut u8, _t: *mut i64) -> bool {
    let Some((v, q)) = edge_probe::level(edge_tick()) else { return false };
    unsafe {
        *value = v;
        *quality = q;
    }
    true
}

/// Der Treiber fuer `edge_r/temp`.
///
/// # Safety
///
/// Drei gueltige Zeiger des Rahmens.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_in_edge_r_temp(value: *mut f64, _quality: *mut u8, t: *mut i64) -> bool {
    let (v, at) = edge_probe::temp(edge_tick());
    unsafe {
        *value = v;
        if let Some(at) = at {
            *t = at;
        }
    }
    true
}

/// Der Treiber fuer `edge_r/on`.
///
/// # Safety
///
/// Drei gueltige Zeiger des Rahmens.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_in_edge_r_on(value: *mut u8, _quality: *mut u8, _t: *mut i64) -> bool {
    unsafe { *value = u8::from(edge_probe::on(edge_tick())) };
    true
}

/// Der Treiber fuer `edge_r/mode`.
///
/// # Safety
///
/// Drei gueltige Zeiger des Rahmens.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_in_edge_r_mode(value: *mut u32, _quality: *mut u8, _t: *mut i64) -> bool {
    unsafe { *value = edge_probe::mode(edge_tick()) };
    true
}
