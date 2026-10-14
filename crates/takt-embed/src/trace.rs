//! Die Senke des Trace (12.5, 12.11): Wohin ein Programm seine Zeilen
//! schreibt.
//!
//! **Eine Senke je Programm und Instanz.** Der Rahmen schreibt seinen Trace
//! stueckweise ueber die geteilten Haken `takt_trace_*` und gibt jedem
//! Aufruf die Senke mit, die der Wirt der Huelle bei `new` gab. Die Haken
//! haben keinen Zustand; den haelt die Senke. Zwei Programme oder zwei
//! Instanzen desselben Programms schreiben so nie ineinander, auch wenn
//! eines das andere unterbricht.
//!
//! **Die Haken stellt der Wirt einmal** ([`hooks!`]), fuer die eine Art von
//! Senke, die er seinen Programmen gibt ([`Sink`]). Sie rufen deren
//! [`Line::write`] direkt, ohne Tabelle: So sieht `takt check-image` den
//! ganzen Weg vom Tick bis in die Senke des Wirts und prueft unter
//! `xip_flash`, dass er im RAM liegt (12.3).
//!
//! **Was ein Haken schreibt.** Genau das Stueck, ohne Trenner: Leerzeichen
//! und Zeilenende schreibt der Rahmen selbst (`grammar/trace.md` T1,
//! FB-690). Eine Fliesskommazahl steht in der Textform aus T2, wie der
//! Interpreter sie schreibt (FB-750), ein Byte als `0x` und zwei Ziffern. Ganzzahlen und Bytes gehen ohne `core::fmt` ueber
//! `takt_rt_core::digits`, wie auf der Leitung der Boards.

#![allow(unsafe_code, reason = "die Haken des Rahmens als C-Funktionen (12.5); 9.5 fuehrt sie in der TCB")]

use core::ffi::{CStr, c_char, c_void};

use takt_rt_core::digits;

/// Was eine Senke mit den Stuecken des Trace tut: eine Leitung, ein Ring,
/// ein Puffer im Test.
///
/// `write` nimmt `&self`: Der Wirt darf die Senke lesen, solange das
/// Programm sie haelt, etwa zwischen zwei Ticks. Er sagt zu, dass nie zwei
/// Kontexte zugleich in dieselbe Senke schreiben.
pub trait Line {
    /// Nimmt ein Stueck einer Zeile.
    fn write(&self, bytes: &[u8]);
}

/// Die Senke, die ein Wirt seinen Programmen gibt (`Program::new`, 12.11):
/// die eine Art von [`Line`], fuer die er die Haken stellt.
///
/// # Safety
///
/// Nur [`hooks!`] erfuellt diesen Trait: Die Haken eines Abbilds lesen jeden
/// Zeiger auf eine Senke als diese eine Art, und ein zweiter Aufruf bindet
/// nicht, weil die Haken dann doppelt staenden.
pub unsafe trait Sink: Line {}

/// Der Zeiger, den der Rahmen bei `init` bekommt und jedem Haken gibt.
/// Geschrieben wird durch ihn nie; die Haken lesen ihn als `&S`.
pub fn as_ffi<S: Sink>(sink: &S) -> *mut c_void {
    core::ptr::from_ref(sink).cast_mut().cast()
}

/// Die Senke hinter dem Zeiger eines Hakens.
///
/// # Safety
///
/// `sink` ist null oder kommt aus [`as_ffi`] einer Senke der Art `L`, die
/// noch lebt: Die Huelle leiht sie fuer die Lebenszeit des Programms.
unsafe fn line<'s, L: Line>(sink: *mut c_void) -> Option<&'s L> {
    // SAFETY: siehe oben.
    unsafe { sink.cast_const().cast::<L>().as_ref() }
}

/// Schreibt Formatiertes in eine Senke.
struct Fmt<'s, L: Line>(&'s L);

impl<L: Line> core::fmt::Write for Fmt<'_, L> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.0.write(s.as_bytes());
        Ok(())
    }
}

/// Ein Text im Trace, fuer `takt_trace_text`.
///
/// # Safety
///
/// `sink` wie bei [`line`]; `text` ist null oder nullterminiert und gilt
/// waehrend des Aufrufs.
pub unsafe fn text<L: Line>(sink: *mut c_void, text: *const c_char) {
    // SAFETY: siehe oben.
    let Some(line) = (unsafe { line::<L>(sink) }) else { return };
    if text.is_null() {
        return;
    }
    // SAFETY: nicht null und nullterminiert, siehe oben.
    line.write(unsafe { CStr::from_ptr(text) }.to_bytes());
}

