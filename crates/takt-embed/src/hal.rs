//! Ein Pin aus `embedded-hal` 1.0 als Geraet (2.7 in plan/m11.md, 12.6;
//! Merkmal `hal`): ein Treiberobjekt besteht dann aus den Pins seines HAL,
//! ohne eigenen Treibercode.

use embedded_hal::digital::{InputPin, OutputPin, PinState};

use crate::{Input, Output, Quality, Sample};

/// Ein Pin als Eingang oder Ausgang mit Rohtyp `bool` (8.10): `true` ist
/// der hohe Pegel; eine Polaritaet kehrt der HAL um, nicht der Adapter.
#[derive(Debug, Default)]
pub struct Pin<P>(pub P);

impl<P: InputPin> Input<bool> for Pin<P> {
    /// Der Pegel zur Tickgrenze; scheitert das Lesen, ist die Abtastung
    /// `Bad` (12.6 Zeile 2).
    fn sample(&mut self, now: i64) -> Option<Sample<bool>> {
        Some(match self.0.is_high() {
            Ok(high) => Sample::good(high, now),
            Err(_) => Sample { value: false, quality: Quality::Bad, t: now },
        })
    }
}

impl<P: OutputPin> Output<bool> for Pin<P> {
    /// Stellt den Pegel; bestaetigt, wenn der HAL ihn gesetzt hat (12.6
    /// Zeile 6).
    fn write(&mut self, value: bool, _now: i64) -> bool {
        self.0.set_state(PinState::from(value)).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::Pin;
    use crate::{Input, Output, Quality};
    use core::convert::Infallible;
    use embedded_hal::digital::{Error, ErrorKind, ErrorType, InputPin, OutputPin};

    /// Ein nachgebauter Pin: ein Pegel und ob das Lesen scheitert.
    #[derive(Default)]
    struct Fake {
        high: bool,
        broken: bool,
    }

    #[derive(Debug)]
    struct Broken;

    impl Error for Broken {
        fn kind(&self) -> ErrorKind {
            ErrorKind::Other
        }
    }

    impl ErrorType for Fake {
        type Error = Broken;
    }

    impl InputPin for Fake {
        fn is_high(&mut self) -> Result<bool, Broken> {
            if self.broken { Err(Broken) } else { Ok(self.high) }
        }

        fn is_low(&mut self) -> Result<bool, Broken> {
            self.is_high().map(|h| !h)
        }
    }

    impl OutputPin for Fake {
        fn set_low(&mut self) -> Result<(), Broken> {
            if self.broken {
                return Err(Broken);
            }
            self.high = false;
            Ok(())
        }

        fn set_high(&mut self) -> Result<(), Broken> {
            if self.broken {
                return Err(Broken);
            }
            self.high = true;
            Ok(())
        }
    }

    #[test]
    fn a_pin_samples_its_level_at_the_tick() {
        let mut pin = Pin(Fake { high: true, broken: false });
        let s = pin.sample(1_000).expect("Abtastung");
        assert_eq!((s.value, s.quality, s.t), (true, Quality::Good, 1_000));
    }

    /// Ein Lesefehler des HAL ist eine Abtastung ohne Wert (12.6 Zeile 2).
    #[test]
    fn a_failed_read_is_bad() {
        let mut pin = Pin(Fake { high: true, broken: true });
        assert_eq!(pin.sample(5).map(|s| s.quality), Some(Quality::Bad));
    }

    #[test]
    fn a_pin_confirms_what_it_set() {
        let mut pin = Pin(Fake::default());
        assert!(Output::write(&mut pin, true, 0) && pin.0.high);
        assert!(Output::write(&mut pin, false, 0) && !pin.0.high);
        pin.0.broken = true;
        assert!(!Output::write(&mut pin, true, 0), "ein gescheiterter Schreibvorgang ist unbestaetigt");
    }

    /// Ein Pin, dessen HAL nie scheitert.
    #[test]
    fn an_infallible_pin_is_always_good() {
        struct Always;
        impl ErrorType for Always {
            type Error = Infallible;
        }
        impl InputPin for Always {
            fn is_high(&mut self) -> Result<bool, Infallible> {
                Ok(false)
            }
            fn is_low(&mut self) -> Result<bool, Infallible> {
                Ok(true)
            }
        }
        assert_eq!(Pin(Always).sample(0).map(|s| s.quality), Some(Quality::Good));
    }
}
