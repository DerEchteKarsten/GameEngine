#include "datatypes.slang"

struct DrawIndirectCommand {
    uint vertex_count;
    uint instance_count;
    uint first_vertex;
    uint first_instance;
};

struct Bindings {
    Aabb *aabbs;
    uint *instance_bvh_root_nodes;
    float4x4* instance_transforms;
    Cluster *cluster_buffer;
    DrawIndirectCommand* indirect_draw;
};

[[vk::push_constant]] Bindings b;

[shader("compute")]
[numthreads(64, 1, 1)]
void main(uint gid: SV_GroupID, uint dtid: SV_DispatchThreadID, uint gtid: SV_GroupThreadID) {
    var slot = 0;
    InterlockedAdd(b.indirect_draw.instance_count, 1, slot);
    b.cluster_buffer[slot] = Cluster(0, dtid);
}
