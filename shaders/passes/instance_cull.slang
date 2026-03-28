#include "bindless.slang"
#include "datatypes.slang"

struct InstanceCullBindings {
    uint64_t num_instances;
    Buf<BvhNode *> instance_bvh_root_nodes;
    Buf<AabbError> instance_aabbs;
    Buf<float4x4> instance_transforms;
    MutBuf<InstanceBvhRoot> bvh_node_stack;
    MutBuf<TraversalVariables> variables;
    float4x4 clip_from_world;
};

[[vk::push_constant]] InstanceCullBindings b;

float4 normalize_plane(float4 p) {
    return p / length(p.xyz);
}
bool aabb_in_frustum(float3 center, float3 half_extent, float4x4 world_from_local) {
    let clip_from_local = mul(transpose(world_from_local), b.clip_from_world);
    let rowmajor = transpose(clip_from_local);
    float4 planes[] = {
        rowmajor[3] + rowmajor[0],
        rowmajor[3] - rowmajor[0],
        rowmajor[3] + rowmajor[1],
        rowmajor[3] - rowmajor[1],
        rowmajor[2],
    };

    for (var i = 0; i < 5; i++) {
        let plane = normalize_plane(planes[i]);
        let flipped = half_extent * sign(plane.xyz);
        if (dot(center + flipped, plane.xyz) <= -plane.w) {
            return false;
        }
    }
    return true;
}

[shader("compute")]
[numthreads(64, 1, 1)]
void instance_cull(uint gid: SV_GroupID, uint dtid: SV_DispatchThreadID, uint gtid: SV_GroupThreadID) {
    if (dtid < b.num_instances) {
        let transform = b.instance_transforms.val[dtid];
        let aabb = b.instance_aabbs.val[dtid];
        if (aabb_in_frustum(aabb.center_and_error.xyz, aabb.half_extent.xyz, transform)) {
            var slot = 0;
            InterlockedAdd(b.variables.val.node_write_offset, 1, slot);
            InterlockedAdd(b.variables.val.node_count, 1);
            b.bvh_node_stack.val[slot] = InstanceBvhRoot(dtid, b.instance_bvh_root_nodes.val[dtid]);    
        }
    }
}
    