/// Eine Zahl im Trace, fuer `takt_trace_i64`.
///
/// # Safety
///
/// `sink` wie bei [`line`].
pub unsafe fn signed<L: Line>(sink: *mut c_void, value: i64) {
    // SAFETY: siehe oben.
    if let Some(line) = unsafe { line::<L>(sink) } {
        if value < 0 {
            line.write(b"-");
        }
        let mut buf = [0u8; 20];
        line.write(digits::decimal(value.unsigned_abs(), &mut buf));
    }
}

/// Eine Zahl ohne Vorzeichen im Trace, fuer `takt_trace_u64`.
///
/// # Safety
///
/// `sink` wie bei [`line`].
pub unsafe fn unsigned<L: Line>(sink: *mut c_void, value: u64) {
    // SAFETY: siehe oben.
    if let Some(line) = unsafe { line::<L>(sink) } {
        let mut buf = [0u8; 20];
        line.write(digits::decimal(value, &mut buf));
    }
}

/// Eine Fliesskommazahl in der Textform aus T2 (4.2, 3.9), wie der
/// Interpreter sie schreibt (`takt_rt_core::digits::float64`), fuer
/// `takt_trace_f64`.
///
/// # Safety
///
/// `sink` wie bei [`line`].
pub unsafe fn float<L: Line>(sink: *mut c_void, value: f64) {
    // SAFETY: siehe oben.
    if let Some(line) = unsafe { line::<L>(sink) } {
        let _ = digits::float64(value, &mut Fmt(line));
    }
}

/// Ein Byte eines Ausgabestroms, wie der Interpreter es schreibt, fuer
/// `takt_trace_hex8`.
///
/// # Safety
///
/// `sink` wie bei [`line`].
pub unsafe fn byte<L: Line>(sink: *mut c_void, value: u8) {
    // SAFETY: siehe oben.
    if let Some(line) = unsafe { line::<L>(sink) } {
        line.write(&[b'0', b'x', digits::hex(value >> 4), digits::hex(value & 15)]);
    }
}

