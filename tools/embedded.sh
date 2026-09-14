#!/usr/bin/env bash
# Baut die `no_std`-Crates fuer die beiden M5-Ziele (12.3, 12.8).
#
# Der Zweck ist nicht das Artefakt, sondern die Probe: `takt-rt-core` und
# `takt-rt-baremetal` muessen **ohne Board-Crate** fuer beide Ziele bauen.
# Geht das nicht mehr, ist Board-Wissen in eine Schicht gesickert, die
# keines haben darf (plan/m5.md 2.2) — und das faellt sonst erst auf, wenn
# das erste Board da ist und die Schichten nicht mehr zu trennen sind.
#
# Die beiden Ziele liegen in *verschiedenen* Zielklassen (12.8): Cortex-M4F
# rechnet `f32` in Hardware, RV32IMAC in Software. Was hier baut, ist damit
# gegen beide Numerikwege geprueft.
set -euo pipefail

targets=(thumbv7em-none-eabihf riscv32imac-unknown-none-elf)
crates=(takt-rt-core takt-rt-baremetal)

for t in "${targets[@]}"; do
    if ! rustup target list --installed | grep -qx "$t"; then
        echo "Ziel $t fehlt — mit 'rustup target add $t' nachruesten" >&2
        exit 1
    fi
done

for t in "${targets[@]}"; do
    for c in "${crates[@]}"; do
        echo "== $c fuer $t"
        cargo build -p "$c" --target "$t" "$@"
    done
done

echo
echo "Beide Ziele, beide Crates: gebaut ohne Board."
