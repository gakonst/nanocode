#ifndef _STDLIB_H
#define _STDLIB_H
#include <stddef.h>
void *malloc(size_t);
void *calloc(size_t, size_t);
void *realloc(void *, size_t);
void free(void *);
void abort(void);
long strtol(const char *, char **, int);
unsigned long strtoul(const char *, char **, int);
double strtod(const char *, char **);
static inline int abs(int value) { return value < 0 ? -value : value; }
static inline long labs(long value) { return value < 0 ? -value : value; }
static inline long long llabs(long long value) { return value < 0 ? -value : value; }
#endif
