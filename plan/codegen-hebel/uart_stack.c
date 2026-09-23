/* Derselbe UART-Stapel wie test_uart_c6_hw.takt, idiomatisch in C:
 * L0 Port (Register, FIFO-Pumpe), L1 COBS-Framer, L2 Link (Header, CRC,
 * ACK/NAK, Retransmit), Quelle und Senke. Ein Tick sind 500 us.
 * Keine Bereichs- und Indexpruefungen, keine Meldungstexte: so schriebe
 * man es von Hand. */
#include <stdbool.h>
#include <stdint.h>
#include <stddef.h>
void *memcpy(void *, const void *, size_t);

#define TICK_US 500u
#define MS(x) ((x) * 1000u / TICK_US)

#define REG(a) (*(volatile uint32_t *)(a))
#define UART_FIFO REG(0x60000000u)
#define UART_INT_RAW REG(0x60000004u)
#define UART_INT_CLR REG(0x60000010u)
#define UART_STATUS REG(0x6000001Cu)

#define ACK 0x06
#define NAK 0x15
#define DATA 0x02
#define MAGIC 0xA5
#define HDR_LEN 5
#define MAX_PAYLOAD 1024
#define MAX_ENCODED 1030
#define RETRIES 3

enum { ERR_TIMEOUT, ERR_CRC, ERR_LENGTH, ERR_SEQ, ERR_PEER_NAK, ERR_OVERRUN, ERR_LINE };

typedef struct {
    uint16_t len;
    uint8_t d[MAX_PAYLOAD];
} bytes1024_t;

typedef struct {
    uint16_t len;
    uint8_t d[MAX_ENCODED];
} bytes1030_t;

typedef struct {
    uint8_t b, frame, parity, brk;
} rxbyte_t;

/* ---- Ringe ------------------------------------------------------------- */

typedef struct {
    uint8_t *pool;
    uint16_t capb, head, used;
} fq_t; /* Frames als u16-Laenge plus Bytes, umlaufend */

static void fq_put(fq_t *q, uint16_t at, uint8_t b) {
    if (at >= q->capb) at -= q->capb;
    q->pool[at] = b;
}

static uint8_t fq_get(const fq_t *q, uint16_t at) {
    if (at >= q->capb) at -= q->capb;
    return q->pool[at];
}

static bool fq_push(fq_t *q, const uint8_t *d, uint16_t n) {
    if (q->used + 2u + n > q->capb) return false;
    uint16_t at = q->head + q->used;
    fq_put(q, at, (uint8_t)n);
    fq_put(q, at + 1, (uint8_t)(n >> 8));
    for (uint16_t i = 0; i < n; i++) fq_put(q, at + 2 + i, d[i]);
    q->used += 2u + n;
    return true;
}

static uint16_t fq_pop(fq_t *q, uint8_t *d) {
    if (!q->used) return 0;
    uint16_t n = fq_get(q, q->head) | (uint16_t)(fq_get(q, q->head + 1) << 8);
    for (uint16_t i = 0; i < n; i++) d[i] = fq_get(q, q->head + 2 + i);
    q->head += 2u + n;
    if (q->head >= q->capb) q->head -= q->capb;
    q->used -= 2u + n;
    return n;
}

static uint8_t g_rx_pool[5120], g_tx_pool[3072], g_app_rx_pool[1024], g_app_tx_pool[5120];
static fq_t rx_frames = { g_rx_pool, sizeof g_rx_pool }, tx_frames = { g_tx_pool, sizeof g_tx_pool };
static fq_t app_rx = { g_app_rx_pool, sizeof g_app_rx_pool }, app_tx = { g_app_tx_pool, sizeof g_app_tx_pool };

static struct {
    rxbyte_t e[64];
    uint8_t head, len;
} rx_raw;
static struct {
    uint8_t e[256];
    uint8_t head;
    uint16_t len;
} tx_raw;

static bool rx_push(const rxbyte_t *ev) {
    if (rx_raw.len == 64) return false;
    rx_raw.e[(uint8_t)(rx_raw.head + rx_raw.len) & 63] = *ev;
    rx_raw.len++;
    return true;
}

