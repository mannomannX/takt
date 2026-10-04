//! Der Treiberrand fuer die erzeugten Rahmen (12.6, FB-285): `takt_edge_*`.
//!
//! **Derselbe Kern wie im Interpreter.** Die Regeln stehen ohne `std` in
//! `takt-hal`; der Rahmen haelt den Zustand je Kanal und Treiber in seinen
//! Feldern und ruft hier hinein. Satz 9.4.4 gilt fuer die Randfaelle nur,
//! wenn derselbe Code urteilt — eine Fassung in C waere eine zweite Quelle
//! fuer dieselben Faelle.

use core::slice;

use takt_hal::contract::{self, Delivery, Device, Event, Track, Window};
use takt_hal::quality::{Bounds, Gate, Quality, Reason, Scalar, Verdict};

/// Ein Feld des Rahmens als Slice; leer, wenn es keines gibt.
///
/// # Safety
///
/// Ist `n` positiv und `p` nicht null, zeigt `p` auf `n` Eintraege, die
/// waehrend des Aufrufs niemand sonst liest oder schreibt.
unsafe fn items<'a, T>(p: *mut T, n: u32) -> &'a mut [T] {
    if p.is_null() || n == 0 {
        return &mut [];
    }
    // SAFETY: vom Aufrufer zugesagt.
    unsafe { slice::from_raw_parts_mut(p, n as usize) }
}

/// Die Zeilen 1 und 2 fuer die Lieferungen eines Ticks
/// ([`contract::settle`]): `tracks` und `device_of` je Kanal, `devices` je
/// Treiber. Die Meldungen gehen nach `events`, hoechstens `max_events`;
/// das Ergebnis ist ihre Zahl.
///
/// # Safety
///
/// Jeder Zeiger zeigt auf so viele Eintraege, wie die Zahl daneben sagt
/// (`device_of` auf `channels`), oder ist null bei null Eintraegen.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments, reason = "die Felder des Rahmens, je mit Zeiger und Laenge")]
pub unsafe extern "C" fn takt_edge_settle(
    tracks: *mut Track,
    channels: u32,
    devices: *mut Device,
    n_devices: u32,
    device_of: *const u32,
    deliveries: *mut Delivery,
    n: u32,
    window: Window,
    events: *mut Event,
    max_events: u32,
) -> u32 {
    // SAFETY: vom Aufrufer zugesagt; `device_of` wird nur gelesen.
    let (tracks, devices, device_of, deliveries, events) = unsafe {
        (
            items(tracks, channels),
            items(devices, n_devices),
            items(device_of.cast_mut(), channels),
            items(deliveries, n),
            items(events, max_events),
        )
    };
    let mut k = 0;
    contract::settle(tracks, devices, device_of, deliveries, &window, |e| {
        if let Some(slot) = events.get_mut(k) {
            *slot = e;
            k += 1;
        }
    });
    k as u32
}

/// Ein Wert des Rahmens: eine Ganzzahl oder eine Fliesskommazahl, so wie
/// `Value` im Interpreter `Scalar` ist.
struct Number {
    int: bool,
    i: i64,
    f: f64,
}

impl Scalar for Number {
    fn as_f64(&self) -> Option<f64> {
        Some(if self.int { self.i as f64 } else { self.f })
    }

    fn as_i64(&self) -> Option<i64> {
        self.int.then_some(self.i)
    }
}

/// Ein Urteil als Zahl: die Qualitaet im untersten Byte (0 `Good` bis 3
/// `Bad`), der Grund im zweiten (0 `Stale` bis 3 `Driver`, 255 keiner), im
/// dritten eine 1, wenn der letzte gute Wert gehalten wird.
fn code(v: Verdict) -> u32 {
    let quality = match v.quality {
        Quality::Good => 0,
        Quality::Suspect => 1,
        Quality::Stale => 2,
        Quality::Bad => 3,
    };
    let reason = match v.reason {
        Some(Reason::Stale) => 0,
        Some(Reason::OutOfRange) => 1,
        Some(Reason::Implausible) => 2,
        Some(Reason::Driver) => 3,
        None => 255,
    };
    quality | (reason << 8) | (u32::from(v.held) << 16)
}

/// Prueft einen Wert gegen Range und `max_slew` (Zeilen 3, 4); `t` ist der
/// Zeitpunkt, den [`takt_edge_settle`] geliefert hat. Das Ergebnis siehe
/// [`code`].
///
/// # Safety
///
/// `gate` und `bounds` zeigen auf je einen Eintrag oder sind null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_edge_gate(
    gate: *mut Gate,
    bounds: *const Bounds,
    int: bool,
    i: i64,
    f: f64,
    t: i64,
) -> u32 {
    // SAFETY: vom Aufrufer zugesagt.
    let (Some(gate), Some(b)) = (unsafe { gate.as_mut() }, unsafe { bounds.as_ref() }) else { return 0 };
    code(gate.check(&Number { int, i, f }, t, &b.limits()))
}

/// Der Treiber eines Kanals ist degradiert (Zeile 2): Der Bezugspunkt
/// faellt weg.
///
/// # Safety
///
/// `gate` zeigt auf einen Eintrag oder ist null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_edge_driver_bad(gate: *mut Gate) {
    // SAFETY: vom Aufrufer zugesagt.
    if let Some(g) = unsafe { gate.as_mut() } {
        g.driver_bad();
    }
}

/// Die Ausgabeseite (Zeile 6): `true`, wenn der Besitzer `Runtime(Driver)`
/// bekommt — der Schreibvorgang unbestaetigt, der Heartbeat still oder
/// der Sendepuffer ueberfahren. Ein negativer freier Platz oder eine
/// negative Kapazitaet heisst unbekannt.
#[unsafe(no_mangle)]
pub extern "C" fn takt_edge_output(confirmed: bool, alive: bool, free: i32, capacity: i32) -> bool {
    let known = |n: i32| u32::try_from(n).ok();
    contract::output_fails(confirmed, alive, known(free), known(capacity))
}

/// Ob `bytes` ein Wert der Gestalt `shape` in kanonischer Form ist (Zeile 5,
/// `takt_native::bytes::decodes`); `exact` fuer ein Element, das genau so
/// lang ist, sonst fuer einen Slot mit Fuellung.
///
/// # Safety
///
/// `shape` zeigt auf `shape_len` Bytes, `bytes` auf `len` Bytes, oder sie
/// sind null bei null Bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_edge_decodes(
    shape: *const u8,
    shape_len: u32,
    bytes: *const u8,
    len: u32,
    exact: bool,
) -> bool {
    // SAFETY: vom Aufrufer zugesagt; beide werden nur gelesen.
    let (shape, bytes) = unsafe { (items(shape.cast_mut(), shape_len), items(bytes.cast_mut(), len)) };
    takt_native::bytes::decodes(shape, bytes, exact)
}
