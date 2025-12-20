#include "bindless.slang"

struct TestBindings {
    uint num;
    uint num2;
    Buf<uint> test;
}

[[vk::push_constant]] TestBindings b;

[shader("compute")]
[numthreads(8, 8, 1)]
void compute_test() {

    printf("%u, %u, %u", b.num, b.num2, *b.test.val);
}