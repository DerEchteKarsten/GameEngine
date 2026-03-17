#include "bindless.slang"
#include "datatypes.slang"

struct InstanceCullBindings {
    uint64_t num_instances;
    Buf<BvhNode*> instance_bvh_root_nodes;
    MutBuf<InstanceBvhRoot> bvh_node_stack;
    MutBuf<TraversalVariables> variables;
};

[[vk::push_constant]] InstanceCullBindings b;

[shader("compute")]
[numthreads(64, 1, 1)]
void instance_cull(uint gid: SV_GroupID, uint dtid: SV_DispatchThreadID, uint gtid: SV_GroupThreadID) {
    if (dtid < b.num_instances) {
        var slot = 0;
        InterlockedAdd(b.variables.val.write_offset, 1, slot);
        InterlockedAdd(b.variables.val.node_count, 1);
        b.bvh_node_stack.val[slot] = InstanceBvhRoot(slot, b.instance_bvh_root_nodes.val[slot]);
    }
}
