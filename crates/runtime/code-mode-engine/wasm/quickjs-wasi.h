#include <alloca.h>
#include <assert.h>
#include <ctype.h>
#include <errno.h>
#include <inttypes.h>
#include <limits.h>
#include <math.h>
#include <stdarg.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/time.h>
#include <time.h>
#include <unistd.h>
#undef __wasi__
#define NANOCODE_STACK_FRAMES 4096
#define NANOCODE_STACK_FRAME_BYTES 192
#define NANOCODE_STACK_SCALE 2
__attribute__((unused)) static uintptr_t nanocode_stack_frames[NANOCODE_STACK_FRAMES];
__attribute__((unused)) static size_t nanocode_stack_count;
__attribute__((unused)) static inline uintptr_t nanocode_stack_pointer(uintptr_t sp)
{
    size_t count = nanocode_stack_count;
    while (count && nanocode_stack_frames[count - 1] <= sp)
        count--;
    if (count < NANOCODE_STACK_FRAMES)
        nanocode_stack_frames[count++] = sp;
    nanocode_stack_count = count;
    return sp + 0x40000000u - (nanocode_stack_frames[0] - sp) * (NANOCODE_STACK_SCALE - 1) - count * NANOCODE_STACK_FRAME_BYTES;
}
#define __builtin_frame_address(level) ((void *)nanocode_stack_pointer((uintptr_t)__builtin_frame_address(level)))
