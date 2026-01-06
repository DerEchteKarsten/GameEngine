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


bool pop(inout InstancedOffset node)
{
    var old_head = 0u;
    InterlockedAdd(b.dp.val.node_head, 1u, old_head);

    if (b.dp.val.node_tail <= old_head)
    {
        InterlockedAdd(b.dp.val.node_head, uint(-1));
        return false;
    }

    node = b.bvh_node_stack.val[old_head];
    return true;
}


uint push(uint n)
{
    uint oldTail;
    InterlockedAdd(b.dp.val.node_tail, n, oldTail);
    return oldTail;
}

[shader("compute")]
[numthreads(64, 1, 1)]
void bvh_cull(uint gid: SV_GroupID, uint dtid: SV_DispatchThreadID, uint gtid: SV_GroupThreadID) {
    while (b.dp.val.done < 4 * 64) {
        var node = InstancedOffset(0,0);
        let has_job = pop(node);
        AllMemoryBarrierWithWaveSync();
        let all_idle = WaveActiveAllTrue(!has_job);

        if(all_idle) {
            var idle_value = 0;
            InterlockedAdd(b.dp.val.done, 1, idle_value);
            while (true) {
                if (b.dp.val.done < idle_value) {
                    break;
                }
                printf("IDLE!!");
            }
        }

        if(!has_job) {
            continue;
        }

        let bvh_node = &b.bvh_nodes.val[node.offset];
        uint children[8];
        var child_count = 0;
        for (uint i = 0; i < 8; i++) {
            let cluster_count = get_byte(i, bvh_node.child_counts);
            if (cluster_count == 255u) {
                children[child_count] = asuint(bvh_node->aabbs[i].half_extent_and_offset.w);
                child_count += 1;
            }else {
                var base = 0;
                InterlockedAdd(b.dp.val.indirect_draw.instance_count, child_count, base);
                var value = InstancedOffset(node.instance, asuint(bvh_node->aabbs[i].half_extent_and_offset.w));
                for (uint j = 0; j < cluster_count; j++) {
                    b.clusters.val[base + j] = value;
                    value.offset += 1;
                }
            }
        }

        let slot = push(child_count);
        for (int i = 0; i < 8; i++) {
            b.bvh_node_stack.val[slot + i] = InstancedOffset(node.instance, children[i]);
        }
    }
}