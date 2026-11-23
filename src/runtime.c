/* The Ruach runtime, included at the top of every generated program. */
#include <stdint.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>
#include <time.h>
#include <sys/mman.h>
#include <ucontext.h>

#define RH_IMMORTAL (INT64_C(1) << 60)
#define RH_LIKELY(x) __builtin_expect(!!(x), 1)
#define RH_UNLIKELY(x) __builtin_expect(!!(x), 0)

typedef struct { int64_t rc; } Hdr;
typedef struct { int64_t rc; int64_t tag; } Cell;
typedef struct { int64_t rc; int64_t len; int64_t cap; char data[]; } Str;
typedef struct Clo { int64_t rc; void *fn; void (*drop)(struct Clo *); } Clo;

static int rh_argc;
static char **rh_argv;
static bool rh_at_eof;
static bool rh_last_io_ok = true;

__attribute__((noreturn, cold)) static void rh_die(const char *msg) {
    fflush(stdout);
    fprintf(stderr, "panic: %s\n", msg);
    exit(101);
}

__attribute__((noreturn, cold)) static void rh_oob(int64_t i, int64_t len) {
    char buf[96];
    snprintf(buf, sizeof buf, "index %lld is out of bounds for length %lld", (long long)i, (long long)len);
    rh_die(buf);
}

static inline void *rh_alloc(size_t n) {
    void *p = malloc(n);
    if (RH_UNLIKELY(!p)) rh_die("out of memory");
    return p;
}

static inline void *rh_realloc(void *p, size_t n) {
    void *q = realloc(p, n);
    if (RH_UNLIKELY(!q)) rh_die("out of memory");
    return q;
}

/* A variant value is a pointer to a Cell, or `2 * tag + 1` for a
   constructor without fields. */
static inline bool rh_is_cell(void *p) { return !((uintptr_t)p & 1); }
static inline int64_t rh_tag(void *p) {
    return rh_is_cell(p) ? ((Cell *)p)->tag : (int64_t)((uintptr_t)p >> 1);
}
static inline void rh_cell_inc(void *p) { if (rh_is_cell(p)) ((Cell *)p)->rc++; }
/* True when the last reference is gone and the cell must be dropped. */
static inline bool rh_cell_release(void *p) { return rh_is_cell(p) && --((Cell *)p)->rc == 0; }

static inline void rh_clo_inc(Clo *c) { c->rc++; }
static inline void rh_clo_dec(Clo *c) { if (--c->rc == 0) c->drop(c); }

/* ------------------------------------------------------------- integers */

static inline int64_t rh_add(int64_t a, int64_t b) { return (int64_t)((uint64_t)a + (uint64_t)b); }
static inline int64_t rh_sub(int64_t a, int64_t b) { return (int64_t)((uint64_t)a - (uint64_t)b); }
static inline int64_t rh_mul(int64_t a, int64_t b) { return (int64_t)((uint64_t)a * (uint64_t)b); }
static inline int64_t rh_neg(int64_t a) { return (int64_t)(0 - (uint64_t)a); }
static inline int64_t rh_div(int64_t a, int64_t b) {
    if (RH_UNLIKELY(b == 0)) rh_die("division by zero");
    if (RH_UNLIKELY(b == -1)) return rh_neg(a);
    return a / b;
}
static inline int64_t rh_rem(int64_t a, int64_t b) {
    if (RH_UNLIKELY(b == 0)) rh_die("remainder by zero");
    if (RH_UNLIKELY(b == -1)) return 0;
    return a % b;
}
static inline int64_t rh_shl(int64_t a, int64_t b) { return (b < 0 || b > 63) ? 0 : (int64_t)((uint64_t)a << b); }
static inline int64_t rh_shr(int64_t a, int64_t b) { return (b < 0 || b > 63) ? (a < 0 ? -1 : 0) : (a >> b); }
static inline int64_t rh_f2i(double f) {
    if (f != f) return 0;
    if (f >= 9.2233720368547758e18) return INT64_MAX;
    if (f <= -9.2233720368547758e18) return INT64_MIN;
    return (int64_t)f;
}

/* -------------------------------------------------------------- strings */

static inline void rh_str_inc(Str *s) { s->rc++; }
static inline void rh_str_dec(Str *s) { if (--s->rc == 0) free(s); }

static Str *rh_str_new(int64_t cap) {
    Str *s = rh_alloc(sizeof(Str) + (size_t)cap + 1);
    s->rc = 1;
    s->len = 0;
    s->cap = cap;
    s->data[0] = 0;
    return s;
}

