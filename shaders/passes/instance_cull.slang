#include "bindless.slang"
#include "datatypes.slang"

struct InstanceCullBindings {
    uint64_t num_instances;
    Buf<AabbError> aabbs;
    Buf<uint> instance_bvh_root_nodes;
    Buf<float4x4> instance_transforms;
    MutBuf<DispatchParams> dp;
    MutBuf<InstancedOffset> bvh_node_stack;
};


uint push(uint n)
{
    uint oldTail;
    InterlockedAdd(b.dp.val.node_tail, n, oldTail);
    return oldTail;
}


[[vk::push_constant]] InstanceCullBindings b;

[shader("compute")]
[numthreads(64, 1, 1)]
void instance_cull(uint gid: SV_GroupID, uint dtid: SV_DispatchThreadID, uint gtid: SV_GroupThreadID) {
    if(dtid < b.num_instances) {
        var slot = push(1);
        b.bvh_node_stack.val[slot] = InstancedOffset(dtid, b.instance_bvh_root_nodes.val[dtid]);
    }
}
