/* line.c - die Leitung auf dem USB-Serial-JTAG des C6 (line.h).
 *
 * **Der Vertrag der Leitung** ist der des Bring-ups in Rust
 * (`crates/takt-board-esp32c6/src/uart.rs`; FB-264, FB-267, FB-272, FB-311):
 * Ein Byte geht nur ins FIFO, solange `serial_in_ep_data_free` steht. Ein
 * Paket bleibt unter 64 Byte und geht immer mit `wr_done` hinaus, denn ein
 * volles Paket ist fuer den Host eine unvollstaendige Transaktion. Nach dem
 * Abschicken nimmt die Leitung erst wieder ein Byte, wenn das Rohflag
 * `serial_in_empty` das Abholen meldet; `serial_in_ep_data_free` allein
 * trog unter Last. Nach einem Bus-Reset gibt erst ein `wr_done` beim
 * naechsten IN-Token den Endpunkt frei.
 *
 * Ohne Interrupt: Das Rohflag liest die Leitung bei jedem Aufruf selbst.
 * Ein Interrupt, der es quittierte, nahme ihr das Abholen; eine ISR, die
 * aus einem eigenen FIFO nachschiebt, verstummte, sobald es leer lief und
 * kein Paket mehr unterwegs war, das den naechsten Interrupt ausloest
 * (FB-842).
 */
#include "line.h"

#include <stdint.h>

#include "esp_attr.h"
#include "hal/usb_serial_jtag_ll.h"
#include "hal/wdt_hal.h"
#include "soc/lp_aon_reg.h"
#include "soc/soc.h"

/* Hoechstens so viel geht je Paket an den Host: kuerzer als sein FIFO von
 * 64 Byte (FB-264, FB-267). */
#define LINE_PACKET 63U

/* Das Wort, mit dem der Host das Board zurueckverlangt, in der Konsole und
 * als Wert in LP_AON STORE0 (`REENUMERATE_MAGIC` des Harness). */
#define LINE_MAGIC 0x54414B54U

/* So viele Takte des RTC wartet der Watchdog vor dem Reset: unter einer
 * Millisekunde am RC-Takt, wenige am 32-kHz-Quarz. */
#define LINE_RESET_TICKS 100U

/* Bytes im FIFO seit dem letzten Abschicken. Die Leitung gehoert einem
 * Kontext zugleich (`takt_sdk.h`): im Lauf dem Schritt, sonst `app_main`. */
static uint32_t line_filled;

/* Abgeschickt und vom Host noch nicht abgeholt. */
static uint8_t line_in_flight;

/* Seit einem Bus-Reset, bis der Host wieder abfragt (FB-311). */
static uint8_t line_stalled;

/* Setzt den Chip auf RTC-Ebene zurueck, wie der EN-Pin: Auch der
 * USB-Serial-JTAG faengt neu an, und der Host sieht ein Abstecken. Ein
 * Neustart von ESP-IDF (`esp_restart`) setzte nur das HP-System zurueck
 * (FB-843). */
static IRAM_ATTR void line_chip_reset(void) {
    wdt_hal_context_t rwdt = RWDT_HAL_CONTEXT_DEFAULT();
    wdt_hal_init(&rwdt, WDT_RWDT, 0U, false);
    wdt_hal_write_protect_disable(&rwdt);
    wdt_hal_config_stage(&rwdt, WDT_STAGE0, LINE_RESET_TICKS, WDT_STAGE_ACTION_RESET_RTC);
    wdt_hal_enable(&rwdt);
    wdt_hal_write_protect_enable(&rwdt);
    for (;;) {
        /* Der Watchdog setzt den Chip zurueck. */
    }
}

/* Liest die Gegenrichtung leer, hoechstens ein Paket, damit ein ungelesenes
 * Paket den Host nicht blockiert; steht `TAKT` darin, setzt sich der Chip
 * zurueck (FB-266). */
