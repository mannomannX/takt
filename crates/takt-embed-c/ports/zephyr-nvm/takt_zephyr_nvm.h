/* takt_zephyr_nvm.h - das Geraet des Journals (5.9, 12.3; `struct takt_nvm`)
 * im Flash unter Zephyr (M11 Schritt 19, FB-732), fuer jeden Port unter
 * Zephyr. Das Modul baut es mit `CONFIG_TAKT_JOURNAL_FLASH`.
 *
 * Die Partition des Devicetree, die der Wirt nennt, haelt die zwei Slots des
 * Journals, jeder die Haelfte, und jeder beginnt auf einer Grenze des
 * Loeschblocks (F401: Sektor 2 und 3 zu 16 KiB; C6: zwei Sektoren zu 4 KiB).
 * Das Geraet geht ueber `flash_area`, die rohe Flash-Schnittstelle:
 * Loeschen, Schreiben und Lesen tun genau das, was das Journal verlangt, und
 * dauern so lange, wie die Hardware-Konfiguration misst (`nvm_erase_ns`,
 * `nvm_program_ns`). NVS und ZMS fuehren selbst ein Journal mit eigener
 * Speicherbereinigung; darunter laege ein zweites Journal, und ein
 * Schreibvorgang des Programms loeste zu einer Zeit, die Pruefung 32 nicht
 * kennt, ein Loeschen aus.
 *
 * Die Flash-Schnittstelle von Zephyr arbeitet synchron: `begin_erase` und
 * `begin_write` tun den Vorgang ganz und halten den Schrittfaden so lange
 * an (`blocking_ns`, 12.3); `poll` meldet danach sein Ergebnis. Auf beiden
 * Boards haelt das Flash ohnehin den Kern an (eine Bank, Code im Flash). */
#ifndef TAKT_ZEPHYR_NVM_H
#define TAKT_ZEPHYR_NVM_H

#include <stdint.h>

#include <zephyr/storage/flash_map.h>

#include "takt_sdk.h"

#ifdef __cplusplus
extern "C" {
#endif

/* Das Geraet; der Wirt stellt den Speicher, die Felder gehoeren ihm. */
struct takt_zephyr_nvm {
    const struct flash_area *area;
    uint32_t slot_size;
    int32_t result; /* was `poll` als Naechstes meldet: TAKT_NVM_* */
};

/* Oeffnet die Partition `id` (`PARTITION_ID(label)`) und fuellt `nvm` mit den
 * Rueckrufen des Geraets; `blocking_ns` ist `P_NVM_BLOCKING_NS` des
 * Programms. 0 oder -errno; -EINVAL, wenn die Partition nicht in zwei
 * Slots zerfaellt. */
int takt_zephyr_nvm_open(struct takt_zephyr_nvm *dev, uint8_t id, int64_t blocking_ns, struct takt_nvm *nvm);

/* Loescht beide Slots: ein frisches Journal, wie es ein Lauf ohne
 * gespeicherte Werte verlangt (Portpruefung). 0 oder -errno. */
int takt_zephyr_nvm_clear(const struct takt_zephyr_nvm *dev);

#ifdef __cplusplus
}
#endif

#endif /* TAKT_ZEPHYR_NVM_H */
