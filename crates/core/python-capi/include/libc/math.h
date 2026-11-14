#ifndef _MATH_H
#define _MATH_H
#define HUGE_VAL __builtin_huge_val()
#define INFINITY __builtin_inff()
#define NAN __builtin_nanf("")
#define isnan(x) __builtin_isnan(x)
#define isinf(x) __builtin_isinf(x)
#define isfinite(x) __builtin_isfinite(x)
#define signbit(x) __builtin_signbit(x)
#define copysign __builtin_copysign
#define fabs __builtin_fabs
#define floor __builtin_floor
#define ceil __builtin_ceil
#define sqrt __builtin_sqrt
#define trunc __builtin_trunc
double fmod(double, double);
double pow(double, double);
double exp(double);
double log(double);
double ldexp(double, int);
double frexp(double, int *);
double modf(double, double *);
#endif
