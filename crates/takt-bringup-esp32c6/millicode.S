// Millicode fuer `-msave-restore` (RV32, psABI): Prolog und Epilog rufen
// `__riscv_save_N` und `__riscv_restore_N` statt jeden Sicherungsbefehl
// selbst zu tragen. Rusts `compiler_builtins` liefert die Routinen nicht.
//
// Nach compiler-rt `lib/builtins/riscv/{save,restore}.S`:
// Part of the LLVM Project, under the Apache License v2.0 with LLVM Exceptions.
// See https://llvm.org/LICENSE.txt for license information.
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception

  .text
  .option nopic

  .globl  __riscv_save_12
  .type   __riscv_save_12,@function
__riscv_save_12:
  addi   sp, sp, -64
  mv     t1, zero
  sw     s11, 12(sp)
  j      .Lriscv_save_11_8

  .globl  __riscv_save_11
  .type   __riscv_save_11,@function
  .globl  __riscv_save_10
  .type   __riscv_save_10,@function
  .globl  __riscv_save_9
  .type   __riscv_save_9,@function
  .globl  __riscv_save_8
  .type   __riscv_save_8,@function
__riscv_save_11:
__riscv_save_10:
__riscv_save_9:
__riscv_save_8:
  addi   sp, sp, -64
  li     t1, 16
.Lriscv_save_11_8:
  sw     s10, 16(sp)
  sw     s9,  20(sp)
  sw     s8,  24(sp)
  sw     s7,  28(sp)
  j      .Lriscv_save_7_4

  .globl  __riscv_save_7
  .type   __riscv_save_7,@function
  .globl  __riscv_save_6
  .type   __riscv_save_6,@function
  .globl  __riscv_save_5
  .type   __riscv_save_5,@function
  .globl  __riscv_save_4
  .type   __riscv_save_4,@function
__riscv_save_7:
__riscv_save_6:
__riscv_save_5:
__riscv_save_4:
  addi   sp, sp, -64
  li     t1, 32
.Lriscv_save_7_4:
  sw     s6, 32(sp)
  sw     s5, 36(sp)
  sw     s4, 40(sp)
  sw     s3, 44(sp)
  sw     s2, 48(sp)
  sw     s1, 52(sp)
  sw     s0, 56(sp)
  sw     ra, 60(sp)
  add    sp, sp, t1
  jr     t0

  .globl  __riscv_save_3
  .type   __riscv_save_3,@function
  .globl  __riscv_save_2
  .type   __riscv_save_2,@function
  .globl  __riscv_save_1
  .type   __riscv_save_1,@function
  .globl  __riscv_save_0
  .type   __riscv_save_0,@function
__riscv_save_3:
__riscv_save_2:
__riscv_save_1:
__riscv_save_0:
  addi    sp, sp, -16
  sw      s2,  0(sp)
  sw      s1,  4(sp)
  sw      s0,  8(sp)
  sw      ra,  12(sp)
  jr      t0

  .globl  __riscv_restore_12
  .type   __riscv_restore_12,@function
__riscv_restore_12:
  lw      s11, 12(sp)
  addi    sp, sp, 16

  .globl  __riscv_restore_11
  .type   __riscv_restore_11,@function
  .globl  __riscv_restore_10
  .type   __riscv_restore_10,@function
  .globl  __riscv_restore_9
  .type   __riscv_restore_9,@function
  .globl  __riscv_restore_8
  .type   __riscv_restore_8,@function
__riscv_restore_11:
__riscv_restore_10:
__riscv_restore_9:
__riscv_restore_8:
  lw      s10, 0(sp)
  lw      s9,  4(sp)
  lw      s8,  8(sp)
  lw      s7,  12(sp)
  addi    sp, sp, 16

  .globl  __riscv_restore_7
  .type   __riscv_restore_7,@function
  .globl  __riscv_restore_6
  .type   __riscv_restore_6,@function
  .globl  __riscv_restore_5
  .type   __riscv_restore_5,@function
  .globl  __riscv_restore_4
  .type   __riscv_restore_4,@function
__riscv_restore_7:
__riscv_restore_6:
__riscv_restore_5:
__riscv_restore_4:
  lw      s6,  0(sp)
  lw      s5,  4(sp)
  lw      s4,  8(sp)
  lw      s3,  12(sp)
  addi    sp, sp, 16

  .globl  __riscv_restore_3
  .type   __riscv_restore_3,@function
  .globl  __riscv_restore_2
  .type   __riscv_restore_2,@function
  .globl  __riscv_restore_1
  .type   __riscv_restore_1,@function
  .globl  __riscv_restore_0
  .type   __riscv_restore_0,@function
__riscv_restore_3:
__riscv_restore_2:
__riscv_restore_1:
__riscv_restore_0:
  lw      s2,  0(sp)
  lw      s1,  4(sp)
  lw      s0,  8(sp)
  lw      ra,  12(sp)
  addi    sp, sp, 16
  ret
