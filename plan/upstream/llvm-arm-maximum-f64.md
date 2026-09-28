# Meldung an LLVM: `llvm.maximum.f64` auf thumbv7em (FB-290)

Entwurf einer Fehlermeldung für <https://github.com/llvm/llvm-project/issues>.
Abgeschickt wird sie vom Konto des Projekts, nicht aus einer Sitzung heraus:
Eine Meldung geht nach außen. Der Text unten ist englisch, weil der
Tracker es ist; Reproducer ist `llvm-arm-maximum-f64.ll` daneben.

Stand 2026-09-28: mit clang 22.1.8 (`ca7933e47d3a`) erneut reproduziert.
Im Projekt ist der Fall umgangen, ohne eine Umgehung im Codegen: Die
Endlichkeitsprüfung der Matrizen prüft je Element statt über `maximum`
(FB-297), und `46_matrices` läuft auf dem F401 ohne Abweichung.

---

**Title:** [ARM] Cannot select `setcc i64` after soft-float expansion of `llvm.maximum.f64` on Cortex-M4 (FPv4-SP)

**Description**

Valid IR that passes the verifier aborts instruction selection when compiled
for a Cortex-M4 with single-precision FPU (`-mfloat-abi=hard`, so `double`
is soft-float). The combination is `fabs` on two doubles, `llvm.maximum.f64`
of the results and an `fcmp one` against infinity.

```llvm
target triple = "thumbv7em-none-unknown-eabihf"

declare double @llvm.fabs.f64(double)
declare double @llvm.maximum.f64(double, double)

define i1 @finite_pair(double %a, double %b) {
  %fa = call double @llvm.fabs.f64(double %a)
  %fb = call double @llvm.fabs.f64(double %b)
  %m = call double @llvm.maximum.f64(double %fa, double %fb)
  %ok = fcmp one double %m, 0x7FF0000000000000
  ret i1 %ok
}
```

**Command**

```
clang -c -Os --target=thumbv7em-none-eabihf -mcpu=cortex-m4 -mfloat-abi=hard repro.ll -o repro.o
```

**Actual**

```
fatal error: error in backend: Cannot select: i32 = setcc ..., Constant:i64<0>, seteq:ch
  ... i64 = build_pair (ARMISD::VMOVRRD ...)
```

**Expected**

An object file. The same IR compiles for x86-64 and riscv32imac.

**Notes**

The `setcc` on `i64` appears to come from the expansion of `fmaximum` for
a soft-float `double` (the zero/sign handling of `maximum`), after the
type legalizer has already run. Replacing `llvm.maximum.f64` by `maxnum`
or by explicit compares avoids the abort.

**Environment**

clang version 22.1.8 (https://github.com/llvm/llvm-project ca7933e47d3a3451d81e72ac174dcb5aa28b59d1), host x86_64-pc-windows-msvc.
