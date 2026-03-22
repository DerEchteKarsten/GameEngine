#include "bindless.slang"
#include "datatypes.slang"

struct InstanceHeader {
    uint64_t meshlet_offset;
    uint64_t cull_data_offset;
}

struct BvhCullBindings {
    MutBuf<InstanceBvhRoot> queue;
    MutBuf<TraversalVariables> queue_state;
    MutBuf<InstancedMeshlet> meshlets;
    Buf<float4x4> instance_transforms;
    Buf<InstanceHeader> instance_headers;
    float4 camera_pos;
    float4x4 proj;
    float4x4 clip_from_world;
    float window_height;
};

#define GROUP_SIZE 64
#define MAX_NODE_CHILDREN_BITS_NUM 3
#define MAX_NODE_CHILDREN_NUM (1 << MAX_NODE_CHILDREN_BITS_NUM)
#define MAX_NODES_PER_GROUP (GROUP_SIZE >> MAX_NODE_CHILDREN_BITS_NUM)
#define MAX_NODES 2 * 1024 * 1024 // 2Mib
#define INVALID_INSTANCE_ID (~uint64_t(0))
#define INVALID_PTR (BvhNode*)(~uint64_t(0))

groupshared uint GroupNumCandidateNodes;
groupshared uint GroupCandidateNodesOffset;

groupshared uint GroupNodeBatchStartIndex;

groupshared uint GroupNodeCount;
groupshared uint GroupNodeMask;
groupshared InstanceBvhRoot GroupNodeData[MAX_NODES_PER_GROUP];

groupshared uint GroupMeshletBatchStartIndex;
groupshared uint GroupMeshletBatchReadySize;

groupshared uint GroupLeafCount;
groupshared uint GroupLeafPrefix[GROUP_SIZE + 1];
groupshared uint2 GroupLeafKey[GROUP_SIZE];
groupshared uint GroupLeafMeshletCount[GROUP_SIZE];
groupshared uint GroupCandidateMeshletBase;
groupshared uint GroupCandidateMeshletCount;

/**
 * High-performance scalar wavefront atomic adder (fixed-size version)
 * @param Dest           Destination variable (usually a global counter)
 * @param bCondition     Whether the current lane meets the add condition (e.g. bVisible)
 * @param ValuePerThread Fixed increment per lane (usually 1)
 * @param OutOffset      [out] Unique base offset assigned to each lane that meets the condition
 */
#define WaveInterlockedAddScalar(Dest, bCondition, ValuePerThread, OutOffset)                \
    {                                                                                        \
        /* Count how many lanes in the wave satisfy the condition (bCondition == true) */    \
        /* Hardware reads the active mask and counts set bits; very fast */                  \
        uint _WaveCount = WaveActiveCountBits(bCondition);                                   \
                                                                                             \
        uint _WaveBaseOffset = 0;                                                            \
                                                                                             \
        /* First lane performs one atomic on behalf of the wave */                           \
        if (WaveIsFirstLane())                                                               \
        {                                                                                    \
            if (_WaveCount > 0)                                                              \
            {                                                                                \
                InterlockedAdd(Dest, _WaveCount * (ValuePerThread), _WaveBaseOffset);        \
            }                                                                                \
        }                                                                                    \
                                                                                             \
        /* Broadcast the global base offset to all lanes in the wave */                      \
        _WaveBaseOffset = WaveReadLaneFirst(_WaveBaseOffset);                                \
                                                                                             \
        /* Compute this lane's rank among lanes that satisfy the condition (0, 1, 2...) */   \
        /* WavePrefixCountBits(bCondition) is equivalent to a prefix sum but uses bit ops */ \
        uint _LaneOffset = WavePrefixCountBits(bCondition);                                  \
                                                                                             \
        /* Final offset = global base offset + (rank * fixed increment) */                   \
        OutOffset = _WaveBaseOffset + (_LaneOffset * (ValuePerThread));                      \
    }

[[vk::push_constant]] BvhCullBindings b;

