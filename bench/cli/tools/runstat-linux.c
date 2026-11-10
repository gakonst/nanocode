#define _GNU_SOURCE
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <inttypes.h>
#include <linux/perf_event.h>
#include <pthread.h>
#include <signal.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/prctl.h>
#include <sys/resource.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

// Linux counterpart of runstat.c. Runs a command in its own process group as a child subreaper, so every helper it
// starts stays in the tree, samples /proc for the tree's summed RSS and process count, kills what is left when the
// command exits, and writes JSON. Env: RUNSTAT_INTERVAL_US (sampler period, default 2000, 0 = off),
// RUNSTAT_CGROUP (a writable cgroup v2 directory to create the run's cgroup in; default the parent of our own),
// RUNSTAT_PERF=1 (count instructions with perf_event_open; off by default because on a VM the counter's context
// switch cost inflated nc --version from 2.4 to 8 ms of CPU and an nc TUI session from 160 to 380 ms).

#define MAXP 8192
typedef struct { int pid; unsigned long long start, cpu; } proc_t;
static proc_t seen[MAXP];
static int nseen, procs_max, helpers_max, samples, self_pid, main_pid;
static uint64_t rss_max;
static long tick, page;
static atomic_int stop;
static pthread_mutex_t mu = PTHREAD_MUTEX_INITIALIZER;
static volatile sig_atomic_t fwd;

static uint64_t ns(void) { struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t); return (uint64_t)t.tv_sec * 1000000000ull + t.tv_nsec; }

static int readfile(const char *path, char *buf, int n) {
    int fd = open(path, O_RDONLY | O_CLOEXEC);
    if (fd < 0) return -1;
    int k = read(fd, buf, n - 1);
    close(fd);
    if (k < 0) return -1;
    buf[k] = 0;
    return k;
}

static int writefile(const char *path, const char *s) {
    int fd = open(path, O_WRONLY | O_CLOEXEC);
    if (fd < 0) return -1;
    int ok = write(fd, s, strlen(s)) == (ssize_t)strlen(s);
    close(fd);
    return ok ? 0 : -1;
}

typedef struct { int pid, ppid; char state; unsigned long long cpu, start; } stat_t;

static int readstat(int pid, stat_t *s) {
    char path[64], buf[1024];
    snprintf(path, sizeof path, "/proc/%d/stat", pid);
    if (readfile(path, buf, sizeof buf) <= 0) return -1;
    char *p = strrchr(buf, ')');
    if (!p) return -1;
    unsigned long long ut, st, start;
    if (sscanf(p + 2, "%c %d %*d %*d %*d %*d %*u %*u %*u %*u %*u %llu %llu %*d %*d %*d %*d %*d %*d %llu", &s->state, &s->ppid, &ut, &st, &start) != 5) return -1;
    s->pid = pid; s->cpu = ut + st; s->start = start;
    return 0;
}

// Membership is decided once per pid from its parent chain: as a child subreaper we inherit every orphan in the tree,
// so a process is in the tree exactly when it descends from us. Only new pids and members are read on each pass.
#define PIDMAX 4194304
static unsigned char cls[PIDMAX];

static int classify(int pid, stat_t *s, int depth) {
    stat_t tmp;
    if (pid <= 0 || pid >= PIDMAX) return 1;
    if (!s) s = &tmp;
    if (cls[pid] == 1) return 1;
    if (readstat(pid, s)) return 0;
    if (cls[pid] == 2) return 2;
    int p = s->ppid;
    int c = p == self_pid ? 2 : p <= 1 || p >= PIDMAX || depth > 64 ? 1 : cls[p] ? cls[p] : classify(p, NULL, depth + 1);
    if (c) cls[pid] = c;
    return c;
}

// Live (non-zombie) members of the tree; fills pids and their stat, returns the count.
static int tree(int *pids, int cap, stat_t *out) {
    int k = 0;
    DIR *d = opendir("/proc");
    if (!d) return 0;
    struct dirent *e;
    stat_t s;
    while ((e = readdir(d)) && k < cap) {
        if (e->d_name[0] < '0' || e->d_name[0] > '9') continue;
        int pid = atoi(e->d_name);
        if (pid == self_pid || classify(pid, &s, 0) != 2 || s.state == 'Z') continue;
        pids[k] = pid;
        if (out) out[k] = s;
        k++;
    }
    closedir(d);
    return k;
}