static bool rx_pop(rxbyte_t *ev) {
    if (!rx_raw.len) return false;
    *ev = rx_raw.e[rx_raw.head & 63];
    rx_raw.head++;
    rx_raw.len--;
    return true;
}

static bool tx_push(uint8_t b) {
    if (tx_raw.len == 256) return false;
    tx_raw.e[(uint8_t)(tx_raw.head + tx_raw.len)] = b;
    tx_raw.len++;
    return true;
}

static uint8_t tx_pop(void) {
    uint8_t b = tx_raw.e[tx_raw.head++];
    tx_raw.len--;
    return b;
}

/* ---- reine Funktionen --------------------------------------------------- */

static void cobs_encode(const bytes1024_t *src, bytes1030_t *dst) {
    uint16_t code_at = 0, n = 1;
    uint8_t code = 1;
    dst->d[0] = 0;
    for (uint16_t i = 0; i < src->len; i++) {
        uint8_t b = src->d[i];
        if (b) {
            dst->d[n++] = b;
            code++;
        }
        if (!b || code == 255) {
            dst->d[code_at] = code;
            code_at = n;
            dst->d[n++] = 0;
            code = 1;
        }
    }
    dst->d[code_at] = code;
    dst->d[n++] = 0;
    dst->len = n;
}

static void cobs_decode(const bytes1030_t *src, bytes1024_t *dst) {
    uint16_t i = 0, n = 0;
    while (i < src->len) {
        uint8_t code = src->d[i++];
        if (!code) goto bad;
        for (uint8_t k = 1; k < code && i < src->len; k++) {
            if (n >= MAX_PAYLOAD) goto bad;
            dst->d[n++] = src->d[i++];
        }
        if (code < 255 && i < src->len) {
            if (n >= MAX_PAYLOAD) goto bad;
            dst->d[n++] = 0;
        }
    }
    dst->len = n;
    return;
bad:
    dst->len = 0;
}

static uint16_t crc16_ccitt(const uint8_t *b, uint16_t n, uint16_t crc) {
    for (uint16_t i = 0; i < n; i++) {
        crc ^= (uint16_t)(b[i] << 8);
        for (int bit = 0; bit < 8; bit++) crc = (crc & 0x8000) ? (uint16_t)((crc << 1) ^ 0x1021) : (uint16_t)(crc << 1);
    }
    return crc;
}

/* ---- Ausgaenge ----------------------------------------------------------- */

struct outputs {
    bool rs485_de, link_up;
    uint16_t sent_lines, got_lines;
    uint32_t d_rx, d_tx, d_ovf, d_ferr, d_fin, d_fout, d_dec, d_long, d_crc, d_retx;
} out;

/* ---- L0 Port ------------------------------------------------------------- */

enum { PORT_RUN, PORT_FAULT };

static struct {
    uint8_t state;
    uint16_t t, tx_stalled_for;
    uint8_t de_hold;
    uint32_t overruns, frame_errs, parity_errs, breaks, rx_bytes, tx_bytes;
    bool sending, cts_ok, line_ok;
} port = { .cts_ok = true };

static void port_fault(void) {
    port.state = PORT_FAULT;
    port.t = 0;
    out.rs485_de = false;
    port.line_ok = false;
}

static void port_step(void) {
    if (port.state == PORT_FAULT) {
        if (++port.t >= MS(100)) port.state = PORT_RUN;
        return;
    }
    uint32_t st = UART_STATUS, ir = UART_INT_RAW;
    port.line_ok = true;
    if (ir & (1u << 4)) {
        port.overruns++;
        UART_INT_CLR = 1u << 4;
    }
    if (ir & (1u << 3)) {
        port.frame_errs++;
        UART_INT_CLR = 1u << 3;
    }
    if (ir & (1u << 2)) {
        port.parity_errs++;
        UART_INT_CLR = 1u << 2;
    }
    if (ir & (1u << 7)) {
        port.breaks++;
        UART_INT_CLR = 1u << 7;
    }
    if (st & 0xFFu) {
        rxbyte_t ev = { (uint8_t)UART_FIFO, (ir >> 3) & 1, (ir >> 2) & 1, (ir >> 7) & 1 };
        if (!rx_push(&ev)) {
            port_fault();
            return;
        }
        port.rx_bytes++;
    }
    unsigned fifo = (st >> 16) & 0xFFu;
    int room = 120 - (int)fifo, sent = 0;
    while (sent < room && port.cts_ok && tx_raw.len) {
        UART_FIFO = tx_pop();
        port.tx_bytes++;
        sent++;
    }
    if (tx_raw.len && !sent) {
        if (++port.tx_stalled_for >= MS(50)) {
            port_fault();
            return;
        }
    } else {
        port.tx_stalled_for = 0;
    }
    port.sending = sent > 0;
    bool busy = tx_raw.len || sent || fifo;
    out.rs485_de = busy || port.de_hold;
    if (busy) port.de_hold = MS(3);
    else if (port.de_hold) port.de_hold--;
}

