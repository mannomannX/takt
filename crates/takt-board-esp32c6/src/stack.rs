//! Stack-Painting (12.3, 13.8): wie tief der Stack unter Last reicht.
//!
//! Wie auf dem F401: den freien Stack mit einem Muster fuellen, das
//! Programm laufen lassen, danach von unten suchen, bis wohin das Muster
//! ueberschrieben ist. Die Grenzen setzt `esp-hal`: `_stack_start_cpu0`
//! ist das obere Ende. Am unteren liegt sein Waechterwort
//! `__stack_chk_guard`, das ein Daten-Watchpoint bewacht — gemalt wird erst
//! darueber, sonst meldete der Chip einen Ueberlauf, den es nicht gab.

use core::ptr::{read_volatile, with_exposed_provenance, with_exposed_provenance_mut, write_volatile};

unsafe extern "C" {
    static _stack_start_cpu0: u32;
    static __stack_chk_guard: u32;
}

/// Das Muster: kein plausibler Nutzwert, weder null noch `0xFFFF_FFFF`.
pub(crate) const PATTERN: u32 = 0xC5C5_C5C5;

/// Abstand unter dem aktuellen Stackzeiger, der beim Malen frei bleibt:
/// Das Malen selbst braucht Stack.
const MARGIN: usize = 256;

fn bounds() -> (usize, usize) {
    // Das Wort hinter dem Waechterwort.
    let above = (&raw const __stack_chk_guard).wrapping_add(1);
    (above.expose_provenance(), (&raw const _stack_start_cpu0).expose_provenance())
}

fn stack_pointer() -> usize {
    let sp: usize;
    // SAFETY: liest nur das Register `sp`.
    unsafe { core::arch::asm!("mv {0}, sp", out(reg) sp, options(nomem, nostack)) };
    sp
}

/// Fuellt den Stack ueber dem Waechterwort bis kurz unter den Stackzeiger.
pub fn paint() {
    let (bottom, _) = bounds();
    let limit = stack_pointer().saturating_sub(MARGIN) & !3;
    for at in (bottom..limit).step_by(4) {
        // SAFETY: Das Wort liegt zwischen dem Waechterwort und dem
        // Stackzeiger — Speicher, den niemand benutzt, bis der Stack
        // dorthin waechst.
        unsafe { write_volatile(with_exposed_provenance_mut::<u32>(at), PATTERN) };
    }
}

/// Die groesste Tiefe des Stacks seit [`paint`] in Byte, vom oberen Ende.
pub fn high_water() -> u32 {
    let (bottom, top) = bounds();
    depth(bottom, top)
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
    critical_section::with(|_| {
        let sp = stack_pointer() & !3;
        let bottom = sp.saturating_sub(window & !3).max(bounds().0);
        for at in (bottom..sp).step_by(4) {
            // SAFETY: Unter dem Stackzeiger und ueber dem Waechterwort, bei
            // gesperrten Interrupts: Speicher, den bis zum Aufruf niemand
            // benutzt.
            unsafe { write_volatile(with_exposed_provenance_mut::<u32>(at), PATTERN) };
        }
        f();
        depth(bottom, sp)
    })
}

/// Die Tiefe eines gemalten Stacks `[bottom, top)` in Byte, vom oberen Ende.
pub(crate) fn depth(bottom: usize, top: usize) -> u32 {
    crate::word(top.saturating_sub(deepest(bottom, top)))
}

/// Das unterste Wort zwischen `bottom` und `limit`, das nicht mehr das
/// Muster traegt; `limit`, wenn keines.
fn deepest(bottom: usize, limit: usize) -> usize {
    let painted = |at: usize| {
        // SAFETY: Lesen im Stackbereich; das Muster steht dort, wo der Stack
        // seit dem Malen nicht hinkam.
        unsafe { read_volatile(with_exposed_provenance::<u32>(at)) == PATTERN }
    };
    (bottom..limit).step_by(4).find(|&at| !painted(at)).unwrap_or(limit)
}