[shader("compute")]
[numthreads(GROUP_SIZE, 1, 1)]
void bvh_cull(uint group_index: SV_GroupIndex) {
    var node_batch_local_offset = MAX_NODES_PER_GROUP;
    var node_batch_base_index = 0;
    while(true) {

        GroupMemoryBarrierWithGroupSync();
        if (group_index == 0)
        {
            GroupNumCandidateNodes = 0;
            GroupNodeMask = 0;
        }
        GroupMemoryBarrierWithGroupSync();

        var ready_node_mask = 0;
        if (node_batch_local_offset == MAX_NODES_PER_GROUP)
        {
            // Claim the next node-batch range from the global queue.
            if (group_index == 0)
            {
                InterlockedAdd(b.queue_state.val.read_offset, MAX_NODES_PER_GROUP, GroupNodeBatchStartIndex);
            }

            GroupMemoryBarrierWithGroupSync();

            node_batch_local_offset = 0;
            node_batch_base_index = GroupNodeBatchStartIndex;
            if (node_batch_base_index >= MAX_NODES)
            {
                // continue;
                break;
            }
        }

        let node_queue_index = node_batch_base_index + node_batch_local_offset + group_index;
        var lane_has_node = (node_batch_local_offset + group_index < MAX_NODES_PER_GROUP);
        if (lane_has_node)
        {
            var node_data = b.queue.val[node_queue_index];
            lane_has_node = node_data.node != INVALID_PTR && node_data.instance != INVALID_INSTANCE_ID;
            if (lane_has_node)
            {
                GroupNodeData[group_index] = node_data;
            }
        }

        if (lane_has_node)
        {
            InterlockedOr(GroupNodeMask, 1u << group_index);
        }
        AllMemoryBarrierWithGroupSync();

        ready_node_mask = GroupNodeMask;

        // If the first slot is valid, process the contiguous ready prefix.
        if ((ready_node_mask & 1u) > 0)
        {
            let batch_size = firstbitlow(~ready_node_mask);
            ProcessNodeBatch(batch_size, group_index);
            if (group_index < batch_size)
            {
                b.queue.val[node_queue_index] = InstanceBvhRoot(INVALID_INSTANCE_ID, INVALID_PTR);
            }

            node_batch_local_offset += batch_size;
            continue;
        }

        if (group_index == 0) {
            GroupNodeCount = b.queue_state.val.node_count;
        }
        GroupMemoryBarrierWithGroupSync();
        if (GroupNodeCount == 0) {
            break;
        }
    }
}

bool lod_error_is_imperceptible(float4 lod_sphere, float simplification_error, uint instance_id) {
    let world_from_local = b.instance_transforms.val[instance_id];
    let world_scale = max(length(world_from_local[0]), max(length(world_from_local[1]), length(world_from_local[2])));
    let camera_pos = b.camera_pos;

    var near = b.proj[2][3];    
    let world_sphere_center = (mul(world_from_local, float4(lod_sphere.xyz, 1.0))).xyz;
    let world_sphere_radius = lod_sphere.w * world_scale;
    let d_pos = world_sphere_center - camera_pos.xyz;
    let d = max(sqrt(dot(d_pos, d_pos)) - world_sphere_radius, near);
    let norm_error = simplification_error / d * b.proj[1][1] * 0.5;
    return norm_error * b.window_height < 10.0;
}

float4 normalize_plane(float4 p){
    return p / length(p.xyz);
}
bool aabb_in_frustum(float3 center, float3 half_extent, uint32_t instance_id) {
    let world_from_local = b.instance_transforms.val[instance_id];
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
        if (dot (center + flipped, plane.xyz) <= -plane.w) {
                return false;
            }
    }
    return true;
}