/* ---- L1 Framer ----------------------------------------------------------- */

enum { FR_ASSEMBLE, FR_SYNC, FR_RESYNC };

static struct {
    uint8_t state;
    uint16_t t;
    uint32_t frames_in, frames_out, decode_errors, too_long;
    bytes1030_t acc, enc;
    bytes1024_t out;
} fr;

static void framer_fault(void) {
    fr.state = FR_RESYNC;
    fr.t = 0;
    fr.acc.len = 0;
}

static void framer_step(void) {
    uint16_t n = fq_pop(&tx_frames, fr.out.d);
    if (n) {
        fr.out.len = n;
        cobs_encode(&fr.out, &fr.enc);
        for (uint16_t i = 0; i < fr.enc.len; i++) {
            if (!tx_push(fr.enc.d[i])) {
                framer_fault();
                return;
            }
        }
        fr.frames_out++;
    }
    if (fr.state == FR_RESYNC) {
        if (++fr.t >= MS(1)) fr.state = FR_SYNC;
        return;
    }
    rxbyte_t ev;
    while (rx_pop(&ev)) {
        if (fr.state == FR_SYNC) {
            fr.acc.len = 0;
            if (!ev.b) fr.state = FR_ASSEMBLE;
            continue;
        }
        if (ev.frame || ev.parity || ev.brk) {
            fr.decode_errors++;
            framer_fault();
            return;
        }
        if (!ev.b) {
            if (fr.acc.len) {
                cobs_decode(&fr.acc, &fr.out);
                if (fr.out.len) {
                    if (!fq_push(&rx_frames, fr.out.d, fr.out.len)) {
                        framer_fault();
                        return;
                    }
                    fr.frames_in++;
                } else {
                    fr.decode_errors++;
                }
            }
            fr.acc.len = 0;
        } else if (fr.acc.len < MAX_ENCODED) {
            fr.acc.d[fr.acc.len++] = ev.b;
        } else {
            fr.too_long++;
            framer_fault();
            return;
        }
    }
}

/* ---- L2 Link ------------------------------------------------------------- */

enum { LK_IDLE, LK_TRANSMIT, LK_AWAIT, LK_BACKOFF, LK_DOWN };

static struct {
    uint8_t state, tx_seq, rx_seq, last_err, attempts;
    uint16_t t;
    uint32_t retransmits, crc_errors, delivered;
    bool up;
    bytes1024_t pending, scratch;
} lk;

static void link_down(void) {
    lk.state = LK_DOWN;
    lk.up = false;
    lk.pending.len = 0;
}

static void send_frame(uint8_t kind, uint8_t seq, const uint8_t *payload, uint16_t len) {
    uint8_t b[HDR_LEN + MAX_PAYLOAD + 2];
    b[0] = MAGIC;
    b[1] = kind;
    b[2] = seq;
    b[3] = (uint8_t)len;
    b[4] = (uint8_t)(len >> 8);
    memcpy(b + HDR_LEN, payload, len);
    uint16_t c = crc16_ccitt(payload, len, 0xFFFF);
    b[HDR_LEN + len] = (uint8_t)(c >> 8);
    b[HDR_LEN + len + 1] = (uint8_t)c;
    if (!fq_push(&tx_frames, b, HDR_LEN + len + 2)) link_down();
}

