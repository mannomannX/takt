/* takt_idf_nvm.c - das Geraet des Journals in einer Partition unter ESP-IDF
 * (takt_idf_nvm.h). */
#include "takt_idf_nvm.h"

#include <string.h>

#include "esp_attr.h"

/* Ob `len` Byte ab `offset` im Slot `slot` liegen. */
static IRAM_ATTR int takt_idf_nvm_inside(const struct takt_idf_nvm *dev, uint8_t slot, uint32_t offset,
                                         uint32_t len) {
    return ((slot <= 1U) && (offset <= dev->slot_size) && (len <= (dev->slot_size - offset))) ? 1 : 0;
}

/* Weckt die Aufgabe des Geraets, aus dem Schritt in der ISR oder vor dem
 * ersten Tick aus einer Aufgabe. Aus der ISR laeuft sie, wenn der Planer
 * das naechste Mal waehlt. */
static IRAM_ATTR void takt_idf_nvm_wake(const struct takt_idf_nvm *dev) {
    if (xPortInIsrContext() != pdFALSE) {
        BaseType_t woken = pdFALSE;
        vTaskNotifyGiveFromISR(dev->task, &woken);
    } else {
        (void)xTaskNotifyGive(dev->task);
    }
}

/* Nimmt einen Auftrag an, wenn das Geraet ruht. Auftraege und `poll` gibt
 * nur der Kern, nacheinander; die Aufgabe des Geraets schreibt den Zustand
 * nur aus `TAKT_NVM_BUSY`. Zwischen Lesen und Schreiben aendert ihn darum
 * niemand. */
static IRAM_ATTR uint8_t takt_idf_nvm_take(struct takt_idf_nvm *dev) {
    uint8_t taken = 0U;
    if (__atomic_load_n(&dev->state, __ATOMIC_SEQ_CST) == TAKT_NVM_IDLE) {
        __atomic_store_n(&dev->state, TAKT_NVM_BUSY, __ATOMIC_SEQ_CST);
        taken = 1U;
    }
    return taken;
}

static IRAM_ATTR uint8_t takt_idf_nvm_erase(void *ctx, uint8_t slot) {
    struct takt_idf_nvm *dev = ctx;
    uint8_t taken = 0U;
    if ((takt_idf_nvm_inside(dev, slot, 0U, dev->slot_size) != 0) && (takt_idf_nvm_take(dev) != 0U)) {
        dev->erase = 1U;
        dev->slot = slot;
        takt_idf_nvm_wake(dev);
        taken = 1U;
    }
    return taken;
}

static IRAM_ATTR uint8_t takt_idf_nvm_write(void *ctx, uint8_t slot, uint32_t offset, const uint8_t *bytes,
                                            uint32_t len) {
    struct takt_idf_nvm *dev = ctx;
    uint8_t taken = 0U;
    if ((takt_idf_nvm_inside(dev, slot, offset, len) != 0) && (len <= dev->buffer_bytes) &&
        (takt_idf_nvm_take(dev) != 0U)) {
        (void)memcpy(dev->buffer, bytes, len);
        dev->erase = 0U;
        dev->slot = slot;
        dev->offset = offset;
        dev->len = len;
        takt_idf_nvm_wake(dev);
        taken = 1U;
    }
    return taken;
}

/* Das Ergebnis des letzten Vorgangs, einmal; danach ruht das Geraet. Solange
 * die Aufgabe noch schreibt, `TAKT_NVM_BUSY`. */
static IRAM_ATTR int32_t takt_idf_nvm_poll(void *ctx) {
    struct takt_idf_nvm *dev = ctx;
    int32_t result = __atomic_load_n(&dev->state, __ATOMIC_SEQ_CST);
    if ((result == TAKT_NVM_DONE) || (result == TAKT_NVM_FAILED)) {
        __atomic_store_n(&dev->state, TAKT_NVM_IDLE, __ATOMIC_SEQ_CST);
    }
    return result;
}

static uint8_t takt_idf_nvm_read(void *ctx, uint8_t slot, uint32_t offset, uint8_t *into, uint32_t len) {
    const struct takt_idf_nvm *dev = ctx;
    uint8_t ok = 0U;
    if ((takt_idf_nvm_inside(dev, slot, offset, len) != 0) &&
        (esp_partition_read(dev->partition, ((size_t)slot * dev->slot_size) + offset, into, len) == ESP_OK)) {
        ok = 1U;
    }
    return ok;
}

/* Die Aufgabe des Geraets: fuehrt jeden Auftrag aus und meldet sein
 * Ergebnis. */
static void takt_idf_nvm_run(void *arg) {
    struct takt_idf_nvm *dev = arg;
    for (;;) {
        (void)ulTaskNotifyTake(pdTRUE, portMAX_DELAY);
        const size_t at = ((size_t)dev->slot * dev->slot_size) + ((dev->erase != 0U) ? 0U : dev->offset);
        const esp_err_t rc = (dev->erase != 0U) ? esp_partition_erase_range(dev->partition, at, dev->slot_size)
                                                : esp_partition_write(dev->partition, at, dev->buffer, dev->len);
        __atomic_store_n(&dev->state, (rc == ESP_OK) ? TAKT_NVM_DONE : TAKT_NVM_FAILED, __ATOMIC_SEQ_CST);
    }
}

int takt_idf_nvm_open(struct takt_idf_nvm *dev, const char *label, int64_t blocking_ns, uint8_t *buffer,
                      uint32_t buffer_bytes, StackType_t *stack, size_t stack_bytes, UBaseType_t priority,
                      struct takt_nvm *nvm) {
    int rc = -1;
    dev->task = NULL;
    dev->partition = esp_partition_find_first(ESP_PARTITION_TYPE_DATA, ESP_PARTITION_SUBTYPE_ANY, label);
    if (dev->partition != NULL) {
        const uint32_t erase = dev->partition->erase_size;
        dev->slot_size = (uint32_t)(dev->partition->size / 2U);
        dev->buffer = buffer;
        dev->buffer_bytes = buffer_bytes;
        __atomic_store_n(&dev->state, TAKT_NVM_IDLE, __ATOMIC_SEQ_CST);
        if ((dev->slot_size != 0U) && (erase != 0U) && ((dev->slot_size % erase) == 0U)) {
            dev->task = xTaskCreateStatic(takt_idf_nvm_run, "takt-nvm", (uint32_t)(stack_bytes / sizeof(StackType_t)),
                                          dev, priority, stack, &dev->tcb);
        }
        if (dev->task != NULL) {
            nvm->ctx = dev;
            nvm->slot_size = dev->slot_size;
            nvm->blocking_ns = blocking_ns;
            nvm->begin_erase = takt_idf_nvm_erase;
            nvm->begin_write = takt_idf_nvm_write;
            nvm->poll = takt_idf_nvm_poll;
            nvm->read = takt_idf_nvm_read;
            rc = 0;
        }
    }
    return rc;
}

int takt_idf_nvm_clear(const struct takt_idf_nvm *dev) {
    return (esp_partition_erase_range(dev->partition, 0U, (size_t)dev->slot_size * 2U) == ESP_OK) ? 0 : -1;
}
