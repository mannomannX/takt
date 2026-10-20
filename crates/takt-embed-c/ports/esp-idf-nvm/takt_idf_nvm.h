/* takt_idf_nvm.h - das Geraet des Journals (5.9, 12.3; `struct takt_nvm`)
 * in einer Partition unter ESP-IDF (M11 Schritt 21a).
 *
 * Die Partition, die der Wirt nennt, haelt die zwei Slots des Journals,
 * jeder die Haelfte und jeder auf einer Grenze des Loeschblocks (C6: zwei
 * Sektoren zu 4 KiB). Das Geraet geht ueber `esp_partition`, die rohe
 * Schnittstelle: Loeschen, Schreiben und Lesen tun genau das, was das
 * Journal verlangt. NVS fuehrte selbst ein Journal mit eigener
 * Speicherbereinigung, und ein Schreibvorgang des Programms loeste zu einer
 * Zeit, die Pruefung 32 nicht kennt, ein Loeschen aus.
 *
 * **Asynchron, ein Treiber, der den Vorgang anstoesst und abfragt** (12.3
 * (2), `nvm_blocking = false`): Unter ESP-IDF rechnet der Schritt in einer
 * IRAM-ISR, und dort darf kein Flash-Zugriff laufen. `begin_erase` und
 * `begin_write` nehmen den Auftrag nur an — `begin_write` kopiert die Bytes
 * in den Puffer des Wirts — und wecken die Aufgabe des Geraets; die
 * loescht oder schreibt, waehrend ESP-IDF den Planer anhaelt, und der
 * Schritt laeuft im RAM weiter. `poll` meldet das Ergebnis, einmal; danach
 * ruht das Geraet. `read` laeuft nur vor dem ersten Tick
 * (`takt_core_init_journaled`) und liest selbst.
 *
 * Der Schritt ruft `begin_*` und `poll` im IRAM; die Komponente legt das
 * Geraet dorthin (`esp-idf/takt/takt.lf`).
 */
#ifndef TAKT_IDF_NVM_H
#define TAKT_IDF_NVM_H

#include <stddef.h>
#include <stdint.h>

#include "esp_partition.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "takt_sdk.h"

#ifdef __cplusplus
extern "C" {
#endif

/* Das Geraet; der Wirt stellt den Speicher, die Felder gehoeren ihm. */
struct takt_idf_nvm {
    const esp_partition_t *partition;
    uint32_t slot_size;
    uint8_t *buffer;       /* die Bytes des laufenden Schreibvorgangs */
    uint32_t buffer_bytes; /* so viele hoechstens */
    TaskHandle_t task;     /* die Aufgabe, die loescht und schreibt */
    StaticTask_t tcb;
    uint8_t erase;   /* der Auftrag: 1 loeschen, 0 schreiben */
    uint8_t slot;    /* sein Slot */
    uint32_t offset; /* beim Schreiben der Beginn im Slot */
    uint32_t len;    /* und die Zahl der Bytes im Puffer */
    int32_t state;   /* atomar: TAKT_NVM_* */
};

/* Oeffnet die Partition mit der Bezeichnung `label` und fuellt `nvm` mit den
 * Rueckrufen des Geraets; `blocking_ns` ist `P_NVM_BLOCKING_NS` des
 * Programms, unter `nvm_blocking = false` also 0. `buffer` mit
 * `buffer_bytes` Byte nimmt einen
 * Schreibvorgang auf (`PERSIST_BOUND` und der Kopf eines Eintrags des
 * Journals genuegen); `stack`, `stack_bytes` und `priority` sind die der
 * Aufgabe des Geraets, ueber der fremden Last des Wirts. 0, oder -1, wenn
 * die Partition fehlt, nicht in zwei Slots auf Grenzen des Loeschblocks
 * zerfaellt oder die Aufgabe nicht entsteht. */
int takt_idf_nvm_open(struct takt_idf_nvm *dev, const char *label, int64_t blocking_ns, uint8_t *buffer,
                      uint32_t buffer_bytes, StackType_t *stack, size_t stack_bytes, UBaseType_t priority,
                      struct takt_nvm *nvm);

/* Loescht beide Slots: ein frisches Journal, wie es ein Lauf ohne
 * gespeicherte Werte verlangt (Portpruefung). Vor dem ersten Tick. 0 oder
 * -1. */
int takt_idf_nvm_clear(const struct takt_idf_nvm *dev);

#ifdef __cplusplus
}
#endif

#endif /* TAKT_IDF_NVM_H */
