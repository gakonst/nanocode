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
#define NANOCODE_STACK_FRAMES 16384
#define NANOCODE_STACK_FIRST 16
#define NANOCODE_STACK_FRAME_BYTES 4096
#define NANOCODE_STACK_RESERVE_BYTES 49152
#define NANOCODE_STACK_PROBE_BYTES 131072
#define NANOCODE_STACK_PROBE_LIMIT 1048576
#define NANOCODE_STACK_OFFSET 0x40000000u
__attribute__((import_module("host"), import_name("stack_room"))) extern uint32_t nanocode_stack_room(uint32_t want, uint32_t entry);
__attribute__((unused)) static uintptr_t nanocode_stack_frames[NANOCODE_STACK_FRAMES];
__attribute__((unused)) static volatile uint32_t nanocode_stack_entry;
__attribute__((unused)) static uint32_t nanocode_stack_entered;
__attribute__((unused)) static uint32_t nanocode_stack_proven;
__attribute__((unused)) static size_t nanocode_stack_count;
__attribute__((unused)) static size_t nanocode_stack_base = NANOCODE_STACK_FIRST;
__attribute__((unused)) static size_t nanocode_stack_checked;
__attribute__((unused)) static size_t nanocode_stack_span;
__attribute__((unused)) static size_t nanocode_stack_calls;
__attribute__((unused)) static inline uintptr_t nanocode_stack_pointer(uintptr_t sp)
{
    size_t count = nanocode_stack_count;
    while (count && nanocode_stack_frames[count - 1] <= sp)
        count--;
    if (nanocode_stack_entry != nanocode_stack_entered) {
        nanocode_stack_entered = nanocode_stack_entry;
        nanocode_stack_base = NANOCODE_STACK_FIRST;
        nanocode_stack_proven = 0;
        nanocode_stack_checked = 0;
        nanocode_stack_span = 0;
    } else if (count < nanocode_stack_checked) {
        nanocode_stack_checked = count;
    }
    if (count < NANOCODE_STACK_FRAMES)
        nanocode_stack_frames[count++] = sp;
    nanocode_stack_count = count;
    if ((count >= nanocode_stack_base && count >= nanocode_stack_checked + nanocode_stack_span) || (count == NANOCODE_STACK_FRAMES && ++nanocode_stack_calls % 4 == 0)) {
        uint32_t want = nanocode_stack_proven >= NANOCODE_STACK_PROBE_LIMIT / 2 ? NANOCODE_STACK_PROBE_LIMIT : nanocode_stack_proven * 2 > NANOCODE_STACK_PROBE_BYTES ? nanocode_stack_proven * 2 : NANOCODE_STACK_PROBE_BYTES;
        uint32_t room = nanocode_stack_room(want, (uint32_t)(uintptr_t)&nanocode_stack_entry);
        if (room < NANOCODE_STACK_RESERVE_BYTES + NANOCODE_STACK_FRAME_BYTES)
            return NANOCODE_STACK_OFFSET / 2;
        nanocode_stack_proven = room;
        nanocode_stack_checked = count;
        nanocode_stack_span = (room - NANOCODE_STACK_RESERVE_BYTES) / NANOCODE_STACK_FRAME_BYTES;
        if (nanocode_stack_span > nanocode_stack_base)
            nanocode_stack_base = nanocode_stack_span;
    }
    return sp + NANOCODE_STACK_OFFSET;
}
#define __builtin_frame_address(level) ((void *)nanocode_stack_pointer((uintptr_t)__builtin_frame_address(level)))