static IRAM_ATTR void line_receive(void) {
    /* Wie viele Bytes von `TAKT` zuletzt am Stueck kamen. */
    static uint32_t matched;
    static const uint8_t word[4] = {(uint8_t)'T', (uint8_t)'A', (uint8_t)'K', (uint8_t)'T'};
    uint8_t bytes[LINE_PACKET + 1U];
    const int n = usb_serial_jtag_ll_read_rxfifo(bytes, sizeof bytes);
    for (int i = 0; i < n; i++) {
        if (bytes[i] == word[matched]) {
            matched++;
        } else {
            matched = (bytes[i] == word[0]) ? 1U : 0U;
        }
        if (matched == sizeof word) {
            line_chip_reset();
        }
    }
}

/* Nimmt das Abholen zur Kenntnis: Das FIFO ist wieder frei. */
static IRAM_ATTR void line_settle(void) {
    const uint32_t raw = usb_serial_jtag_ll_get_intraw_mask();
    if ((raw & (uint32_t)USB_SERIAL_JTAG_INTR_BUS_RESET) != 0U) {
        usb_serial_jtag_ll_clr_intsts_mask((uint32_t)USB_SERIAL_JTAG_INTR_BUS_RESET |
                                           (uint32_t)USB_SERIAL_JTAG_INTR_TOKEN_REC_IN_EP1);
        line_filled = 0U;
        line_in_flight = 0U;
        line_stalled = 1U;
    } else if ((line_stalled != 0U) && ((raw & (uint32_t)USB_SERIAL_JTAG_INTR_TOKEN_REC_IN_EP1) != 0U)) {
        line_stalled = 0U;
        if (usb_serial_jtag_ll_txfifo_writable() == 0) {
            usb_serial_jtag_ll_clr_intsts_mask((uint32_t)USB_SERIAL_JTAG_INTR_SERIAL_IN_EMPTY);
            usb_serial_jtag_ll_txfifo_flush();
            line_in_flight = 1U;
        }
    } else if ((line_in_flight != 0U) && ((raw & (uint32_t)USB_SERIAL_JTAG_INTR_SERIAL_IN_EMPTY) != 0U)) {
        usb_serial_jtag_ll_clr_intsts_mask((uint32_t)USB_SERIAL_JTAG_INTR_SERIAL_IN_EMPTY);
        line_in_flight = 0U;
    } else {
        /* Nichts abgeholt. */
    }
}

/* Ein Byte, wenn das FIFO es nimmt, ohne zu warten. */
static IRAM_ATTR uint8_t line_try_write(void *ctx, uint8_t byte) {
    (void)ctx;
    uint8_t taken = 0U;
    line_settle();
    if ((line_in_flight == 0U) && (line_filled < LINE_PACKET) && (usb_serial_jtag_ll_txfifo_writable() != 0) &&
        (usb_serial_jtag_ll_write_txfifo(&byte, 1U) == 1)) {
        line_filled++;
        taken = 1U;
    }
    return taken;
}

/* Schickt das Paket ab, wenn eines gefuellt ist und keines unterwegs. */
static IRAM_ATTR void line_flush(void *ctx) {
    (void)ctx;
    line_settle();
    line_receive();
    if ((line_filled != 0U) && (line_in_flight == 0U)) {
        usb_serial_jtag_ll_clr_intsts_mask((uint32_t)USB_SERIAL_JTAG_INTR_SERIAL_IN_EMPTY);
        line_in_flight = 1U;
        line_filled = 0U;
        usb_serial_jtag_ll_txfifo_flush();
    }
}

void line_open(struct takt_line *line) {
    /* Den Wunsch nach einem Chip-Reset schreibt der Host ueber JTAG in
     * LP_AON STORE0, das einen Reset ueberlebt und das ESP-IDF auf dem C6
     * nicht nutzt, oder als `TAKT` in die Konsole: Der Start liest beides
     * vor allem anderen. */
    if (REG_READ(LP_AON_STORE0_REG) == LINE_MAGIC) {
        REG_WRITE(LP_AON_STORE0_REG, 0U);
        line_chip_reset();
    }
    line_receive();
    /* Nur, was die Leitung selbst liest: kein Interrupt des USB-Serial-JTAG. */
    usb_serial_jtag_ll_disable_intr_mask(USB_SERIAL_JTAG_LL_INTR_MASK);
    line->ctx = NULL;
    line->try_write = line_try_write;
    line->flush = line_flush;
}
