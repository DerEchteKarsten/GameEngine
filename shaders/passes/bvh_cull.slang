#include "bindless.slang"
#include "datatypes.slang"

struct BvhCullBindings {
    MutBuf<InstanceBvhRoot> queue;
    MutBuf<TraversalVariables> queue_state;
    MutBuf<InstancedMeshlet> visible_meshlets;
    MutBuf<InstanceMeshletIndex> canidate_meshlets;
    MutBuf<uint> meshlet_batch_buffer;
    Buf<float4x4> instance_transforms;
    Buf<InstanceHeader> instance_headers;
    float4 camera_pos;
    float4x4 proj;
    float4x4 clip_from_world;
    float window_height;
};
[[vk::push_constant]] BvhCullBindings b;
#define INVALID_U32 (~uint(0))
#define INVALID_U64 (~uint64_t(0))
#define INVALID_PTR ((BvhNode*)INVALID_U64)

#define DAG_CULL_GROUP_SIZE 64
#define MAX_BVH_NODE_CHILDREN_BITS_NUM 3
#define MAX_BVH_NODE_CHILDREN_NUM (1 << MAX_BVH_NODE_CHILDREN_BITS_NUM)
#define MAX_BVH_NODES_PER_GROUP (DAG_CULL_GROUP_SIZE >> MAX_BVH_NODE_CHILDREN_BITS_NUM)

#define MAX_NODES 2 * 1024 * 1024

#define MAX_CANDIDATE_MESHLETS 16 * 1024 * 1024
#define MAX_CANDIDATE_MESHLETS_BATCH (MAX_CANDIDATE_MESHLETS / DAG_CULL_GROUP_SIZE)

#define MAX_VISIBLE_MESHLETS 4 * 1024 * 1024

groupshared uint GroupNumCandidateNodes;
groupshared uint GroupCandidateNodesOffset;

groupshared uint GroupNodeBatchStartIndex;

groupshared uint GroupNodeCount;
groupshared uint GroupNodeMask;
groupshared InstanceBvhRoot GroupNodeData[MAX_BVH_NODES_PER_GROUP];

groupshared uint GroupMeshletBatchStartIndex;
groupshared uint GroupMeshletBatchReadySize;

groupshared uint GroupLeafCount;
groupshared uint GroupLeafPrefix[DAG_CULL_GROUP_SIZE + 1];
groupshared InstanceMeshletIndex GroupLeafKey[DAG_CULL_GROUP_SIZE];
groupshared uint GroupLeafMeshletCount[DAG_CULL_GROUP_SIZE];
groupshared uint GroupCandidateMeshletBase;
groupshared uint GroupCandidateMeshletCount;

bool lod_error_is_imperceptible(float4 lod_sphere, float simplification_error, float4x4 world_from_local) {
    let world_from_local_t = transpose(world_from_local);
    let world_scale = max(length(world_from_local_t[0]), max(length(world_from_local_t[1]), length(world_from_local_t[2])));
    let camera_pos = b.camera_pos;

    var near = b.proj[2][3];
    let world_sphere_center = (mul(world_from_local, float4(lod_sphere.xyz, 1.0))).xyz;
    let world_sphere_radius = lod_sphere.w * world_scale;
    let d_pos = world_sphere_center - camera_pos.xyz;
    let d = max(sqrt(dot(d_pos, d_pos)) - world_sphere_radius, near);
    let norm_error = (simplification_error * world_scale) / d * b.proj[1][1] * 0.5;
    return norm_error * b.window_height < 1.0;
}

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

uint FindLeafIndexForFlatMeshlet(uint flatIndex, uint leafCount)
{
    uint lo = 0u;
    uint hi = leafCount;
    while (lo + 1u < hi)
    {
        const uint mid = (lo + hi) >> 1u;
        if (GroupLeafPrefix[mid] <= flatIndex)
            lo = mid;
        else
            hi = mid;
    }
    return lo;
}