static void link_rx(void) {
    uint8_t *f = lk.scratch.d;
    uint16_t n;
    while ((n = fq_pop(&rx_frames, f))) {
        uint16_t len = f[3] | (uint16_t)(f[4] << 8);
        bool valid = n >= HDR_LEN && f[0] == MAGIC && (f[1] == ACK || f[1] == NAK || f[1] == DATA) && len <= MAX_PAYLOAD;
        if (!valid) {
            lk.crc_errors++;
            lk.last_err = ERR_CRC;
            continue;
        }
        if (lk.state == LK_AWAIT) {
            if (f[1] == ACK) {
                lk.up = true;
                lk.tx_seq++;
                lk.state = LK_IDLE;
            } else if (f[1] == NAK) {
                lk.last_err = ERR_PEER_NAK;
                lk.retransmits++;
                lk.state = LK_BACKOFF;
                lk.t = 0;
            }
        }
        if (n < HDR_LEN + len + 2) {
            lk.last_err = ERR_LENGTH;
            continue;
        }
        uint16_t want = (uint16_t)(f[HDR_LEN + len] << 8) | f[HDR_LEN + len + 1];
        if (crc16_ccitt(f + HDR_LEN, len, 0xFFFF) != want) {
            lk.crc_errors++;
            lk.last_err = ERR_CRC;
            send_frame(NAK, f[2], 0, 0);
            continue;
        }
        if (f[1] == DATA) {
            if (f[2] == lk.rx_seq) {
                if (!fq_push(&app_rx, f + HDR_LEN, len)) {
                    link_down();
                    return;
                }
                lk.delivered++;
                lk.rx_seq++;
            }
            send_frame(ACK, f[2], 0, 0);
        }
    }
}

static void link_step(void) {
    if (lk.crc_errors >= 1000000u) link_down();
    if (lk.state != LK_DOWN) link_rx();
    switch (lk.state) {
    case LK_IDLE: {
        uint16_t n = fq_pop(&app_tx, lk.pending.d);
        if (!n) break;
        lk.pending.len = n;
        lk.attempts = 0;
    }
    /* fallthrough */
    case LK_TRANSMIT:
        send_frame(DATA, lk.tx_seq, lk.pending.d, lk.pending.len);
        lk.attempts++;
        lk.state = LK_AWAIT;
        lk.t = 0;
        break;
    case LK_AWAIT:
        if (++lk.t >= MS(25)) {
            lk.last_err = ERR_TIMEOUT;
            lk.retransmits++;
            lk.state = LK_BACKOFF;
            lk.t = 0;
        }
        break;
    case LK_BACKOFF:
        if (lk.attempts > RETRIES) link_down();
        else if (++lk.t >= MS(5)) lk.state = LK_TRANSMIT;
        break;
    case LK_DOWN:
        if (port.line_ok && port.cts_ok) lk.state = LK_IDLE;
        break;
    }
}

/* ---- Quelle und Senke ---------------------------------------------------- */

static struct {
    uint16_t t, n, m;
} app;

static void source_step(void) {
    if (++app.t < MS(1000)) return;
    app.t = 0;
    fq_push(&app_tx, (const uint8_t *)"PING\n", 5);
    app.n = (uint16_t)((app.n + 1) % 1000);
    out.sent_lines = app.n;
    out.link_up = lk.up;
    out.d_rx = port.rx_bytes;
    out.d_tx = port.tx_bytes;
    out.d_ovf = port.overruns;
    out.d_ferr = port.frame_errs;
    out.d_fin = fr.frames_in;
    out.d_fout = fr.frames_out;
    out.d_dec = fr.decode_errors;
    out.d_long = fr.too_long;
    out.d_crc = lk.crc_errors;
    out.d_retx = lk.retransmits;
}

static void sink_step(void) {
    while (fq_pop(&app_rx, lk.scratch.d)) {
        app.m = (uint16_t)((app.m + 1) % 1000);
        out.got_lines = app.m;
    }
}

/* ---- Tick ----------------------------------------------------------------- */

void stack_tick(void) {
    port_step();
    framer_step();
    link_step();
    source_step();
    sink_step();
}
