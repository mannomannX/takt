/* line.c - die Leitung auf dem USB-Serial-JTAG des C6 (line.h). */
#include "line.h"

#include <stdint.h>

#include "esp_attr.h"
#include "esp_intr_alloc.h"
#include "hal/usb_serial_jtag_ll.h"
#include "soc/interrupts.h"

/* Das FIFO vor dem Sender: bei voller Geschwindigkeit einige Millisekunden. */
#define LINE_BYTES 512U

/* Hoechstens so viel geht je Paket an den Sender: kuerzer als sein FIFO von
 * 64 Byte, sonst haelt der Host ein volles Paket fuer unvollstaendig
 * (FB-264, FB-267). */
#define LINE_PACKET 63U

/* Das Flag von `esp_intr_alloc` fuer die Stufe der Leitung. */
#define LINE_LEVEL_FLAG ESP_INTR_FLAG_LEVEL3
_Static_assert(LINE_PRIORITY == 3U, "LINE_LEVEL_FLAG nennt die Stufe LINE_PRIORITY");

/* Das FIFO: `tail` schreibt, wer sendet (Schritt oder Wirt, nie beide
 * zugleich), `head` nur die ISR; beide zaehlen frei und atomar. */
static uint8_t line_fifo[LINE_BYTES];
static uint32_t line_head;
static uint32_t line_tail;

/* Atomar: Der Host hat `TAKT` gesandt. */
static int line_wish;

/* Liest die Gegenrichtung und sucht das Wort, mit dem der Host das Board
 * zurueckverlangt. */
static IRAM_ATTR void line_receive(void) {
    static const char word[] = "TAKT";
    /* Wie viele Bytes des Worts zuletzt am Stueck kamen. */
    static uint32_t matched;
    uint8_t bytes[LINE_PACKET];
    const int n = usb_serial_jtag_ll_read_rxfifo(bytes, sizeof bytes);
    for (int i = 0; i < n; i++) {
        if (bytes[i] == (uint8_t)word[matched]) {
            matched++;
        } else {
            matched = (bytes[i] == (uint8_t)word[0]) ? 1U : 0U;
        }
        if (matched == ((sizeof word) - 1U)) {
            matched = 0U;
            __atomic_store_n(&line_wish, 1, __ATOMIC_SEQ_CST);
        }
    }
}

/* Gibt dem Sender ein Paket; ist nichts da, schweigt der Interrupt, bis
 * `try_write` ihn wieder einschaltet. */
static IRAM_ATTR void line_send(void) {
    const uint32_t head = __atomic_load_n(&line_head, __ATOMIC_SEQ_CST);
    const uint32_t tail = __atomic_load_n(&line_tail, __ATOMIC_SEQ_CST);
    uint32_t sent = 0U;
    while (((head + sent) != tail) && (sent < LINE_PACKET) && (usb_serial_jtag_ll_txfifo_writable() != 0)) {
        const uint8_t byte = line_fifo[(head + sent) % LINE_BYTES];
        if (usb_serial_jtag_ll_write_txfifo(&byte, 1U) != 1) {
            break;
        }
        sent++;
    }
    if (sent != 0U) {
        usb_serial_jtag_ll_txfifo_flush();
        __atomic_store_n(&line_head, head + sent, __ATOMIC_SEQ_CST);
    } else {
        usb_serial_jtag_ll_disable_intr_mask(USB_SERIAL_JTAG_INTR_SERIAL_IN_EMPTY);
    }
}

static IRAM_ATTR void line_isr(void *arg) {
    (void)arg;
    const uint32_t pending = usb_serial_jtag_ll_get_intsts_mask();
    usb_serial_jtag_ll_clr_intsts_mask(pending);
    if ((pending & (uint32_t)USB_SERIAL_JTAG_INTR_SERIAL_OUT_RECV_PKT) != 0U) {
        line_receive();
    }
    if ((pending & (uint32_t)USB_SERIAL_JTAG_INTR_SERIAL_IN_EMPTY) != 0U) {
        line_send();
    }
}

/* Ein Byte, wenn im FIFO Platz ist, ohne zu warten. */
static IRAM_ATTR uint8_t line_try_write(void *ctx, uint8_t byte) {
    (void)ctx;
    const uint32_t tail = __atomic_load_n(&line_tail, __ATOMIC_SEQ_CST);
    const uint32_t head = __atomic_load_n(&line_head, __ATOMIC_SEQ_CST);
    uint8_t taken = 0U;
    if ((tail - head) < LINE_BYTES) {
        line_fifo[tail % LINE_BYTES] = byte;
        __atomic_store_n(&line_tail, tail + 1U, __ATOMIC_SEQ_CST);
        taken = 1U;
    }
    return taken;
}

/* Weckt die ISR, die sendet, was im FIFO liegt. */
static IRAM_ATTR void line_flush(void *ctx) {
    (void)ctx;
    usb_serial_jtag_ll_ena_intr_mask(USB_SERIAL_JTAG_INTR_SERIAL_IN_EMPTY);
}

int line_open(struct takt_line *line) {
    int rc = -1;
    usb_serial_jtag_ll_disable_intr_mask(USB_SERIAL_JTAG_LL_INTR_MASK);
    usb_serial_jtag_ll_clr_intsts_mask(USB_SERIAL_JTAG_LL_INTR_MASK);
    if (esp_intr_alloc(ETS_USB_SERIAL_JTAG_INTR_SOURCE, ESP_INTR_FLAG_IRAM | LINE_LEVEL_FLAG, line_isr, NULL, NULL) ==
        ESP_OK) {
        usb_serial_jtag_ll_ena_intr_mask(USB_SERIAL_JTAG_INTR_SERIAL_OUT_RECV_PKT);
        line->ctx = NULL;
        line->try_write = line_try_write;
        line->flush = line_flush;
        rc = 0;
    }
    return rc;
}

int line_handback(void) { return __atomic_load_n(&line_wish, __ATOMIC_SEQ_CST); }
