#!/usr/bin/env bash
# Baut und prueft, was auf die MCU geht (12.3, 12.8).
#
# **Warum ein eigenes Skript.** Das Board-Crate liegt ausserhalb des
# Workspace — Registerzugriff braucht `unsafe`, und `forbid` (13.4) laesst
# sich nicht lokal aufheben. Ein Crate ausserhalb des Workspace wird von
# `cargo test --workspace` nicht erfasst und darum leicht vergessen; genau
# das verhindert dieses Skript.
#
# Drei Pruefungen, jede mit eigenem Zweck:
#
#   1. Die `no_std`-Kerne bauen fuer *beide* M5-Ziele — ohne Board-Crate.
#      Geht das nicht mehr, ist Board-Wissen in eine Schicht gesickert,
#      die keines haben darf (plan/m5.md 2.2).
#   2. Das Board-Crate baut fuer sein Ziel und ist clippy-sauber.
#   3. Die rechnende Haelfte (`takt-board-support`) laeuft auf dem Wirt
#      mit ihren Tests. Sie ist der Grund, warum die Ausnahme klein ist:
#      Perioden, Prescaler, Zaehlerueberlauf und Zyklenumrechnung sind
#      dort, wo sie geprueft werden koennen.
set -euo pipefail

cd "$(dirname "$0")/.."

targets=(thumbv7em-none-eabihf riscv32imac-unknown-none-elf)
cores=(takt-rt-core takt-rt-baremetal)

for t in "${targets[@]}"; do
    if ! rustup target list --installed | grep -qx "$t"; then
        echo "Ziel $t fehlt — mit 'rustup target add $t' nachruesten" >&2
        exit 1
    fi
done

echo "== 1. Die Kerne, fuer beide Ziele, ohne Board"
for t in "${targets[@]}"; do
    for c in "${cores[@]}"; do
        echo "-- $c fuer $t"
        cargo build -p "$c" --target "$t" "$@"
    done
done

echo
echo "== 2. Das Board-Crate (eigener Workspace, thumbv7em)"
#
# Ohne `--all-targets`: Tests fuer ein `no_std`-Ziel brauchen einen
# Testlaeufer und einen `panic_handler`, die es dort nicht gibt. Was
# testbar ist, liegt ohnehin in `takt-board-support` — und genau das ist
# der Grund fuer die Trennung.
(
    cd crates/takt-board-blackpill
    cargo build --target thumbv7em-none-eabihf "$@"
    cargo clippy --target thumbv7em-none-eabihf "$@" -- -D warnings
)

echo
echo "== 3. Die rechnende Haelfte auf dem Wirt"
cargo test -p takt-board-support "$@"

echo
echo "Alles gebaut und geprueft."
