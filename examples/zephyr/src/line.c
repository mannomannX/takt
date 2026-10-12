/* line.c - die Leitung auf dem UART-Geraet `takt,line` (line.h).
 *
 * **Ohne zu warten** (12.2): `try_write` legt ein Byte in einen Ring, wenn
 * Platz ist, und der Interrupt des Geraets gibt ihn an dessen Sender weiter.
 * Den Ring davor, das Zaehlen und das Verwerfen macht das SDK.
 *
 * **Die Gegenrichtung liest derselbe Interrupt.** `XOFF` haelt das Senden
 * an, bis `XON` kommt: Der Adapter des F401 meldet so, dass der Host nicht
 * abholt (FB-306); der Trace ist Text, beide Zeichen kommen darin nicht vor.
 * Steht `TAKT` darin, verlangt der Host das Board zurueck (handback.h). */
#include "line.h"

#include <errno.h>
#include <stdint.h>

#include <zephyr/device.h>
#include <zephyr/drivers/uart.h>
#include <zephyr/kernel.h>
#include <zephyr/sys/ring_buffer.h>

#include "handback.h"

/* Was die Leitung ohne Warten annimmt: bei 921600 Baud gut fuenf
 * Millisekunden, mehr als ein Tick des Pruefprogramms schreibt. */
#define LINE_BYTES 512U

/* Hoechstens so viel geht je Interrupt an den Sender: Der USB-Serial-JTAG
 * des C6 nimmt ein Paket nach dem anderen, kuerzer als sein FIFO (FB-264,
 * FB-267); ein USART ohne FIFO nimmt ohnehin ein Byte. */
#define LINE_PACKET 63U

#define XON 0x11U
#define XOFF 0x13U

/* Das Wort, mit dem der Host das Board zurueckverlangt. */
static const uint8_t handback_word[] = {(uint8_t)'T', (uint8_t)'A', (uint8_t)'K', (uint8_t)'T'};

static const struct device *const line_device = DEVICE_DT_GET(DT_CHOSEN(takt_line));

RING_BUF_DECLARE(line_ring, LINE_BYTES);

/* Der Host hat `XOFF` gesandt und noch kein `XON`. */
static atomic_t line_paused;

/* Wie viele Bytes des Worts zuletzt am Stueck kamen; nur der Interrupt. */
static size_t line_matched;

/* Ein Byte der Gegenrichtung. */
static void line_received(const struct device *dev, uint8_t byte) {
    if (byte == XOFF) {
        (void)atomic_set(&line_paused, 1);
    } else if (byte == XON) {
        (void)atomic_set(&line_paused, 0);
        uart_irq_tx_enable(dev);
    } else {
        if (byte == handback_word[line_matched]) {
            line_matched++;
        } else {
            line_matched = (byte == handback_word[0]) ? 1U : 0U;
        }
        if (line_matched == sizeof handback_word) {
            line_matched = 0U;
            handback_wish();
        }
    }
}

/* Gibt dem Sender ein Paket; das naechste, wenn er wieder bereit ist. Ist
 * nichts da oder haelt der Host an, schweigt der Interrupt, bis `flush`
 * oder `XON` ihn wieder einschaltet. */
static void line_send(const struct device *dev) {
    uint8_t *bytes = NULL;
    const uint32_t claimed = ring_buf_get_claim(&line_ring, &bytes, LINE_PACKET);
    uint32_t sent = 0U;
    if ((claimed == 0U) || (atomic_get(&line_paused) != 0)) {
        uart_irq_tx_disable(dev);
    } else {
        const int taken = uart_fifo_fill(dev, bytes, (int)claimed);
        sent = (taken > 0) ? (uint32_t)taken : 0U;
    }
    (void)ring_buf_get_finish(&line_ring, sent);
}

/* Je Aufruf: alles Empfangene, und ein Paket an den Sender. */
static void line_isr(const struct device *dev, void *user_data) {
    ARG_UNUSED(user_data);
    if (uart_irq_update(dev) > 0) {
        if (uart_irq_rx_ready(dev) > 0) {
            uint8_t byte = 0U;
            while (uart_fifo_read(dev, &byte, 1) == 1) {
                line_received(dev, byte);
            }
        }
        if (uart_irq_tx_ready(dev) > 0) {
            line_send(dev);
        }
    }
}

static uint8_t line_try_write(void *ctx, uint8_t byte) {
    ARG_UNUSED(ctx);
    return (ring_buf_put(&line_ring, &byte, 1U) == 1U) ? 1U : 0U;
}

static void line_flush(void *ctx) {
    ARG_UNUSED(ctx);
    if ((atomic_get(&line_paused) == 0) && (ring_buf_is_empty(&line_ring) == false)) {
        uart_irq_tx_enable(line_device);
    }
}

int line_open(struct takt_line *line) {
    int rc = -ENODEV;
    if (device_is_ready(line_device)) {
        rc = uart_irq_callback_user_data_set(line_device, line_isr, NULL);
        if (rc == 0) {
            uart_irq_rx_enable(line_device);
            line->ctx = NULL;
            line->try_write = line_try_write;
            line->flush = line_flush;
        }
    }
    return rc;
}
