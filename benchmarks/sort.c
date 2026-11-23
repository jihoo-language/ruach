#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
static int cmp(const void *a, const void *b) {
    int64_t x = *(const int64_t *)a, y = *(const int64_t *)b;
    return (x > y) - (x < y);
}
int main(void) {
    int64_t n = 5000000, seed = 42;
    int64_t *a = malloc(n * sizeof *a);
    for (int64_t i = 0; i < n; i++) { seed = (seed * 1103515245 + 12345) % 2147483648; a[i] = seed; }
    qsort(a, n, sizeof *a, cmp);
    int ok = 1;
    for (int64_t i = 0; i + 1 < n; i++) if (a[i] > a[i + 1]) ok = 0;
    printf("%s\n%lld\n%lld\n", ok ? "true" : "false", (long long)a[0], (long long)a[n - 1]);
    return 0;
}
