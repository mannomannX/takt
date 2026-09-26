//! Telemetrie ueber USART1 (12.3): die Leitung hinter
//! [`takt_rt_baremetal::Telemetry`].
//!
//! 12.3 nennt „Telemetrie ueber UART/CAN/Ethernet, reduziert
//! (Zustandspfad, Faults, `pub var`)". Fuer den Bring-up ist das der
//! einzige Weg, vom Board etwas zu erfahren: Ohne Debugger sieht man
//! sonst nur, ob die LED blinkt.
//!
//! **Ohne zu warten** (12.2: „nie blockierend"; FB-277). Der USART hat kein
//! FIFO, nur ein Senderegister; wer auf `TXE` wartet, haelt den Tick an
//! (FB-227 auf dem C6). Darum nimmt [`Port::try_write`] ein Byte nur, wenn
//! im FIFO dieser Leitung Platz ist, und der Interrupt `USART1` gibt je
//! `TXE` eines an das Register weiter — ein Hardware-FIFO in Software. Den
//! Ring davor, das Zaehlen und das Verwerfen macht `takt-rt-baremetal` fuer
//! jedes Board gleich.
//!
//! **Die Gegenrichtung liest derselbe Interrupt.** Steht `TAKT` darin, gibt
//! die Anwendung das Board an den Host zurueck (`bootloader`, FB-275) —
//! derselbe Wunsch wie auf dem ESP32-C6. Wer erst beim naechsten Tick
//! nachsaehe, faende von vier Bytes am Stueck nur das letzte. Den Chip
//! zuruecksetzen darf der Interrupt nicht mitten in einem Schritt; er merkt
//! sich den Wunsch, und das naechste [`Port::flush`] fuehrt ihn aus.
//!
//! **XON/XOFF (FB-306).** Der Adapter hat keinen Rueckstau: Holt der Wirt
//! unter Last nicht rechtzeitig ab, laeuft ein Puffer ueber, und Bytes
//! fehlen (FB-304). Mit XON/XOFF sendet der Adapter `XOFF`, bevor das
//! geschieht, und `XON`, wenn wieder Platz ist; so lange haelt der
//! Interrupt das Senden an. Der Trace ist Text (grammar/trace.md), die
//! beiden Steuerzeichen kommen darin nicht vor, und der Erkenner sieht sie
//! nicht.
//!
//! Pins: PA9 (TX) und PA10 (RX), die Standardbelegung von USART1 auf
//! diesem Board.

use core::cell::Cell;
use core::sync::atomic::{AtomicBool, Ordering};

use cortex_m::interrupt::Mutex;
use stm32f4::stm32f401::{GPIOA, RCC, USART1};
use takt_board_support::ByteFifo;
use takt_board_support::console::Magic;
use takt_rt_baremetal::Port;

/// Die Rate der Leitung, fuer jedes Programm dieselbe: Der Host muss das
/// laufende erreichen, ohne zu wissen, welches es ist (FB-275). Der
/// FT232RL am anderen Ende und der USART treffen beide 923 077 Baud,
/// 0,16 Prozent ueber dem Nennwert.
pub const BAUD: u32 = 921_600;

/// Der Ring vor der Leitung: Er faengt die Buendel eines Ticks, die die
/// Leitung erst zwischen den Ticks abnimmt.
const RING: usize = 1024;

/// Was die Leitung ohne Warten annimmt: gut fuenf Millisekunden bei
/// 921600 Baud, mehr als ein Konformitaetslauf je Tick schreibt.
static TX: ByteFifo<512> = ByteFifo::new();

/// Der Host hat das Board zurueckverlangt; gesetzt vom Interrupt.
static HANDBACK: AtomicBool = AtomicBool::new(false);

/// Weitersenden und Anhalten (FB-306).
const XON: u8 = 0x11;
const XOFF: u8 = 0x13;

/// Der Adapter hat `XOFF` gesandt und noch kein `XON`.
static PAUSED: AtomicBool = AtomicBool::new(false);

/// Der Erkenner der Gegenrichtung; nur der Interrupt fuehrt ihn fort.
static RECOGNIZER: Mutex<Cell<Magic>> = Mutex::new(Cell::new(Magic::new()));

/// Die Leitung.
pub struct Usart1 {
    usart: USART1,
}

