#include <stdio.h>
#include <stdlib.h>
#include <stdbool.h>
#include <stdint.h>
int main(void) {
    int64_t total = 0;
    for (int round = 0; round < 5; round++) {
        int64_t n = 50000000;
        bool *a = malloc(n);
        for (int64_t i = 0; i < n; i++) a[i] = true;
        a[0] = a[1] = false;
        for (int64_t i = 2; i * i < n; i++)
            if (a[i]) for (int64_t j = i * i; j < n; j += i) a[j] = false;
        int64_t c = 0;
        for (int64_t i = 0; i < n; i++) c += a[i];
        total += c;
        free(a);
    }
    printf("%lld\n", (long long)total);
    return 0;
}
