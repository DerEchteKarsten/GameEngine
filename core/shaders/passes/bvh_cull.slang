#include "datatypes.slang"

struct Bindings {
    BvhNode *bvh_nodes;
    float4x4 *instance_transforms;
    int *bvh_node_stack;
    CullData *cull_data;
    DrawIndirectCommand *indirect;
};

[[vk::push_constant]] Bindings b;

[shader("compute")]
[numthreads(64, 1, 1)]
void main(uint gid: SV_GroupID, uint dtid: SV_DispatchThreadID, uint gtid: SV_GroupThreadID) {
    var node_stack_index = 0;
    do {
        InterlockedAdd(b.bvh_node_stack[0], -1, node_stack_index);
        let node_index = b.bvh_node_stack[node_stack_index+1];
        let node = b.bvh_nodes[node_index];
        if(node.child_counts == 255) {

        }
    } while (node_stack_index > 0);
}