#ifndef _TIME_H
#define _TIME_H
#include <sys/types.h>
typedef long long clock_t;
#define CLOCKS_PER_SEC 1000000
clock_t clock(void);
time_t time(time_t *);
#endif
