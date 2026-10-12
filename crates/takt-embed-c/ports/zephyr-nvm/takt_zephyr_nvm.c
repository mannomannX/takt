/* takt_zephyr_nvm.c - das Geraet des Journals im Flash unter Zephyr
 * (takt_zephyr_nvm.h). */
#include "takt_zephyr_nvm.h"

#include <errno.h>

/* Der Beginn des Slots `slot` in der Partition. */
static off_t takt_zephyr_nvm_at(const struct takt_zephyr_nvm *dev, uint8_t slot, uint32_t offset) {
    return (off_t)(((uint32_t)slot * dev->slot_size) + offset);
}

/* Ob `len` Byte ab `offset` im Slot `slot` liegen. */
static int takt_zephyr_nvm_inside(const struct takt_zephyr_nvm *dev, uint8_t slot, uint32_t offset, uint32_t len) {
    return ((slot <= 1U) && (offset <= dev->slot_size) && (len <= (dev->slot_size - offset))) ? 1 : 0;
}

static uint8_t takt_zephyr_nvm_erase(void *ctx, uint8_t slot) {
    struct takt_zephyr_nvm *dev = ctx;
    uint8_t taken = 0U;
    if (takt_zephyr_nvm_inside(dev, slot, 0U, dev->slot_size) != 0) {
        const int rc = flash_area_erase(dev->area, takt_zephyr_nvm_at(dev, slot, 0U), dev->slot_size);
        dev->result = (rc == 0) ? TAKT_NVM_DONE : TAKT_NVM_FAILED;
        taken = 1U;
    }
    return taken;
}

static uint8_t takt_zephyr_nvm_write(void *ctx, uint8_t slot, uint32_t offset, const uint8_t *bytes, uint32_t len) {
    struct takt_zephyr_nvm *dev = ctx;
    uint8_t taken = 0U;
    if (takt_zephyr_nvm_inside(dev, slot, offset, len) != 0) {
        const int rc = flash_area_write(dev->area, takt_zephyr_nvm_at(dev, slot, offset), bytes, len);
        dev->result = (rc == 0) ? TAKT_NVM_DONE : TAKT_NVM_FAILED;
        taken = 1U;
    }
    return taken;
}

/* Das Ergebnis des letzten Vorgangs, einmal; danach ruht das Geraet. */
static int32_t takt_zephyr_nvm_poll(void *ctx) {
    struct takt_zephyr_nvm *dev = ctx;
    const int32_t result = dev->result;
    dev->result = TAKT_NVM_IDLE;
    return result;
}

static uint8_t takt_zephyr_nvm_read(void *ctx, uint8_t slot, uint32_t offset, uint8_t *into, uint32_t len) {
    const struct takt_zephyr_nvm *dev = ctx;
    uint8_t ok = 0U;
    if ((takt_zephyr_nvm_inside(dev, slot, offset, len) != 0) &&
        (flash_area_read(dev->area, takt_zephyr_nvm_at(dev, slot, offset), into, len) == 0)) {
        ok = 1U;
    }
    return ok;
}

int takt_zephyr_nvm_open(struct takt_zephyr_nvm *dev, uint8_t id, int64_t blocking_ns, struct takt_nvm *nvm) {
    int rc = flash_area_open(id, &dev->area);
    if (rc == 0) {
        dev->slot_size = (uint32_t)(dev->area->fa_size / 2U);
        dev->result = TAKT_NVM_IDLE;
        if (dev->slot_size == 0U) {
            rc = -EINVAL;
        } else {
            nvm->ctx = dev;
            nvm->slot_size = dev->slot_size;
            nvm->blocking_ns = blocking_ns;
            nvm->begin_erase = takt_zephyr_nvm_erase;
            nvm->begin_write = takt_zephyr_nvm_write;
            nvm->poll = takt_zephyr_nvm_poll;
            nvm->read = takt_zephyr_nvm_read;
        }
    }
    return rc;
}

int takt_zephyr_nvm_clear(struct takt_zephyr_nvm *dev) {
    return flash_area_erase(dev->area, 0, (size_t)dev->slot_size * 2U);
}
