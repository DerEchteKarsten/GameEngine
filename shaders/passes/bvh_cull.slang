#include "bindless.slang"
#include "datatypes.slang"

struct TraversalVariables {
    uint head;
    uint tail;
    uint work_count;
}

struct BvhCullBindings {
    Buf<BvhNode *> stack;
    Buf<TraversalVariables> variables;
};

static const uint QUEUE_SIZE = 10000;

[[vk::push_constant]] BvhCullBindings b;

uint pushNode(BvhNode* ptr)
{
    uint idx;
    InterlockedAdd(b.variables.val.tail, 1, idx);
    b.stack.val[idx & (QUEUE_SIZE - 1)] = ptr;
    return idx;
}

bool popNode(out BvhNode* ptr)
{
    uint head;

    while (true)
    {
        head = b.variables.val.head;
        uint tail = b.variables.val.tail;

        if (head >= tail)
            return false;

        uint prev_head;
        InterlockedCompareExchange(b.variables.val.head, head, head + 1, prev_head);
        if (head == prev_head)
        {
            ptr = b.stack.val[head & (QUEUE_SIZE - 1)];
            return true;
        }
    }
}
[numthreads(64, 1, 1)]
void bvhcull(uint3 tid: SV_DispatchThreadID)
{
    while (true)
    {
        BvhNode *node;
        printf("got here", node);

        if (popNode(node))
        {
            printf("%p", node);

            if (node.child_counts > 0)
            {
                InterlockedAdd(b.variables.val.work_count, 1);

                [unroll]
                for (uint i = 0; i < 8; i++)
                    pushNode(get_ptr(node.aabb_and_offsets[i]));
            }

            InterlockedAdd(b.variables.val.work_count, -1);
        }
        else
        {
            if (b.variables.val.work_count == 0)
                break;
        }
    }
}