impl Usart1 {
    /// Richtet USART1 auf PA9/PA10 ein.
    ///
    /// `baud` ist die Zielbaudrate, `pclk_hz` der Takt des Busses, an dem
    /// USART1 haengt (APB2, auf diesem Board gleich dem Kerntakt).
    pub fn new(
        usart: USART1,
        gpioa: &GPIOA,
        rcc: &RCC,
        pclk_hz: u32,
        baud: u32,
    ) -> Result<Usart1, takt_board_support::uart::BaudError> {
        rcc.ahb1enr().modify(|_, w| w.gpioaen().set_bit());
        rcc.apb2enr().modify(|_, w| w.usart1en().set_bit());
        // Errata: zwei APB-Takte Verzoegerung, siehe lib.rs.
        let _ = rcc.apb2enr().read();

        // PA9 sendet, PA10 empfaengt. RX mit Pull-up: Ohne Gegenstelle
        // laege der Pin in der Luft und lieferte Rahmenfehler als Bytes.
        gpioa.moder().modify(|_, w| unsafe { w.moder9().bits(0b10).moder10().bits(0b10) });
        gpioa.afrh().modify(|_, w| unsafe { w.afrh9().bits(7).afrh10().bits(7) });
        gpioa.pupdr().modify(|_, w| unsafe { w.pupdr10().bits(0b01) });
        // Hohe Flankensteilheit: Bei 921600 Baud ist die Flanke ein
        // merklicher Teil des Bits.
        gpioa.ospeedr().modify(|_, w| unsafe { w.ospeedr9().bits(0b10) });

        // Der Teiler steht als Festkommazahl: obere 12 Bit ganzzahlig,
        // untere 4 als Sechzehntel. Die Rechnung liegt in
        // `takt-board-support`, wo sie getestet ist — und wo ihre erste
        // Fassung um den Faktor 16 danebenlag (FB-274).
        let brr = takt_board_support::uart::divisor(pclk_hz, baud)?;
        usart.brr().write(|w| unsafe { w.bits(brr) });
        usart.cr1().modify(|_, w| {
            w.te().set_bit();
            w.re().set_bit();
            w.rxneie().set_bit();
            w.ue().set_bit()
        });

        Ok(Usart1 { usart })
    }
}

/// Vom Interrupt-Handler `USART1` des Programms zu rufen.
///
/// Empfangen zuerst, damit ein `XOFF` vor dem naechsten Byte wirkt: `XON`
/// und `XOFF` schalten das Senden, alles andere geht in den Erkenner. `SR`
/// vor `DR` zu lesen loescht auch einen Ueberlauf (RM0368, `ORE`), der sonst
/// den Interrupt stehen liesse; die Schranke haelt einen Sturm von
/// Rahmenfehlern auf (4.1, von Hand). Senden: je `TXE` ein Byte aus dem
/// FIFO; ist er leer oder die Leitung angehalten, schweigt der
/// Sende-Interrupt, bis [`Port::flush`] oder `XON` ihn wieder weckt.
pub fn on_interrupt() {
    // SAFETY: Der Interrupt besitzt den Sendeweg ab dem FIFO und das
    // Empfangsregister; das Programm schreibt nur `TXEIE` (unter
    // `interrupt::free`) und nie `DR`.
    let usart = unsafe { &*USART1::ptr() };
    cortex_m::interrupt::free(|cs| {
        let cell = RECOGNIZER.borrow(cs);
        let mut magic = cell.get();
        for _ in 0..4 {
            let sr = usart.sr().read();
            if sr.rxne().bit_is_clear() && sr.ore().bit_is_clear() {
                break;
            }
            match usart.dr().read().dr().bits() as u8 {
                XOFF => PAUSED.store(true, Ordering::Relaxed),
                XON => {
                    PAUSED.store(false, Ordering::Relaxed);
                    usart.cr1().modify(|_, w| w.txeie().set_bit());
                }
                b => {
                    if magic.feed(b) {
                        HANDBACK.store(true, Ordering::Relaxed);
                    }
                }
            }
        }
        cell.set(magic);
    });
    if usart.cr1().read().txeie().bit_is_set() && usart.sr().read().txe().bit_is_set() {
        match if PAUSED.load(Ordering::Relaxed) { None } else { TX.pop() } {
            Some(b) => {
                usart.dr().write(|w| unsafe { w.dr().bits(u16::from(b)) });
            }
            None => {
                usart.cr1().modify(|_, w| w.txeie().clear_bit());
            }
        }
    }
}

impl Port for Usart1 {
    fn try_write(&mut self, b: u8) -> bool {
        TX.push(b)
    }

    /// Weckt den Sende-Interrupt und fuehrt einen Wunsch des Hosts aus.
    fn flush(&mut self) {
        if HANDBACK.load(Ordering::Relaxed) {
            crate::bootloader::request();
        }
        if !TX.is_empty() && !PAUSED.load(Ordering::Relaxed) {
            cortex_m::interrupt::free(|_| self.usart.cr1().modify(|_, w| w.txeie().set_bit()));
        }
    }

    /// Der FIFO ist leer, und `TC` meldet auch das letzte Stoppbit gesendet.
    fn idle(&mut self) -> bool {
        TX.is_empty() && self.usart.sr().read().tc().bit_is_set()
    }
}

/// Die Telemetrie des Boards.
pub type Telemetry = takt_rt_baremetal::Telemetry<Usart1, RING>;

/// Die Telemetrie ueber USART1 auf PA9, mit der Gegenrichtung auf PA10.
///
/// Das Programm stellt den Interrupt-Handler `USART1`, der
/// [`on_interrupt`] ruft, und gibt den Interrupt frei; sonst sendet die
/// Leitung nicht und hoert den Host nicht.
pub fn telemetry(
    usart: USART1,
    gpioa: &GPIOA,
    rcc: &RCC,
    pclk_hz: u32,
    baud: u32,
) -> Result<Telemetry, takt_board_support::uart::BaudError> {
    Ok(Telemetry::new(Usart1::new(usart, gpioa, rcc, pclk_hz, baud)?))
}
