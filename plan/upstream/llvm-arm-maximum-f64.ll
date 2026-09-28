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
