#include <errno.h>
#include <inttypes.h>
#include <libproc.h>
#include <mach/mach_time.h>
#include <signal.h>
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>
#include <sys/time.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

extern char **environ;

static uint64_t ns(void) { return clock_gettime_nsec_np(CLOCK_MONOTONIC_RAW); }

int main(int argc, char **argv) {
    if (argc < 3) { fprintf(stderr, "usage: runstat OUT cmd...\n"); return 2; }
    const char *out = argv[1];
    uint64_t start = ns();
    pid_t pid;
    posix_spawnattr_t attr;
    posix_spawnattr_init(&attr);
    int rc = posix_spawnp(&pid, argv[2], NULL, &attr, &argv[2], environ);
    if (rc) { fprintf(stderr, "spawn %s: %s\n", argv[2], strerror(rc)); return 127; }
    siginfo_t info;
    while (waitid(P_PID, pid, &info, WEXITED | WNOWAIT) == -1 && errno == EINTR) {}
    uint64_t end = ns();
    struct rusage_info_v4 ri = {0};
    int have = proc_pid_rusage(pid, RUSAGE_INFO_V4, (rusage_info_t *)&ri) == 0;
    int status = 0;
    struct rusage ru = {0};
    while (wait4(pid, &status, 0, &ru) == -1 && errno == EINTR) {}
    mach_timebase_info_data_t tb;
    mach_timebase_info(&tb);
    FILE *f = fopen(out, "w");
    if (f) {
        fprintf(f, "{\"start_ns\":%" PRIu64 ",\"end_ns\":%" PRIu64 ",\"pid\":%d,\"exit\":%d,\"signal\":%d,"
                   "\"user_us\":%ld,\"sys_us\":%ld,\"maxrss_bytes\":%ld,\"minflt\":%ld,\"majflt\":%ld,\"nvcsw\":%ld,\"nivcsw\":%ld",
                start, end, pid, WIFEXITED(status) ? WEXITSTATUS(status) : -1, WIFSIGNALED(status) ? WTERMSIG(status) : 0,
                (long)(ru.ru_utime.tv_sec * 1000000 + ru.ru_utime.tv_usec), (long)(ru.ru_stime.tv_sec * 1000000 + ru.ru_stime.tv_usec),
                ru.ru_maxrss, ru.ru_minflt, ru.ru_majflt, ru.ru_nvcsw, ru.ru_nivcsw);
        if (have)
            fprintf(f, ",\"ri_user_ns\":%" PRIu64 ",\"ri_sys_ns\":%" PRIu64 ",\"lifetime_max_footprint\":%" PRIu64 ",\"footprint\":%" PRIu64 ",\"instructions\":%" PRIu64 ",\"cycles\":%" PRIu64,
                    ri.ri_user_time * tb.numer / tb.denom, ri.ri_system_time * tb.numer / tb.denom, ri.ri_lifetime_max_phys_footprint, ri.ri_phys_footprint, ri.ri_instructions, ri.ri_cycles);
        fprintf(f, "}\n");
        fclose(f);
    }
    if (WIFSIGNALED(status)) return 128 + WTERMSIG(status);
    return WIFEXITED(status) ? WEXITSTATUS(status) : 1;
}