void ProcessNodeBatch(uint batch_size, uint group_index)
{
    // Each thread maps to (node slot in batch, child slot within that node).
    let node_slot_in_batch = (group_index >> MAX_NODE_CHILDREN_BITS_NUM);
    let child_slot_in_node = group_index & (MAX_NODE_CHILDREN_NUM - 1u);
    let safe_node_slot = min(node_slot_in_batch, batch_size - 1u);

    let job = GroupNodeData[safe_node_slot];

    let child_node = get_ptr(job.node.aabb_and_offsets[child_slot_in_node]);
    var node_write_offset_in_group = 0;

    let child_count = (job.node.child_counts >> (child_slot_in_node * 8)) & 0xff;

    var visible = node_slot_in_batch < batch_size && child_count != 0;
    let is_leave = child_count != 255;

    let world_matrix = b.instance_transforms.val[job.instance];
    visible = visible && aabb_in_frustum(job.node.aabb_and_offsets[child_slot_in_node].center_and_offset_high.xyz, job.node.aabb_and_offsets[child_slot_in_node].half_extent_and_offset_low.xyz, job.instance);
    visible = visible && !lod_error_is_imperceptible(job.node.lod_bounds[child_slot_in_node], job.node.errors[child_slot_in_node], job.instance);

    let push_node = visible && !is_leave;
    if (push_node)
    {
        // Reserve one slot per visible internal child node.
        WaveInterlockedAddScalar(GroupNumCandidateNodes, push_node, 1u, node_write_offset_in_group);
    }

    if (is_leave && visible) {
        let header = b.instance_headers.val[job.instance];
        for (int i = 0; i < child_count; i++) {
            let cull_data = (CullData *)((((uint64_t)child_node) + i) * sizeof(CullData) + header.cull_data_offset);
            if (aabb_in_frustum(cull_data.aabb.center_and_error.xyz, cull_data.aabb.half_extent.xyz, job.instance) &&
                lod_error_is_imperceptible(cull_data.lod_group_sphere, cull_data.aabb.center_and_error.w, job.instance)) {
                var slot = 0;
                InterlockedAdd(b.queue_state.val.meshlet_count, 1, slot);
                b.meshlets.val[slot] = InstancedMeshlet(job.instance, (Meshlet *)((((uint64_t)child_node) + i) * sizeof(Meshlet) + header.meshlet_offset));
            }
        }
    }


    GroupMemoryBarrierWithGroupSync();

    // Group leader reserves a contiguous range in the global node queue.
    if (group_index == 0)
    {
        InterlockedAdd(b.queue_state.val.write_offset, GroupNumCandidateNodes, GroupCandidateNodesOffset);
        InterlockedAdd(b.queue_state.val.node_count, GroupNumCandidateNodes);
    }
    AllMemoryBarrierWithGroupSync();

    // Write new tasks to the queue
    if (push_node)
    {
        uint globalIdx = (GroupCandidateNodesOffset + node_write_offset_in_group);
        b.queue.val[globalIdx] = InstanceBvhRoot(job.instance, child_node);
    }

    DeviceMemoryBarrierWithGroupSync();

    // AllMemoryBarrierWithGroupSync();
    
    // if (group_index == 0)
    // {
    //     GroupLeafCount = 0;
    // }
    // GroupMemoryBarrierWithGroupSync();

    // // Collect visible leaf nodes for group-wide parallel candidate emission.
    // if (visible && child_count == 255)
    // {
    //     GroupLeafKey[leafOffsetInGroup] = uint2(instanceIndex, groupID);
    //     GroupLeafMeshletCount[leafOffsetInGroup] = childNode.GetMeshletCount();
    // }

    // GroupMemoryBarrierWithGroupSync();

    // Build leaf prefix and reserve candidate range once per group.
    // if (groupIndex == 0)
    // {
    //     GroupLeafPrefix[0] = 0;
    //     [loop]
    //     for (uint leafIndex = 0; leafIndex < GroupLeafCount; ++leafIndex)
    //     {
    //         GroupLeafPrefix[leafIndex + 1] = GroupLeafPrefix[leafIndex] + GroupLeafMeshletCount[leafIndex];
    //     }

    //     const uint totalMeshletsInGroup = GroupLeafPrefix[GroupLeafCount];

    //     uint meshletStart = 0;
    //     InterlockedAdd(taskStateUAV[0].PassState[passIndex].TotalMeshlets, totalMeshletsInGroup, meshletStart);

    //     uint candidateWriteOffset = 0;
    //     InterlockedAdd(taskStateUAV[0].PassState[passIndex].CandidateMeshletWriteOffset, totalMeshletsInGroup, candidateWriteOffset);

    //     const uint candidateEnd = min(candidateWriteOffset + totalMeshletsInGroup, MAX_CANDIDATE_MESHLETS);
    //     GroupCandidateMeshletBase = candidateWriteOffset;
    //     GroupCandidateMeshletCount = (uint)max((int)candidateEnd - (int)candidateWriteOffset, 0);
    // }

    // GroupMemoryBarrierWithGroupSync();

    // globallycoherent RWByteAddressBuffer candidateMeshletBufferUAV = GetCandidateMeshletBufferUAV();
    // for (uint flatMeshletIndex = groupIndex; flatMeshletIndex < GroupCandidateMeshletCount; flatMeshletIndex += DAG_CULL_GROUP_SIZE)
    // {
    //     const uint leafIndex = FindLeafIndexForFlatMeshlet(flatMeshletIndex, GroupLeafCount);
    //     const uint localMeshletIndex = flatMeshletIndex - GroupLeafPrefix[leafIndex];
    //     const uint2 leafKey = GroupLeafKey[leafIndex];

    //     VisibleMeshletPayload meshletPayload;
    //     meshletPayload.InstanceIndex = leafKey.x;
    //     meshletPayload.PackedData = meshletPayload.PackData(leafKey.y, localMeshletIndex);

    //     const uint outIndex = GroupCandidateMeshletBase + flatMeshletIndex;
    //     candidateMeshletBufferUAV.Store2(outIndex * MESHLET_BYTE_STRIDE, uint2(meshletPayload.InstanceIndex, meshletPayload.PackedData));
    // }

    // DeviceMemoryBarrierWithGroupSync();

    // if (groupIndex == 0)
    // {
    //     const uint startIndex = GroupCandidateMeshletBase;
    //     const uint endIndex = GroupCandidateMeshletBase + GroupCandidateMeshletCount;
    //     globallycoherent RWByteAddressBuffer meshletBatchBufferUAV = GetMeshletBatchBufferUAV();

    //     for (uint index = startIndex; index < endIndex;)
    //     {
    //         const uint batchIndex = index / DAG_CULL_GROUP_SIZE;
    //         const uint nextIndex = (index & ~(DAG_CULL_GROUP_SIZE - 1u)) + DAG_CULL_GROUP_SIZE;
    //         const uint maxIndex = min(nextIndex, endIndex);
    //         const uint num = maxIndex - index;
    //         uint temp;
    //         meshletBatchBufferUAV.InterlockedAdd(batchIndex * 4, num, temp);
    //         index = nextIndex;
    //     }
    // }

    // DeviceMemoryBarrierWithGroupSync();

    if (group_index == 0)
    {
        InterlockedAdd(b.queue_state.val.node_count, -batch_size);
    }
}