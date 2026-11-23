#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
typedef struct Node { struct Node *l, *r; } Node;
static Node *make(int d) {
    Node *n = malloc(sizeof *n);
    if (d == 0) { n->l = n->r = NULL; } else { n->l = make(d - 1); n->r = make(d - 1); }
    return n;
}
static int64_t check(Node *t) { return t->l ? 1 + check(t->l) + check(t->r) : 1; }
static void drop(Node *t) { if (t->l) { drop(t->l); drop(t->r); } free(t); }
int main(void) {
    int max = 20;
    Node *s = make(max + 1);
    printf("stretch tree of depth %d check: %lld\n", max + 1, (long long)check(s));
    drop(s);
    Node *lng = make(max);
    for (int d = 4; d <= max; d += 2) {
        int64_t n = 1 << (max - d + 4), acc = 0;
        for (int64_t k = 0; k < n; k++) { Node *t = make(d); acc += check(t); drop(t); }
        printf("%lld trees of depth %d check: %lld\n", (long long)n, d, (long long)acc);
    }
    printf("long lived tree of depth %d check: %lld\n", max, (long long)check(lng));
    drop(lng);
    return 0;
}