void ProcessNodeBatch(uint batchSize, uint groupIndex)
{
    // Each thread maps to (node slot in batch, child slot within that node).
    let node_slot_in_batch = (groupIndex >> MAX_BVH_NODE_CHILDREN_BITS_NUM);
    let child_slot_in_node = groupIndex & (MAX_BVH_NODE_CHILDREN_NUM - 1u);
    let safe_node_slot = min(node_slot_in_batch, batchSize - 1u);

    let job = GroupNodeData[safe_node_slot];

    let child_node = get_ptr(job.node.aabb_and_offsets[child_slot_in_node]);
    var node_write_offset_in_group = 0;

    let child_count = (job.node.child_counts >> (child_slot_in_node * 8)) & 0xff;

    var visible = node_slot_in_batch < batchSize && child_count != 0;
    let is_leave = child_count != 255;

    let world_matrix = b.instance_transforms.val[job.instance];
    visible = visible && aabb_in_frustum(job.node.aabb_and_offsets[child_slot_in_node].center_and_offset_high.xyz, job.node.aabb_and_offsets[child_slot_in_node].half_extent_and_offset_low.xyz, world_matrix);
    visible = visible && !lod_error_is_imperceptible(job.node.lod_bounds[child_slot_in_node], job.node.errors[child_slot_in_node], world_matrix);

    let push_node = visible && !is_leave;
    if (push_node)
    {
        // Reserve one slot per visible internal child node.
        WaveInterlockedAddScalar(GroupNumCandidateNodes, push_node, 1u, node_write_offset_in_group);
    }

    GroupMemoryBarrierWithGroupSync();

    // Group leader reserves a contiguous range in the global node queue.
    if (groupIndex == 0)
    {
        InterlockedAdd(b.queue_state.val.node_write_offset, GroupNumCandidateNodes, GroupCandidateNodesOffset);
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

    GroupMemoryBarrierWithGroupSync();
    if (groupIndex == 0)
    {
        GroupLeafCount = 0;
    }
    GroupMemoryBarrierWithGroupSync();

    // Collect visible leaf nodes for group-wide parallel candidate emission.
    if (visible && is_leave)
    {
        uint leafOffsetInGroup = 0;
        WaveInterlockedAddScalar(GroupLeafCount, visible && is_leave, 1u, leafOffsetInGroup);
        GroupLeafKey[leafOffsetInGroup] = InstanceMeshletIndex((uint)job.instance, uint((uint64_t)child_node));
        GroupLeafMeshletCount[leafOffsetInGroup] = uint(child_count);
    }

    GroupMemoryBarrierWithGroupSync();

    // Build leaf prefix and reserve candidate range once per group.
    if (groupIndex == 0)
    {
        GroupLeafPrefix[0] = 0;
        [loop]
        for (uint leafIndex = 0; leafIndex < GroupLeafCount; ++leafIndex)
        {
            GroupLeafPrefix[leafIndex + 1] = GroupLeafPrefix[leafIndex] + GroupLeafMeshletCount[leafIndex];
        }

        const uint totalMeshletsInGroup = GroupLeafPrefix[GroupLeafCount];

        uint meshletStart = 0;
        InterlockedAdd(b.queue_state.val.total_meshlets, totalMeshletsInGroup, meshletStart);

        uint candidateWriteOffset = 0;
        InterlockedAdd(b.queue_state.val.candidate_meshlet_write_offset, totalMeshletsInGroup, candidateWriteOffset);

        const uint candidateEnd = min(candidateWriteOffset + totalMeshletsInGroup, MAX_CANDIDATE_MESHLETS);
        GroupCandidateMeshletBase = candidateWriteOffset;
        GroupCandidateMeshletCount = (uint)max((int)candidateEnd - (int)candidateWriteOffset, 0);
    }

    GroupMemoryBarrierWithGroupSync();

    for (uint flatMeshletIndex = groupIndex; flatMeshletIndex < GroupCandidateMeshletCount; flatMeshletIndex += DAG_CULL_GROUP_SIZE)
    {
        let leafIndex = FindLeafIndexForFlatMeshlet(flatMeshletIndex, GroupLeafCount);
        let localMeshletIndex = flatMeshletIndex - GroupLeafPrefix[leafIndex];
        let leafKey = GroupLeafKey[leafIndex];

        const uint outIndex = GroupCandidateMeshletBase + flatMeshletIndex;
        b.canidate_meshlets.val[outIndex] = InstanceMeshletIndex(leafKey.instance, leafKey.meshlet + localMeshletIndex);
    }

    DeviceMemoryBarrierWithGroupSync();

    if (groupIndex == 0)
    {
        const uint startIndex = GroupCandidateMeshletBase;
        const uint endIndex = GroupCandidateMeshletBase + GroupCandidateMeshletCount;

        for (uint index = startIndex; index < endIndex;)
        {
            const uint batchIndex = index / DAG_CULL_GROUP_SIZE;
            const uint nextIndex = (index & ~(DAG_CULL_GROUP_SIZE - 1u)) + DAG_CULL_GROUP_SIZE;
            const uint maxIndex = min(nextIndex, endIndex);
            const uint num = maxIndex - index;
            uint temp;
            InterlockedAdd(b.meshlet_batch_buffer.val[batchIndex], num, temp);
            index = nextIndex;
        }
    }

    DeviceMemoryBarrierWithGroupSync();

    if (groupIndex == 0)
    {
        InterlockedAdd(b.queue_state.val.node_count, -batchSize);
    }
}

void ProcessMeshletBatch(uint meshletBatchStartIndex, uint meshletBatchReadySize, uint groupIndex)
{
    if (groupIndex < meshletBatchReadySize)
    {
        let candidate_index = meshletBatchStartIndex * DAG_CULL_GROUP_SIZE + groupIndex;
        let job = b.canidate_meshlets.val[candidate_index];
        let header = b.instance_headers.val[job.instance];
        let trans = b.instance_transforms.val[job.instance];

        let cull_data = (CullData *)(header.cull_data_offset + uint64_t(job.meshlet * sizeof(CullData)));
        if (aabb_in_frustum(cull_data.aabb.center_and_error.xyz, cull_data.aabb.half_extent.xyz, trans) &&
            lod_error_is_imperceptible(cull_data.lod_group_sphere, cull_data.aabb.center_and_error.w, trans)) {
            uint write_offset;
            InterlockedAdd(b.queue_state.val.visible_meshlet_count, 1, write_offset);
            b.visible_meshlets.val[write_offset] = InstancedMeshlet((uint64_t)job.instance, (Meshlet *)(header.meshlet_offset + uint64_t(job.meshlet * sizeof(Meshlet))));
        }

    }
}

[shader("compute")]
[numthreads(DAG_CULL_GROUP_SIZE, 1, 1)]
void bvh_cull(uint3 DTid: SV_DispatchThreadID, uint groupIndex: SV_GroupIndex)
{
    if (b.queue_state.val.node_write_offset == 0) {
        return;
    }

    bool processNodeQueue = true;
    uint nodeBatchLocalOffset = MAX_BVH_NODES_PER_GROUP;
    uint nodeBatchBaseIndex = 0;
    uint meshletBatchStartIndex = INVALID_U32;

    while (true)
    {
        // Reset per-iteration group-shared counters.
        GroupMemoryBarrierWithGroupSync();
        if (groupIndex == 0)
        {
            GroupNumCandidateNodes = 0;
            GroupNodeMask = 0;
        }
        GroupMemoryBarrierWithGroupSync();

        uint readyNodeMask = 0;
        if (processNodeQueue)
        {
            if (nodeBatchLocalOffset == MAX_BVH_NODES_PER_GROUP)
            {
                // Claim the next node-batch range from the global queue.
                if (groupIndex == 0)
                {
                    InterlockedAdd(b.queue_state.val.node_batch_read_offset, MAX_BVH_NODES_PER_GROUP, GroupNodeBatchStartIndex);
                }

                GroupMemoryBarrierWithGroupSync();

                nodeBatchLocalOffset = 0;
                nodeBatchBaseIndex = GroupNodeBatchStartIndex;
                if (nodeBatchBaseIndex >= MAX_NODES)
                {
                    processNodeQueue = false;
                    continue;
                }
            }

            const uint nodeQueueIndex = nodeBatchBaseIndex + nodeBatchLocalOffset + groupIndex;
            bool laneHasNode = ((nodeBatchLocalOffset + groupIndex) < MAX_BVH_NODES_PER_GROUP);
            if (laneHasNode)
            {
                let nodeData = b.queue.val[nodeQueueIndex];
                laneHasNode = nodeData.instance != INVALID_U64 && nodeData.node != INVALID_PTR;
                if (laneHasNode)
                {
                    GroupNodeData[groupIndex] = nodeData;
                }
            }

            if (laneHasNode)
            {
                InterlockedOr(GroupNodeMask, 1u << groupIndex);
            }
            AllMemoryBarrierWithGroupSync();

            readyNodeMask = GroupNodeMask;

            // If the first slot is valid, process the contiguous ready prefix.
            if ((readyNodeMask & 1u) == 1)
            {
                uint batchSize = firstbitlow(~readyNodeMask);
                ProcessNodeBatch(batchSize, groupIndex);
                if (groupIndex < batchSize)
                {
                    b.queue.val[nodeQueueIndex] = InstanceBvhRoot(INVALID_U64, INVALID_PTR);
                }

                nodeBatchLocalOffset += batchSize;
                continue;
            }
        }

        // No processable nodes this iteration; poll meshlet batches.
        if (meshletBatchStartIndex == INVALID_U32)
        {
            if (groupIndex == 0)
            {
                InterlockedAdd(b.queue_state.val.meshlet_batch_read_offset, 1, GroupMeshletBatchStartIndex);
            }
            GroupMemoryBarrierWithGroupSync();
            meshletBatchStartIndex = GroupMeshletBatchStartIndex;
        }

        if (!processNodeQueue && GroupMeshletBatchStartIndex >= MAX_CANDIDATE_MESHLETS_BATCH)
            break;

        if (groupIndex == 0)
        {
            GroupNodeCount = b.queue_state.val.node_count;
            GroupMeshletBatchReadySize = b.meshlet_batch_buffer.val[meshletBatchStartIndex];
        }
        GroupMemoryBarrierWithGroupSync();

        uint meshletBatchReadySize = GroupMeshletBatchReadySize;
        if (!processNodeQueue && meshletBatchReadySize == 0)
            break;

        bool canRunMeshletPass =
            (processNodeQueue && meshletBatchReadySize == DAG_CULL_GROUP_SIZE) ||
            (!processNodeQueue && meshletBatchReadySize > 0);

        if (canRunMeshletPass)
        {
            ProcessMeshletBatch(meshletBatchStartIndex, meshletBatchReadySize, groupIndex);
            meshletBatchStartIndex = INVALID_U32;
        }

        if (processNodeQueue && GroupNodeCount == 0)
        {
            processNodeQueue = false;
        }
    }
}