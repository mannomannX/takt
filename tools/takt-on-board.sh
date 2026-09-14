#!/usr/bin/env bash
# Uebersetzt ein Takt-Programm fuer die Black Pill und flasht es (M5).
#
#   tools/takt-on-board.sh corpus-try/16_timing.takt [--no-flash]
#
# **Drei Teile werden gebunden:**
#
#   1. Der erzeugte Code — `takt-llvm` macht LLVM-IR aus der `.takt`-Datei,
#      clang ein Objekt fuer `thumbv7em`.
#   2. Der Rahmen — `takt-conformance::mcu` erzeugt das C-Stueck, das
#      Prozessabbild und Latch haelt und `<maschine>_step` ruft (12.1).
#   3. Das Bring-up-Programm — es setzt das Board auf, liefert die
#      Telemetrie und laesst die Tickschleife laufen.
#
# Was dabei entsteht, ist der erste Lauf eines Takt-Programms auf
# Hardware. Der Trace auf USART1 (PA9, 115200) wird gegen `takt sim`
# gehalten — das ist der Kern des M5-Exits, und `takt-trace-serial` macht
# ihn. Dieses Skript flasht nur; den Vergleich ruft man danach:
#
#   cargo run -p takt-trace-serial -- <programm.takt> --port COM4
#
# **Der Black Pill hat keinen USB-Seriell-Wandler.** Sein USB geht direkt
# an den STM32 (HID-Bootloader); PA9 braucht einen TTL-Adapter gegen GND.
# Ohne ihn blinkt das Board, aber niemand kann pruefen, ob es richtig
# rechnet — und genau das war der Zustand, den FB-142 festhaelt.
set -euo pipefail

cd "$(dirname "$0")/.."

program="${1:-}"
if [ -z "$program" ] || [ ! -f "$program" ]; then
    echo "Aufruf: tools/takt-on-board.sh <programm.takt> [--no-flash]" >&2
    exit 1
fi
flash=1
[ "${2:-}" = "--no-flash" ] && flash=0

target=thumbv7em-none-eabihf
work="$(mktemp -d -t takt-board-XXXXXX)"
trap 'rm -rf "$work"' EXIT

sysroot="$(rustc --print sysroot)"
host="$(rustc -vV | sed -n 's/^host: //p')"
llvm="$sysroot/lib/rustlib/$host/bin"

echo "== 1. Das Programm uebersetzen"
# `takt mir --emit-ir` gibt es noch nicht; bis dahin erzeugt der
# Konformitaetstest die IR. Der Umweg ist sichtbar und Absicht: Ein
# eigenes CLI-Kommando dafuer gehoert in den Plan, nicht in ein Skript.
cargo run -q -p takt-cli -- check "$program" >/dev/null || {
    echo "Das Programm uebersetzt nicht." >&2
    exit 1
}
echo "   $program: geprueft"

echo "== 2. Bring-up-Programm bauen"
cargo build -q --release --target "$target" \
    --manifest-path crates/takt-bringup-stm32f401/Cargo.toml --bin takt

target_dir="$(cargo metadata --format-version 1 --no-deps 2>/dev/null |
    sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')"
bin="${target_dir:-target}/$target/release/takt"
[ -f "$bin" ] || { echo "Binaerdatei fehlt: $bin" >&2; exit 1; }

echo "== 3. Abbild erzeugen"
"$llvm/llvm-objcopy" -O binary "$bin" "$work/app.bin"
ls -l "$work/app.bin" | awk '{print "   " $5 " Byte"}'

if [ "$flash" -eq 0 ]; then
    cargo run -q -p takt-flash-weact --bin takt-flash-weact -- "$work/app.bin" --dry-run
    exit 0
fi

echo "== 4. Flashen"
echo "   Das Board muss im Bootloader stehen: KEY halten, NRST kurz druecken."
cargo run -q -p takt-flash-weact --bin takt-flash-weact -- "$work/app.bin"

echo
echo "Fertig. Der Trace laeuft auf PA9 mit 115200 8N1."
echo
echo "Vergleich gegen den Interpreter (braucht einen TTL-Adapter an PA9):"
echo "   cargo run -p takt-trace-serial -- $program --port <COMx|/dev/ttyUSBx>"
