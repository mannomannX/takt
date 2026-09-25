//! Stack-Painting (12.3, 13.8): wie tief der Stack unter Last reicht.
//!
//! `takt bench` misst die Reserve fuer Runtime, Treiber und ISRs so, wie
//! 13.8 es vorschreibt: den freien Stack mit einem Muster fuellen, das
//! Programm laufen lassen, danach von unten suchen, bis wohin das Muster
//! ueberschrieben ist. Die Grenzen setzt `cortex-m-rt`: `_stack_start` ist
//! das obere Ende, `_stack_end` das untere.

use core::ptr::{read_volatile, write_volatile};

unsafe extern "C" {
    static _stack_start: u32;
    static _stack_end: u32;
}

/// Das Muster: kein plausibler Nutzwert, weder null noch `0xFFFF_FFFF`.
const PATTERN: u32 = 0xC5C5_C5C5;

/// Abstand unter dem aktuellen Stackzeiger, der beim Malen frei bleibt:
/// Das Malen selbst braucht Stack.
const MARGIN: usize = 256;

fn bounds() -> (usize, usize) {
    (&raw const _stack_end as usize, &raw const _stack_start as usize)
}

/// Fuellt den Stack vom unteren Ende bis kurz unter den Stackzeiger.
pub fn paint() {
    let (bottom, _) = bounds();
    let limit = (cortex_m::register::msp::read() as usize).saturating_sub(MARGIN) & !3;
    let mut at = bottom;
    while at < limit {
        // SAFETY: Das Wort liegt zwischen dem unteren Ende des Stacks und
        // dem Stackzeiger — Speicher, den niemand benutzt, bis der Stack
        // dorthin waechst.
        unsafe { write_volatile(at as *mut u32, PATTERN) };
        at += 4;
    }
}

/// Die groesste Tiefe des Stacks seit [`paint`] in Byte, vom oberen Ende.
pub fn high_water() -> u32 {
    let (bottom, top) = bounds();
    (top - deepest(bottom, top)) as u32
}

/// Der Stack-Bedarf eines Aufrufs in Byte (13.8: Stack-Bedarf je Native),
/// hoechstens `window`: Wer mehr braucht, meldet `window`.
///
/// Gemalt wird bis an den Stackzeiger, nicht bis kurz darunter wie in
/// [`paint`]: Die Interrupts sind gesperrt, also legt niemand sonst dort
/// einen Rahmen ab — und ein Aufruf, der weniger als den Abstand braucht,
/// bliebe sonst unsichtbar. Nur `window` Byte, damit die Sperre kurz
/// bleibt: Die Leitung sendet im Interrupt.
pub fn usage_of(window: usize, f: impl FnOnce()) -> u32 {
    cortex_m::interrupt::free(|_| {
        let sp = cortex_m::register::msp::read() as usize & !3;
        let bottom = sp.saturating_sub(window & !3).max(bounds().0);
        let mut at = bottom;
        while at < sp {
            // SAFETY: Unter dem Stackzeiger, bei gesperrten Interrupts:
            // Speicher, den bis zum Aufruf niemand benutzt.
            unsafe { write_volatile(at as *mut u32, PATTERN) };
            at += 4;
        }
        f();
        (sp - deepest(bottom, sp)) as u32
    })
}

/// Das unterste Wort zwischen `bottom` und `limit`, das nicht mehr das
/// Muster traegt; `limit`, wenn keines.
fn deepest(bottom: usize, limit: usize) -> usize {
    let mut at = bottom;
    // SAFETY: Lesen im Stackbereich; das Muster steht dort, wo der Stack
    // seit dem Malen nicht hinkam.
    while at < limit && unsafe { read_volatile(at as *const u32) } == PATTERN {
        at += 4;
    }
    at
}
