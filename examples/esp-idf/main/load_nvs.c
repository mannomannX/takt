/* load_nvs.c - NVS-Last neben Takt (load_nvs.h). */
#include "load_nvs.h"

#include <stdint.h>

#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "load.h"
#include "nvs.h"
#include "nvs_flash.h"

/* So gross ist ein Datensatz: Nach einigen Dutzend Saetzen ist eine Seite von
 * NVS voll, und NVS loescht einen Sektor. */
#define NVS_RECORD_BYTES 512U

/* Ein Datensatz je Sekunde (plan/m11.md, Zeile 21). */
#define NVS_PERIOD_MS 1000U

/* Der Stack der Aufgabe in Byte. */
#define NVS_STACK_BYTES 3072U

/* Der Namensraum `takt` in NVS; nur die Aufgabe schreibt hinein. */
static nvs_handle_t load_nvs_handle;

static void load_nvs(void *arg) {
    static uint8_t record[NVS_RECORD_BYTES];
    (void)arg;
    TickType_t wake = xTaskGetTickCount();
    uint32_t n = 0U;
    for (;;) {
        record[n % NVS_RECORD_BYTES] = (uint8_t)n;
        n++;
        (void)nvs_set_blob(load_nvs_handle, "last", record, sizeof record);
        (void)nvs_commit(load_nvs_handle);
        vTaskDelayUntil(&wake, pdMS_TO_TICKS(NVS_PERIOD_MS));
    }
}

int load_nvs_start(void) {
    int rc = -1;
    esp_err_t err = nvs_flash_init();
    if ((err == ESP_ERR_NVS_NO_FREE_PAGES) || (err == ESP_ERR_NVS_NEW_VERSION_FOUND)) {
        if (nvs_flash_erase() == ESP_OK) {
            err = nvs_flash_init();
        }
    }
    if ((err == ESP_OK) && (nvs_open("takt", NVS_READWRITE, &load_nvs_handle) == ESP_OK) &&
        (xTaskCreate(load_nvs, "takt-nvs-last", NVS_STACK_BYTES, NULL, LOAD_PRIORITY, NULL) == pdPASS)) {
        rc = 0;
    }
    return rc;
}