static uint64_t rss_of(int pid) {
    char path[64], buf[256];
    snprintf(path, sizeof path, "/proc/%d/statm", pid);
    unsigned long long size, res;
    if (readfile(path, buf, sizeof buf) <= 0 || sscanf(buf, "%llu %llu", &size, &res) != 2) return 0;
    return res * page;
}

static void sample(void) {
    static int pids[MAXP];
    static stat_t st[MAXP];
    int n = tree(pids, MAXP, st);
    uint64_t rss = 0;
    int helpers = 0;
    for (int i = 0; i < n; i++) {
        rss += rss_of(pids[i]);
        if (pids[i] != main_pid) helpers++;
    }
    pthread_mutex_lock(&mu);
    for (int i = 0; i < n; i++) {
        int j;
        for (j = 0; j < nseen && !(seen[j].pid == st[i].pid && seen[j].start == st[i].start); j++) {}
        if (j == nseen && nseen < MAXP) seen[nseen++] = (proc_t){st[i].pid, st[i].start, 0};
        if (j < MAXP) seen[j].cpu = st[i].cpu;
    }
    if (rss > rss_max) rss_max = rss;
    if (n > procs_max) procs_max = n;
    if (helpers > helpers_max) helpers_max = helpers;
    samples++;
    pthread_mutex_unlock(&mu);
}

static long interval_us;
static void *sampler(void *arg) {
    (void)arg;
    struct timespec next;
    clock_gettime(CLOCK_MONOTONIC, &next);
    while (!atomic_load(&stop)) {
        sample();
        next.tv_nsec += interval_us * 1000;
        while (next.tv_nsec >= 1000000000) { next.tv_nsec -= 1000000000; next.tv_sec++; }
        clock_nanosleep(CLOCK_MONOTONIC, TIMER_ABSTIME, &next, NULL);
    }
    return NULL;
}

static void forward(int sig) { fwd = sig; if (main_pid > 0) kill(-main_pid, sig); }

static long long cg_value(const char *dir, const char *file, const char *key) {
    char path[4200], buf[4096];
    snprintf(path, sizeof path, "%s/%s", dir, file);
    if (readfile(path, buf, sizeof buf) <= 0) return -1;
    if (!key) return atoll(buf);
    char *p = strstr(buf, key);
    return p ? atoll(p + strlen(key) + 1) : -1;
}

static int cg_create(char *dir, size_t n) {
    const char *parent = getenv("RUNSTAT_CGROUP");
    char own[4096], base[4200];
    if (!parent || !*parent) {
        if (readfile("/proc/self/cgroup", own, sizeof own) <= 0 || strncmp(own, "0::", 3)) return -1;
        own[strcspn(own, "\n")] = 0;
        char *slash = strrchr(own + 3, '/');
        if (!slash || slash == own + 3) return -1;
        *slash = 0;
        snprintf(base, sizeof base, "/sys/fs/cgroup%s", own + 3);
        parent = base;
    }
    snprintf(dir, n, "%s/runstat-%d", parent, self_pid);
    if (mkdir(dir, 0755)) return -1;
    char path[4400], type[64];
    snprintf(path, sizeof path, "%s/cgroup.type", dir);
    if (readfile(path, type, sizeof type) > 0 && !strncmp(type, "domain invalid", 14)) writefile(path, "threaded");
    return 0;
}

static void json_num(FILE *f, const char *k, long long v, int ok) { if (ok) fprintf(f, ",\"%s\":%lld", k, v); else fprintf(f, ",\"%s\":null", k); }