static Str *rh_str_from(const char *p, int64_t n) {
    Str *s = rh_str_new(n);
    memcpy(s->data, p, (size_t)n);
    s->len = n;
    s->data[n] = 0;
    return s;
}

/* Makes room for `extra` more bytes in an owned string, in place when the
   string is unique. */
static Str *rh_str_reserve(Str *s, int64_t extra) {
    int64_t need = s->len + extra;
    if (s->rc == 1) {
        if (s->cap >= need) return s;
        int64_t cap = s->cap * 2 > need ? s->cap * 2 : need;
        if (cap < 16) cap = 16;
        s = rh_realloc(s, sizeof(Str) + (size_t)cap + 1);
        s->cap = cap;
        return s;
    }
    int64_t cap = need < 16 ? 16 : need;
    Str *t = rh_str_new(cap);
    memcpy(t->data, s->data, (size_t)s->len);
    t->len = s->len;
    s->rc--;
    return t;
}

static void rh_put(Str **s, const char *p, int64_t n) {
    *s = rh_str_reserve(*s, n);
    memcpy((*s)->data + (*s)->len, p, (size_t)n);
    (*s)->len += n;
    (*s)->data[(*s)->len] = 0;
}

static void rh_puts(Str **s, const char *p) { rh_put(s, p, (int64_t)strlen(p)); }

static Str *rh_str_concat(Str *a, Str *b) {
    rh_put(&a, b->data, b->len);
    rh_str_dec(b);
    return a;
}

static inline bool rh_str_eq(Str *a, Str *b) {
    return a == b || (a->len == b->len && memcmp(a->data, b->data, (size_t)a->len) == 0);
}

static inline int rh_str_cmp(Str *a, Str *b) {
    int64_t n = a->len < b->len ? a->len : b->len;
    int c = memcmp(a->data, b->data, (size_t)n);
    if (c) return c < 0 ? -1 : 1;
    return a->len < b->len ? -1 : a->len > b->len ? 1 : 0;
}

static inline int64_t rh_str_byte(Str *s, int64_t i) {
    if (RH_UNLIKELY((uint64_t)i >= (uint64_t)s->len)) rh_oob(i, s->len);
    return (unsigned char)s->data[i];
}

static Str *rh_str_slice(Str *s, int64_t a, int64_t b) {
    if (a < 0) a = 0;
    if (b > s->len) b = s->len;
    if (b < a) b = a;
    return rh_str_from(s->data + a, b - a);
}

static Str *rh_str_from_byte(int64_t b) {
    char c = (char)(b & 0xff);
    return rh_str_from(&c, 1);
}

static Str *rh_str_trim(Str *s) {
    int64_t a = 0, b = s->len;
    while (a < b && (s->data[a] == ' ' || s->data[a] == '\t' || s->data[a] == '\n' || s->data[a] == '\r')) a++;
    while (b > a && (s->data[b - 1] == ' ' || s->data[b - 1] == '\t' || s->data[b - 1] == '\n' || s->data[b - 1] == '\r')) b--;
    return rh_str_from(s->data + a, b - a);
}

static int64_t rh_str_find(Str *s, Str *t, int64_t from) {
    if (from < 0) from = 0;
    if (t->len == 0) return from <= s->len ? from : -1;
    for (int64_t i = from; i + t->len <= s->len; i++) {
        if (s->data[i] == t->data[0] && memcmp(s->data + i, t->data, (size_t)t->len) == 0) return i;
    }
    return -1;
}

static bool rh_str_is_int(Str *s) {
    int64_t i = 0;
    if (s->len == 0) return false;
    if (s->data[0] == '-' || s->data[0] == '+') i = 1;
    if (i == s->len) return false;
    uint64_t v = 0;
    for (; i < s->len; i++) {
        char c = s->data[i];
        if (c < '0' || c > '9') return false;
        if (v > (UINT64_MAX - 9) / 10) return false;
        v = v * 10 + (uint64_t)(c - '0');
    }
    return s->data[0] == '-' ? v <= (uint64_t)INT64_MAX + 1 : v <= (uint64_t)INT64_MAX;
}

static int64_t rh_str_to_int(Str *s) {
    if (!rh_str_is_int(s)) return 0;
    return (int64_t)strtoll(s->data, NULL, 10);
}

static bool rh_str_is_float(Str *s) {
    if (s->len == 0) return false;
    char *end;
    strtod(s->data, &end);
    return end == s->data + s->len && !(s->data[0] == ' ' || s->data[0] == '\t' || s->data[0] == '\n');
}

