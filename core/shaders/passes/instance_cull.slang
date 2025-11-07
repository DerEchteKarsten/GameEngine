#include "datatypes.slang"

struct Bindings {
    uint64_t num_instances;
    Aabb *aabbs;
    uint *instance_bvh_root_nodes;
    float4x4 *instance_transforms;
    DispatchParams *dp;
    InstancedOffset *bvh_node_stack;
};


uint push(uint n)
{
    uint oldTail;
    InterlockedAdd(b.dp.node_tail, n, oldTail);
    return oldTail;
}


[[vk::push_constant]] Bindings b;

[shader("compute")]
[numthreads(64, 1, 1)]
void main(uint gid: SV_GroupID, uint dtid: SV_DispatchThreadID, uint gtid: SV_GroupThreadID) {
    if(dtid < b.num_instances) {
        var slot = push(1);
        b.bvh_node_stack[slot] = InstancedOffset(dtid, b.instance_bvh_root_nodes[dtid]);
    }
}
