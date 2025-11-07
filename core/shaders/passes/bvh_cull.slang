#include "datatypes.slang"

struct Bindings {
    BvhNode *bvh_nodes;
    float4x4 *instance_transforms;
    CullData *cull_data;
    InstancedOffset *bvh_node_stack;
    InstancedOffset* clusters;
    DispatchParams *dp;
};

[[vk::push_constant]] Bindings b;


bool pop(inout InstancedOffset node)
{
    var old_head = 0u;
    InterlockedAdd(b.dp.node_head, 1u, old_head);

    if (b.dp.node_tail <= old_head)
    {
        InterlockedAdd(b.dp.node_head, uint(-1));
        return false;
    }

    node = b.bvh_node_stack[old_head];
    return true;
}


uint push(uint n)
{
    uint oldTail;
    InterlockedAdd(b.dp.node_tail, n, oldTail);
    return oldTail;
}

[shader("compute")]
[numthreads(64, 1, 1)]
void main(uint gid: SV_GroupID, uint dtid: SV_DispatchThreadID, uint gtid: SV_GroupThreadID) {
    while(b.dp.done < 4 * 64) {
        var node = InstancedOffset(0,0);
        let has_job = pop(node);
        AllMemoryBarrierWithWaveSync();
        let all_idle = WaveActiveAllTrue(!has_job);

        if(all_idle) {
            var idle_value = 0;
            InterlockedAdd(b.dp.done, 1, idle_value);
            while(true) {
                if(b.dp.done < idle_value) {
                    break;
                }
                printf("IDLE!!");
            }
        }

        if(!has_job) {
            continue;
        }

        let bvh_node = b.bvh_nodes[node.offset];
        uint children[8];
        var child_count = 0;
        for(int i = 0; i < 8; i++) {
            let cluster_count = bvh_node.child_counts[i];
            if(cluster_count == 255u) {
                children[child_count] = bvh_node.aabbs[i].offset;
                child_count += 1;
            }else {
                var base = 0;
                InterlockedAdd(b.dp.indirect_draw.instance_count, child_count, base);
                var value = InstancedOffset(node.instance, bvh_node.aabbs[i].offset);
                for(int i = 0; i < cluster_count; i++) {
                    b.clusters[base + i] = value;
                    value.offset += 1;
                }
            }
        }

        let slot = push(child_count);
        for(int i = 0; i<8; i++) {
            b.bvh_node_stack[slot + i] = InstancedOffset(node.instance, children[i]);
        }
    }
}