static double rh_str_to_float(Str *s) {
    return rh_str_is_float(s) ? strtod(s->data, NULL) : 0.0;
}

/* --------------------------------------------------------------- output */

static void rh_show_int(Str **s, int64_t v) {
    char buf[24];
    int n = snprintf(buf, sizeof buf, "%lld", (long long)v);
    rh_put(s, buf, n);
}

static void rh_show_float(Str **s, double v) {
    char buf[40];
    if (v != v) { rh_puts(s, "nan"); return; }
    if (v == INFINITY) { rh_puts(s, "inf"); return; }
    if (v == -INFINITY) { rh_puts(s, "-inf"); return; }
    /* The shortest digits that read back as the same number; without an
       exponent between 1e-5 and 1e16. */
    int n = 0;
    double a = fabs(v);
    int lo = 1;
    if (a >= 1.0 && a < 1e16) lo = (int)floor(log10(a)) + 1;
    for (int prec = lo; prec <= 17; prec++) {
        n = snprintf(buf, sizeof buf, "%.*g", prec, v);
        if (strtod(buf, NULL) == v) break;
    }
    rh_put(s, buf, n);
    if (!strpbrk(buf, ".e")) rh_puts(s, ".0");
}

static Str *rh_float_format(double v, int64_t digits) {
    char buf[400];
    if (digits < 0) digits = 0;
    if (digits > 60) digits = 60;
    int n = snprintf(buf, sizeof buf, "%.*f", (int)digits, v);
    if (n < 0 || n >= (int)sizeof buf) n = snprintf(buf, sizeof buf, "%.*e", (int)digits, v);
    return rh_str_from(buf, n);
}

static void rh_show_str(Str **s, Str *v) {
    rh_put(s, "\"", 1);
    for (int64_t i = 0; i < v->len; i++) {
        unsigned char c = (unsigned char)v->data[i];
        switch (c) {
        case '"': rh_puts(s, "\\\""); break;
        case '\\': rh_puts(s, "\\\\"); break;
        case '\n': rh_puts(s, "\\n"); break;
        case '\t': rh_puts(s, "\\t"); break;
        case '\r': rh_puts(s, "\\r"); break;
        case 0: rh_puts(s, "\\0"); break;
        default: rh_put(s, (const char *)&c, 1);
        }
    }
    rh_put(s, "\"", 1);
}

static void rh_print(Str *s, bool nl, FILE *f) {
    fwrite(s->data, 1, (size_t)s->len, f);
    if (nl) fputc('\n', f);
}

__attribute__((noreturn)) static void rh_panic(Str *msg) {
    fflush(stdout);
    fprintf(stderr, "panic: ");
    fwrite(msg->data, 1, (size_t)msg->len, stderr);
    fputc('\n', stderr);
    exit(101);
}

__attribute__((noreturn)) static void rh_exit(int64_t code) {
    fflush(stdout);
    exit((int)code);
}

static double rh_clock(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (double)ts.tv_sec + (double)ts.tv_nsec * 1e-9;
}

/* ---------------------------------------------------------------- input */

static Str *rh_argv_at(int64_t i) {
    if (i < 0 || i >= rh_argc) rh_oob(i, rh_argc);
    return rh_str_from(rh_argv[i], (int64_t)strlen(rh_argv[i]));
}

static Str *rh_read_line(void) {
    Str *s = rh_str_new(64);
    int c;
    bool any = false;
    while ((c = getchar()) != EOF) {
        any = true;
        if (c == '\n') break;
        char ch = (char)c;
        rh_put(&s, &ch, 1);
    }
    if (!any) rh_at_eof = true;
    if (s->len > 0 && s->data[s->len - 1] == '\r') s->data[--s->len] = 0;
    return s;
}

static Str *rh_read_file(Str *path) {
    FILE *f = fopen(path->data, "rb");
    if (!f) { rh_last_io_ok = false; return rh_str_new(0); }
    Str *s = rh_str_new(4096);
    char buf[65536];
    size_t n;
    while ((n = fread(buf, 1, sizeof buf, f)) > 0) rh_put(&s, buf, (int64_t)n);
    rh_last_io_ok = !ferror(f);
    fclose(f);
    return s;
}

static bool rh_write_file(Str *path, Str *data) {
    FILE *f = fopen(path->data, "wb");
    if (!f) return false;
    bool ok = fwrite(data->data, 1, (size_t)data->len, f) == (size_t)data->len;
    return fclose(f) == 0 && ok;
}
