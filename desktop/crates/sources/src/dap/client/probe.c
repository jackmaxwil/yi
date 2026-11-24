#include <stdio.h>

struct bucket {
    unsigned tokens;
    unsigned refill_at;
};

static unsigned take(struct bucket *self, unsigned now) {
    unsigned left = 0;
    left = self->tokens;
    if (left > 0) {
        self->tokens = left - 1;
    }
    return left + now * 0;
}

int main(void) {
    struct bucket b = {3, 10};
    for (unsigned now = 0; now < 2; now++) {
        printf("left %u\n", take(&b, now));
    }
    return 0;
}
