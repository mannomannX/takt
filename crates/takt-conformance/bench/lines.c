/* takt bench (13.8): DFA-Zeilenparsing bei 2 000 Zeilen je Sekunde — die
 * C-Referenz zu `lines.takt`. `gen` schreibt je Tick zwei Zeilen in eine
 * Warteschlange, `parse` liest sie im naechsten Tick: `T<Kanal>=<Wert>` mit
 * zwei Ganzzahlen, sonst der Auffang. Geprueft wird, was Takt prueft: Platz
 * in der Warteschlange, die Laenge der Zeile, Stellen und Ueberlauf der
 * Platzhalter (8.7), die Arithmetik der Summe und ihr Range. `faults`
 * zaehlt, was Takt gemeldet haette; im Lauf bleibt es null. */
#include <stdint.h>

#define CAP 8u
#define LINE 32u

struct line {
    uint32_t len;
    char text[LINE];
};

static struct line queue[CAP];
static uint32_t head, visible, pending;
static int32_t n;
static int64_t sum, other, digest;
static volatile uint32_t faults;

/* `{x}` (3.9): auf dem negativen Wert gerechnet, damit auch der kleinste
 * darstellbar bleibt, dann umgedreht. */
static uint32_t put_int(char *out, uint32_t at, int64_t v) {
    char digits[20];
    uint32_t k = 0;
    int64_t rest = v < 0 ? v : -v;
    do {
        digits[k++] = (char)('0' - rest % 10);
        rest /= 10;
    } while (rest != 0);
    if (v < 0) {
        out[at++] = '-';
    }
    while (k > 0) {
        out[at++] = digits[--k];
    }
    return at;
}

/* `send` (8.6): Ein voller Strom ist ein Ueberlauf, eine lange Zeile wird
 * gekuerzt. */
static void send(const char *text, uint32_t len) {
    if (visible + pending == CAP) {
        faults++;
        return;
    }
    struct line *l = &queue[(head + visible + pending) % CAP];
    l->len = len > LINE ? LINE : len;
    for (uint32_t i = 0; i < l->len; i++) {
        l->text[i] = text[i];
    }
    pending++;
}

/* `{x:int}` (8.7): `[+-]?[0-9]{1,19}` als laengstes Praefix; ohne Ziffer
 * oder bei Ueberlauf passt das Muster nicht. */
static int take_int(const struct line *l, uint32_t *at, int64_t *out) {
    uint32_t i = *at;
    int negative = 0;
    if (i < l->len && (l->text[i] == '+' || l->text[i] == '-')) {
        negative = l->text[i] == '-';
        i++;
    }
    uint32_t start = i;
    int64_t v = 0;
    while (i < l->len && i - start < 19u && l->text[i] >= '0' && l->text[i] <= '9') {
        int64_t d = l->text[i] - '0';
        if (v < (INT64_MIN + d) / 10) {
            return 0;
        }
        v = v * 10 - d;
        i++;
    }
    if (i == start || (!negative && v == INT64_MIN)) {
        return 0;
    }
    *out = negative ? v : -v;
    *at = i;
    return 1;
}

/* `matches "T{ch:int}={v:int}"`: der ganze Text. */
static int measurement(const struct line *l, int64_t *ch, int64_t *v) {
    uint32_t at = 1;
    if (l->len < 1 || l->text[0] != 'T' || !take_int(l, &at, ch)) {
        return 0;
    }
    if (at >= l->len || l->text[at] != '=') {
        return 0;
    }
    at++;
    return take_int(l, &at, v) && at == l->len;
}

static void handle(const struct line *l) {
    int64_t ch, v, scaled, total;
    if (!measurement(l, &ch, &v)) {
        other = (other + 1) % 65536;
        return;
    }
    if (__builtin_mul_overflow(ch, (int64_t)1000, &scaled) || __builtin_add_overflow(sum, scaled, &total) ||
        __builtin_add_overflow(total, v, &total)) {
        faults++;
        return;
    }
    total %= 65536;
    if (total < 0) {
        faults++;
        return;
    }
    sum = total;
}

void takt_bench_reference(void) {
    char text[48];
    uint32_t len;

    n = (n + 7) % 1000;
    len = 0;
    text[len++] = 'T';
    len = put_int(text, len, n % 16);
    text[len++] = '=';
    len = put_int(text, len, n);
    send(text, len);
    len = 0;
    text[len++] = 'O';
    text[len++] = 'K';
    text[len++] = ' ';
    len = put_int(text, len, n);
    send(text, len);

    digest = (sum + other) % 65536;
    for (uint32_t i = 0; i < visible; i++) {
        handle(&queue[(head + i) % CAP]);
    }
    /* Tickende (9.4 `advance_cursors`): Alles Sichtbare ist untersucht, was
     * dieser Tick schrieb, wird sichtbar. */
    head = (head + visible) % CAP;
    visible = pending;
    pending = 0;
}

unsigned long long takt_bench_reference_digest(void) {
    return (unsigned long long)digest;
}
