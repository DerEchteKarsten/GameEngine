#include "bindless.slang"
#include "datatypes.slang"

struct TraversalVariables {
    uint64_t publish_head;
    uint64_t reserve_head;
    uint64_t tail;
    uint64_t work_count;
}

struct BvhCullBindings {
    Buf<BvhNode *> stack;
    Buf<TraversalVariables> variables;
};

static const uint QUEUE_SIZE = 1024 * 16;

[[vk::push_constant]] BvhCullBindings b;
void pushNode(BvhNode *ptr)
{
    uint64_t idx;

    // 1. Reserve slot
    InterlockedAdd(b.variables.val.reserve_head, 1, idx);

    // 2. Write pointer
    b.stack.val[idx & (QUEUE_SIZE - 1)] = ptr;

    // 3. Ensure write visible
    DeviceMemoryBarrier();

    // 4. Publish in order
    uint64_t expected = idx;
    while (true)
    {
        uint64_t original;
        InterlockedCompareExchange(
            b.variables.val.publish_head,
            expected,
            expected + 1,
            original
        );

        if (original == expected)
            break;
    }

    printf("pushed: %lu, %p", idx, ptr);
}
bool popNode(out BvhNode *ptr)
{
    while (true)
    {
        uint64_t tail;
        InterlockedAdd(b.variables.val.tail, 0, tail);

        uint64_t head;
        InterlockedAdd(b.variables.val.publish_head, 0, head);

        if (tail >= head)
            return false;

        uint64_t original;
        InterlockedCompareExchange(
            b.variables.val.tail,
            tail,
            tail + 1,
            original
        );

        if (original == tail)
        {
            DeviceMemoryBarrier();
            ptr = b.stack.val[tail & (QUEUE_SIZE - 1)];
            printf("popped: %lu, %p", tail, ptr);
            return true;
        }
    }
}

[numthreads(32, 1, 1)]
void bvhcull(uint3 tid: SV_DispatchThreadID)
{
    while (true)
    {
        uint64_t wc;
        InterlockedAdd(b.variables.val.work_count, 0, wc);
        if (wc == 0)
            break;

        BvhNode *node;
        if (popNode(node))
        {
            [unroll]
            for (uint i = 0; i < 8; i++) {
                let child_count = ((node.child_counts >> (i * 8)) & 0xFF);
                if (child_count == 255)
                {
                    InterlockedAdd(b.variables.val.work_count, 1);
                    pushNode(get_ptr(node.aabb_and_offsets[i]));
                } else {
                    printf("Leaf Found in node %p", node, child_count);
                }
            }

            InterlockedAdd(b.variables.val.work_count, -1);
        }
    }
}