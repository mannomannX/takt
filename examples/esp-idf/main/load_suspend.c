/* load_suspend.c - das automatische Aussetzen des Flash gemessen (load.h;
 * `TAKT_LOAD=suspend`, M11 Schritt 21b; plan/m11.md 1.7 und 2.5).
 *
 * Kann der Flash einen Loesch- oder Schreibvorgang aussetzen, sobald der
 * Kern Code aus ihm liest (`CONFIG_SPI_FLASH_AUTO_SUSPEND`), liefe Takt auch
 * in einer Aufgabe mit Code im Flash weiter, die zweite Form aus 2.5. Ob das
 * auf Board 2 traegt, sagen drei Zahlen, gemessen vor dem Lauf:
 *
 * - ob der Baustein es kann (`SPI_FLASH_CHIP_CAP_SUSPEND` seines Treibers in
 *   ESP-IDF) und seine JEDEC-Kennung;
 * - die Revision des C6: ESP-IDF schaltet das Aussetzen auf dem C6 erst ab
 *   v0.2 ein, darunter hat der Chip einen Fehler dabei
 *   (`esp_flash_suspend_cmd_init`, `CONFIG_SPI_FLASH_FORCE_ENABLE_C6_H2_SUSPEND`);
 * - wie lange eine Aufgabe mit Code im Flash ohne Aussetzen steht, waehrend
 *   ein Sektor geloescht und beschrieben wird: Sie wacht jede Millisekunde
 *   auf, und die laengste Pause zwischen zwei Wecken ist die Zahl.
 *
 * Das Aussetzen selbst schaltet dieser Bau nicht ein: Unter v0.2 startete
 * ESP-IDF dann nicht.
 */
#include <stdint.h>
#include <string.h>

#include "esp_chip_info.h"
#include "esp_flash.h"
#include "esp_flash_chips/spi_flash_chip_driver.h"
#include "esp_partition.h"
#include "esp_timer.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "load.h"
#include "program.h"

/* Die Probe ueber allem, was der Wirt rechnet. */
#define PROBE_PRIORITY 22U
#define PROBE_STACK_BYTES 3072U

/* So viele Sektoren loescht und beschreibt die Messung. */
#define PROBE_ROUNDS 8U

/* Ein Sektor des Flash in Byte. */
#define PROBE_SECTOR_BYTES 4096U

/* So viele Sektoren hat die Partition `takt_probe`. */
#define PROBE_SECTORS 4U

/* Atomar: Die Probe misst, solange es 1 ist; danach meldet sie sich mit
 * `probe_done` ab. */
static int probe_on;
static int probe_done;

/* Die laengste Pause zwischen zwei Wecken der Probe in Mikrosekunden; nur
 * die Probe schreibt sie, gelesen wird sie nach `probe_done`. */
static int64_t probe_gap_us;

/* Das laengste Loeschen eines Sektors in Mikrosekunden. */
static int64_t probe_erase_us;

/* Die JEDEC-Kennung des Bausteins, ob sein Treiber das Aussetzen kann, und
 * die Revision des C6 (`esp_chip_info`: Major mal 100 plus Minor). */
static uint32_t probe_jedec;
static uint8_t probe_can_suspend;
static uint16_t probe_revision;

/* Die Probe: Code im Flash, jede Millisekunde geweckt. */
static void probe(void *arg) {
    (void)arg;
    TickType_t wake = xTaskGetTickCount();
    int64_t last = esp_timer_get_time();
    while (__atomic_load_n(&probe_on, __ATOMIC_SEQ_CST) != 0) {
        vTaskDelayUntil(&wake, 1U);
        const int64_t now = esp_timer_get_time();
        if ((now - last) > probe_gap_us) {
            probe_gap_us = now - last;
        }
        last = now;
    }
    __atomic_store_n(&probe_done, 1, __ATOMIC_SEQ_CST);
    vTaskDelete(NULL);
}

int load_start(void) {
    static uint8_t block[PROBE_SECTOR_BYTES];
    int rc = -1;
    esp_chip_info_t info;
    esp_chip_info(&info);
    probe_revision = info.revision;
    const esp_flash_t *chip = esp_flash_default_chip;
    if ((chip != NULL) && (chip->chip_drv != NULL) && (chip->chip_drv->get_chip_caps != NULL)) {
        const uint32_t caps = (uint32_t)chip->chip_drv->get_chip_caps(esp_flash_default_chip);
        probe_can_suspend = ((caps & (uint32_t)SPI_FLASH_CHIP_CAP_SUSPEND) != 0U) ? 1U : 0U;
    }
    const esp_partition_t *part = esp_partition_find_first(ESP_PARTITION_TYPE_DATA, ESP_PARTITION_SUBTYPE_ANY, "takt_probe");
    (void)memset(block, 0xA5, sizeof block);
    __atomic_store_n(&probe_on, 1, __ATOMIC_SEQ_CST);
    if ((part != NULL) && (esp_flash_read_id(NULL, &probe_jedec) == ESP_OK) &&
        (xTaskCreate(probe, "takt-probe", PROBE_STACK_BYTES, NULL, PROBE_PRIORITY, NULL) == pdPASS)) {
        rc = 0;
        vTaskDelay(pdMS_TO_TICKS(10U));
        for (uint32_t round = 0U; (round < PROBE_ROUNDS) && (rc == 0); round++) {
            const uint32_t at = (round % PROBE_SECTORS) * PROBE_SECTOR_BYTES;
            const int64_t start = esp_timer_get_time();
            if (esp_partition_erase_range(part, at, PROBE_SECTOR_BYTES) != ESP_OK) {
                rc = -1;
            }
            const int64_t erased = esp_timer_get_time() - start;
            if (erased > probe_erase_us) {
                probe_erase_us = erased;
            }
            if ((rc == 0) && (esp_partition_write(part, at, block, sizeof block) != ESP_OK)) {
                rc = -1;
            }
            vTaskDelay(pdMS_TO_TICKS(5U));
        }
        __atomic_store_n(&probe_on, 0, __ATOMIC_SEQ_CST);
        while (__atomic_load_n(&probe_done, __ATOMIC_SEQ_CST) == 0) {
            vTaskDelay(1U);
        }
    }
    return rc;
}

void load_banner(void) {
    takt_board_trace("takt aussetzen baustein ");
    takt_board_trace((probe_can_suspend != 0U) ? "ja" : "nein");
    takt_board_trace(" chip ");
    takt_board_trace_u64(probe_revision);
    takt_board_trace("\r\ntakt aussetzen pause ");
    takt_board_trace_u64((uint64_t)probe_gap_us);
    takt_board_trace(" loeschen ");
    takt_board_trace_u64((uint64_t)probe_erase_us);
    takt_board_trace("\r\n  Flash: JEDEC 0x");
    takt_board_trace_hex8((uint8_t)(probe_jedec >> 16U));
    takt_board_trace_hex8((uint8_t)(probe_jedec >> 8U));
    takt_board_trace_hex8((uint8_t)probe_jedec);
    takt_board_trace("; ESP-IDF setzt Flash-Vorgaenge auf dem C6 erst ab Chip v0.2 aus (Revision hier: Major mal 100 "
                     "plus Minor); eine Aufgabe mit Code im Flash, jede ms geweckt, stand ohne Aussetzen bis zur "
                     "Pause oben (us), ein Sektor loeschen dauerte bis zur Zahl danach\r\n");
}
