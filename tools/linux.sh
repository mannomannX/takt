#!/usr/bin/env bash
# Laeuft einen Befehl unter Linux, fuer die Teile, die nur dort laufen
# (Referenz 12.2: `linux_rt`) und fuer aarch64 (Schritt 11).
#
#     tools/linux.sh cargo test -p takt-rt-linux
#     tools/linux.sh cargo run -q -p takt-rt-linux --example befund
#     tools/linux.sh --rt cargo run --release -p takt-rt-linux --example jitter
#
# `--rt` gibt dem Container `SYS_NICE` und startet den Befehl mit
# `chrt -f 80 taskset -c 0` — damit gilt die Zeitgarantie aus 12.2, und
# eine Messung unter ihr ist eine Messung.
#
# WARUM DIESES SKRIPT
#
# Der Quelltext liegt auf einer Windows-Platte und wird ueber 9p in den
# Container gereicht. Fuer Lesevorgaenge ist das tragbar, fuer die
# Schreibvorgaenge eines Builds nicht: `cargo` legt Tausende kleiner
# Dateien an, und jede kostet einen Roundtrip. `--target-dir` zeigt darum
# in das Dateisystem des Containers, und ein benanntes Volume haelt es
# ueber Laeufe hinweg — sonst uebersetzt jeder Lauf alle Abhaengigkeiten neu.
#
# MESSUNGEN
#
# Ein Container teilt sich den Kern mit dem Host, und seine Uhr ist die
# virtualisierte. Zahlen von hier sind Hausnummern, keine Abnahme; die
# Kalibrierung (`takt bench`, 13.8) gehoert auf die Zielhardware. Was
# sich hier pruefen laesst, ist die *Richtigkeit* der Pfade — dass
# `clock_nanosleep_absolute` laeuft, dass `Guarantee` meldet, was
# tatsaechlich gilt.
set -euo pipefail

BILD="${TAKT_LINUX_IMAGE:-takt-linux}"
CACHE="takt-linux-cache"
RT=0

if [ "${1:-}" = "--rt" ]; then
    RT=1
    shift
fi

if [ $# -eq 0 ]; then
    sed -n '2,30p' "$0" | sed 's/^# \?//'
    exit 1
fi

# Das Volume haelt Registry und Build-Verzeichnis; ohne es uebersetzt
# jeder Lauf alle Abhaengigkeiten neu.
docker volume create "$CACHE" >/dev/null

ARGS=(--rm
      -v "$(cd "$(dirname "$0")/.." && pwd -W 2>/dev/null || pwd):/takt"
      -v "$CACHE:/cache"
      -w /takt
      -e CARGO_TARGET_DIR=/cache/target
      -e CARGO_HOME=/cache/cargo)

if [ "$RT" = "1" ]; then
    ARGS+=(--cap-add=SYS_NICE)
    BEFEHL="apt-get install -y -qq util-linux >/dev/null 2>&1 || true; chrt -f 80 taskset -c 0 $*"
else
    BEFEHL="$*"
fi

MSYS_NO_PATHCONV=1 exec docker run "${ARGS[@]}" "$BILD" bash -c "$BEFEHL"