/// Die Haken des Rahmens (`takt_trace_*`, 12.5) fuer die Senke `$sink`, die
/// der Wirt jedem seiner Programme gibt; einmal je Abbild.
///
/// ```ignore
/// struct Uart;
/// impl takt_embed::Line for Uart {
///     fn write(&self, bytes: &[u8]) { /* auf die Leitung */ }
/// }
/// takt_embed::hooks!(Uart);
/// let program = app::Program::init(&mut arena, &mut rig, &Uart);
/// ```
#[macro_export]
macro_rules! hooks {
    ($sink:ty) => {
        // SAFETY: Diese Haken lesen jede Senke als `$sink`; ein zweiter
        // Aufruf im selben Abbild bindet nicht.
        #[allow(unsafe_code, reason = "die eine Senke der Haken (12.5)")]
        unsafe impl $crate::trace::Sink for $sink {}

        /// Vom Rahmen gerufen: ein Text im Trace.
        ///
        /// # Safety
        ///
        /// `sink` kommt aus `init`, `text` ist null oder nullterminiert.
        #[unsafe(no_mangle)]
        #[allow(unsafe_code, reason = "die Haken des Rahmens als C-Funktionen (12.5)")]
        pub unsafe extern "C" fn takt_trace_text(sink: *mut core::ffi::c_void, text: *const core::ffi::c_char) {
            // SAFETY: siehe oben.
            unsafe { $crate::trace::text::<$sink>(sink, text) }
        }

        /// Vom Rahmen gerufen: eine Zahl im Trace.
        ///
        /// # Safety
        ///
        /// `sink` kommt aus `init`.
        #[unsafe(no_mangle)]
        #[allow(unsafe_code, reason = "die Haken des Rahmens als C-Funktionen (12.5)")]
        pub unsafe extern "C" fn takt_trace_i64(sink: *mut core::ffi::c_void, value: i64) {
            // SAFETY: siehe oben.
            unsafe { $crate::trace::signed::<$sink>(sink, value) }
        }

        /// Vom Rahmen gerufen: eine Zahl ohne Vorzeichen im Trace.
        ///
        /// # Safety
        ///
        /// `sink` kommt aus `init`.
        #[unsafe(no_mangle)]
        #[allow(unsafe_code, reason = "die Haken des Rahmens als C-Funktionen (12.5)")]
        pub unsafe extern "C" fn takt_trace_u64(sink: *mut core::ffi::c_void, value: u64) {
            // SAFETY: siehe oben.
            unsafe { $crate::trace::unsigned::<$sink>(sink, value) }
        }

        /// Vom Rahmen gerufen: eine Fliesskommazahl im Trace (4.2).
        ///
        /// # Safety
        ///
        /// `sink` kommt aus `init`.
        #[unsafe(no_mangle)]
        #[allow(unsafe_code, reason = "die Haken des Rahmens als C-Funktionen (12.5)")]
        pub unsafe extern "C" fn takt_trace_f64(sink: *mut core::ffi::c_void, value: f64) {
            // SAFETY: siehe oben.
            unsafe { $crate::trace::float::<$sink>(sink, value) }
        }

        /// Vom Rahmen gerufen: ein Byte eines Ausgabestroms im Trace.
        ///
        /// # Safety
        ///
        /// `sink` kommt aus `init`.
        #[unsafe(no_mangle)]
        #[allow(unsafe_code, reason = "die Haken des Rahmens als C-Funktionen (12.5)")]
        pub unsafe extern "C" fn takt_trace_hex8(sink: *mut core::ffi::c_void, value: u8) {
            // SAFETY: siehe oben.
            unsafe { $crate::trace::byte::<$sink>(sink, value) }
        }
    };
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use core::cell::RefCell;
    use std::string::String;
    use std::vec::Vec;

    #[derive(Default)]
    struct Text(RefCell<Vec<u8>>);

    impl Line for Text {
        fn write(&self, bytes: &[u8]) {
            self.0.borrow_mut().extend_from_slice(bytes);
        }
    }

    impl Text {
        fn text(&self) -> String {
            String::from_utf8(self.0.borrow().clone()).expect("UTF-8")
        }

        fn ffi(&self) -> *mut c_void {
            core::ptr::from_ref(self).cast_mut().cast()
        }
    }

    /// **Ein Haken haengt nichts an** (FB-690): Die Trenner schreibt der
    /// Rahmen, eine Zahl steht ohne Leerzeichen dahinter.
    #[test]
    fn a_hook_writes_exactly_its_piece() {
        let t = Text::default();
        // SAFETY: die Senke lebt waehrend der Aufrufe; die Texte sind nullterminiert.
        unsafe {
            text::<Text>(t.ffi(), c"t=".as_ptr());
            signed::<Text>(t.ffi(), -3);
            text::<Text>(t.ffi(), c" out x [".as_ptr());
            unsigned::<Text>(t.ffi(), u64::MAX);
            text::<Text>(t.ffi(), c", ".as_ptr());
            float::<Text>(t.ffi(), 2.5);
            text::<Text>(t.ffi(), c", ".as_ptr());
            float::<Text>(t.ffi(), 3.0);
            text::<Text>(t.ffi(), c", ".as_ptr());
            float::<Text>(t.ffi(), 1e30);
            text::<Text>(t.ffi(), c", ".as_ptr());
            byte::<Text>(t.ffi(), 0x0a);
            text::<Text>(t.ffi(), c", ".as_ptr());
            signed::<Text>(t.ffi(), i64::MIN);
            text::<Text>(t.ffi(), c"]\n".as_ptr());
        }
        assert_eq!(t.text(), "t=-3 out x [18446744073709551615, 2.5, 3.0, 1.0e30, 0x0a, -9223372036854775808]\n");
    }

    /// **Zwei Senken, zwei Traces** (FB-691): Was ein Programm schreibt,
    /// landet nur in seiner Senke.
    #[test]
    fn two_sinks_keep_their_lines_apart() {
        let (a, b) = (Text::default(), Text::default());
        // SAFETY: wie oben.
        unsafe {
            text::<Text>(a.ffi(), c"t=1".as_ptr());
            text::<Text>(b.ffi(), c"t=2".as_ptr());
            text::<Text>(a.ffi(), c"\n".as_ptr());
        }
        assert_eq!((a.text().as_str(), b.text().as_str()), ("t=1\n", "t=2"));
    }

    /// **Ein langer Text kommt ganz an** (RT-038): Eine still gekuerzte Zeile
    /// waere ein Trace, der vom Interpreter abweicht, ohne es zu sagen.
    #[test]
    fn a_long_text_is_not_cut() {
        let long: Vec<u8> = (0..300).map(|i| b"abcdefghijklmnopqrstuvwxyz"[i % 26]).chain([0]).collect();
        let t = Text::default();
        // SAFETY: wie oben.
        unsafe { text::<Text>(t.ffi(), long.as_ptr().cast()) };
        assert_eq!(t.text().as_bytes(), &long[..300]);
    }

    /// Ohne Senke schreibt kein Haken, und ein Text ohne Zeiger ist keiner.
    #[test]
    fn a_null_sink_or_text_writes_nothing() {
        let t = Text::default();
        // SAFETY: null ist erlaubt.
        unsafe {
            text::<Text>(core::ptr::null_mut(), c"x".as_ptr());
            signed::<Text>(core::ptr::null_mut(), 1);
            text::<Text>(t.ffi(), core::ptr::null());
        }
        assert_eq!(t.text(), "");
    }
}
