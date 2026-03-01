#include "bindless.slang"
#include "datatypes.slang"

struct BvhCullBindings {
    Buf<BvhNode> bvh_nodes;
        Buf<float4x4> instance_transforms;
    Buf<CullData> cull_data;
    MutBuf<InstancedOffset> bvh_node_stack;
    MutBuf<InstancedOffset> clusters;
    MutBuf<DispatchParams> dp;
};

[[vk::push_constant]] BvhCullBindings b;

// uint pushNode(uint nodeIndex)
// {
//     uint idx;
//     InterlockedAdd(gQueueCounter[0], 1, idx);
//     gQueue[idx] = nodeIndex;
//     return idx;
// }

// bool popNode(out uint nodeIndex)
// {
//     uint idx;
//     InterlockedAdd(gQueueCounter[0], -1, idx);

//     if (idx == 0)
//         return false;

//     nodeIndex = gQueue[idx - 1];
//     return true;
// }

[numthreads(64, 1, 1)]
void bvhcull(uint3 tid: SV_DispatchThreadID)
{
    // while (true)
    // {
    //     uint nodeIndex;

    //     if (!popNode(nodeIndex))
    //         break;

    //     Node node = gNodes[nodeIndex];

    //     // ---- Process node ----
    //     float result = node.value * 2.0;
    //     gResults[nodeIndex] = result;

    //     // ---- Push children ----
    //     uint mask = node.childMask;
    //     uint base = node.childBase;

    //     [unroll]
    //     for (uint i = 0; i < 8; i++)
    //     {
    //         if (mask & (1 << i))
    //         {
    //             pushNode(base + i);
    //         }
    //     }
    // }
}