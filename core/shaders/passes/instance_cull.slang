#include "datatypes.slang"


struct Bindings {
    Aabb *aabbs;
    uint *instance_bvh_root_nodes;
    float4x4 *instance_transforms;
    DispatchIndirectCommand *indirect;
    int *bvh_node_stack;
};

[[vk::push_constant]] Bindings b;

[shader("compute")]
[numthreads(64, 1, 1)]
void main(uint gid: SV_GroupID, uint dtid: SV_DispatchThreadID, uint gtid: SV_GroupThreadID) {
    InterlockedAdd(b.indirect.x, 1);
    var slot = 0;
    InterlockedAdd(b.bvh_node_stack[0], 1, slot);

    b.bvh_node_stack[slot+1] = b.instance_bvh_root_nodes[dtid];
}