int main(int argc, char **argv) {
    if (argc < 3) { fprintf(stderr, "usage: runstat OUT cmd...\n"); return 2; }
    const char *out = argv[1];
    page = sysconf(_SC_PAGESIZE);
    tick = sysconf(_SC_CLK_TCK);
    self_pid = getpid();
    interval_us = getenv("RUNSTAT_INTERVAL_US") ? atol(getenv("RUNSTAT_INTERVAL_US")) : 2000;
    prctl(PR_SET_CHILD_SUBREAPER, 1);
    char cg[4300];
    int have_cg = cg_create(cg, sizeof cg) == 0;
    int tty = isatty(0) && tcgetpgrp(0) == getpgrp();
    int go[2], fail[2];
    if (pipe2(go, O_CLOEXEC) || pipe2(fail, O_CLOEXEC)) { perror("pipe"); return 127; }
    uint64_t start = ns();
    pid_t pid = fork();
    if (pid < 0) { perror("fork"); return 127; }
    if (pid == 0) {
        setpgid(0, 0);
        if (tty) { signal(SIGTTOU, SIG_IGN); tcsetpgrp(0, getpid()); signal(SIGTTOU, SIG_DFL); }
        if (have_cg) {
            char p[4400];
            snprintf(p, sizeof p, "%s/cgroup.procs", cg);
            if (writefile(p, "0")) { snprintf(p, sizeof p, "%s/cgroup.threads", cg); writefile(p, "0"); }
        }
        char c;
        close(go[1]);
        if (read(go[0], &c, 1) != 1) _exit(127);
        execvp(argv[2], &argv[2]);
        int err = errno;
        if (write(fail[1], &err, sizeof err) < 0) {}
        _exit(127);
    }
    main_pid = pid;
    setpgid(pid, pid);
    if (tty) { signal(SIGTTOU, SIG_IGN); tcsetpgrp(0, pid); }
    signal(SIGHUP, SIG_IGN); signal(SIGINT, SIG_IGN); signal(SIGQUIT, SIG_IGN);
    signal(SIGTERM, forward);
    struct perf_event_attr pa = {0};
    pa.size = sizeof pa; pa.type = PERF_TYPE_HARDWARE; pa.config = PERF_COUNT_HW_INSTRUCTIONS;
    pa.disabled = 1; pa.enable_on_exec = 1; pa.inherit = 1; pa.exclude_hv = 1;
    int no_perf = !getenv("RUNSTAT_PERF") || strcmp(getenv("RUNSTAT_PERF"), "1");
    int perf = no_perf ? -1 : syscall(SYS_perf_event_open, &pa, pid, -1, -1, PERF_FLAG_FD_CLOEXEC), perf_kernel = 1;
    if (perf < 0 && !no_perf) { pa.exclude_kernel = 1; perf = syscall(SYS_perf_event_open, &pa, pid, -1, -1, PERF_FLAG_FD_CLOEXEC); perf_kernel = 0; }
    int perf_errno = perf < 0 ? errno : 0;
    close(go[0]); close(fail[1]);
    if (write(go[1], "x", 1) != 1) {}
    close(go[1]);
    int err = 0;
    if (read(fail[0], &err, sizeof err) == sizeof err) {
        fprintf(stderr, "spawn %s: %s\n", argv[2], strerror(err));
        waitpid(pid, NULL, 0);
        return 127;
    }
    close(fail[0]);
    pthread_t th;
    int sampling = interval_us > 0 && pthread_create(&th, NULL, sampler, NULL) == 0;
    int status = 0;
    struct rusage ru = {0};
    while (wait4(pid, &status, 0, &ru) == -1 && errno == EINTR) {}
    uint64_t end = ns();
    if (sampling) { atomic_store(&stop, 1); pthread_join(th, NULL); }
    sample();
    static int left[MAXP];
    int nleft = tree(left, MAXP, NULL), killed = 0, sigkilled = 0;
    if (nleft) {
        for (int i = 0; i < nleft; i++) if (kill(left[i], SIGTERM) == 0) killed++;
        kill(-pid, SIGTERM);
        uint64_t until = ns() + 500000000ull;
        int n;
        while ((n = tree(left, MAXP, NULL)) && ns() < until) { while (waitpid(-1, NULL, WNOHANG) > 0) {} usleep(5000); }
        if (n) {
            if (have_cg) { char p[4400]; snprintf(p, sizeof p, "%s/cgroup.kill", cg); writefile(p, "1"); }
            for (int i = 0; i < n; i++) if (kill(left[i], SIGKILL) == 0) sigkilled++;
            kill(-pid, SIGKILL);
        }
    }
    uint64_t until = ns() + 2000000000ull;
    for (;;) {
        pid_t r = waitpid(-1, NULL, WNOHANG);
        if (r > 0) continue;
        if (r < 0 || !tree(left, 1, NULL) || ns() > until) break;
        usleep(2000);
    }
    struct rusage ch = {0}, self = {0};
    getrusage(RUSAGE_CHILDREN, &ch);
    getrusage(RUSAGE_SELF, &self);
    long long instructions = -1;
    if (perf >= 0 && read(perf, &instructions, sizeof instructions) != sizeof instructions) instructions = -1;
    unsigned long long sampled = 0;
    for (int i = 0; i < nseen; i++) sampled += seen[i].cpu;
    long long cg_peak = -1, cg_usage = -1, cg_user = -1, cg_sys = -1, cg_pids = -1;
    if (have_cg) {
        cg_peak = cg_value(cg, "memory.peak", NULL);
        cg_usage = cg_value(cg, "cpu.stat", "usage_usec");
        cg_user = cg_value(cg, "cpu.stat", "user_usec");
        cg_sys = cg_value(cg, "cpu.stat", "system_usec");
        cg_pids = cg_value(cg, "pids.peak", NULL);
        for (int i = 0; i < 100 && rmdir(cg) && errno == EBUSY; i++) usleep(10000);
    }
    long tu = ch.ru_utime.tv_sec * 1000000L + ch.ru_utime.tv_usec, ts = ch.ru_stime.tv_sec * 1000000L + ch.ru_stime.tv_usec;
    FILE *f = fopen(out, "w");
    if (f) {
        fprintf(f, "{\"start_ns\":%" PRIu64 ",\"end_ns\":%" PRIu64 ",\"pid\":%d,\"exit\":%d,\"signal\":%d,"
                   "\"user_us\":%ld,\"sys_us\":%ld,\"maxrss_bytes\":%ld,\"minflt\":%ld,\"majflt\":%ld,\"nvcsw\":%ld,\"nivcsw\":%ld",
                start, end, pid, WIFEXITED(status) ? WEXITSTATUS(status) : -1, WIFSIGNALED(status) ? WTERMSIG(status) : 0,
                (long)(ru.ru_utime.tv_sec * 1000000 + ru.ru_utime.tv_usec), (long)(ru.ru_stime.tv_sec * 1000000 + ru.ru_stime.tv_usec),
                ru.ru_maxrss * 1024, ru.ru_minflt, ru.ru_majflt, ru.ru_nvcsw, ru.ru_nivcsw);
        fprintf(f, ",\"tree_cpu_us\":%ld,\"tree_user_us\":%ld,\"tree_sys_us\":%ld,\"tree_children_maxrss_bytes\":%ld", tu + ts, tu, ts, ch.ru_maxrss * 1024);
        json_num(f, "tree_cpu_sampled_us", (long long)(sampled * 1000000ull / tick), samples > 0);
        json_num(f, "tree_max_rss_bytes", (long long)rss_max, samples > 0);
        fprintf(f, ",\"tree_procs_max\":%d,\"helpers_max\":%d,\"helpers_seen\":%d,\"helpers_left\":%d,\"helpers_killed\":%d,\"helpers_sigkilled\":%d,\"samples\":%d,\"interval_us\":%ld,\"runstat_cpu_us\":%ld",
                procs_max, helpers_max, nseen > 0 ? nseen - 1 : 0, nleft, killed, sigkilled, samples, interval_us,
                (long)(self.ru_utime.tv_sec * 1000000 + self.ru_utime.tv_usec + self.ru_stime.tv_sec * 1000000 + self.ru_stime.tv_usec));
        fprintf(f, ",\"lifetime_max_footprint\":null");
        json_num(f, "instructions", instructions, instructions >= 0);
        fprintf(f, ",\"instructions_scope\":%s", instructions < 0 ? "null" : perf_kernel ? "\"user+kernel\"" : "\"user\"");
        if (perf_errno) fprintf(f, ",\"perf_error\":\"%s\"", strerror(perf_errno));
        if (have_cg) fprintf(f, ",\"cgroup\":\"%s\"", cg); else fprintf(f, ",\"cgroup\":null");
        json_num(f, "cgroup_memory_peak_bytes", cg_peak, cg_peak >= 0);
        json_num(f, "cgroup_cpu_us", cg_usage, cg_usage >= 0);
        json_num(f, "cgroup_user_us", cg_user, cg_user >= 0);
        json_num(f, "cgroup_sys_us", cg_sys, cg_sys >= 0);
        json_num(f, "cgroup_pids_peak", cg_pids, cg_pids >= 0);
        fprintf(f, ",\"sources\":{\"user_us\":\"rusage\",\"sys_us\":\"rusage\",\"maxrss_bytes\":\"rusage\",\"tree_cpu_us\":\"rusage-children-subreaper\",\"tree_cpu_sampled_us\":\"proc-sampler\","
                   "\"tree_max_rss_bytes\":\"proc-sampler-statm\",\"helpers\":\"proc-sampler\",\"instructions\":%s,\"cgroup\":%s}}\n",
                instructions >= 0 ? "\"perf\"" : "null", have_cg ? "\"cgroup-v2\"" : "null");
        fclose(f);
    }
    if (fwd && WIFSIGNALED(status) && WTERMSIG(status) == fwd) return 128 + fwd;
    if (WIFSIGNALED(status)) return 128 + WTERMSIG(status);
    return WIFEXITED(status) ? WEXITSTATUS(status) : 1;
}
