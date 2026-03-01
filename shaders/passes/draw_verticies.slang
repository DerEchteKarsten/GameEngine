#include "random.slang"
#include "datatypes.slang"
#include "bindless.slang"

struct DrawVertexBindings {
    float4x4 view;
    float4x4 proj;
    Buf<uint64_t> instances;
    uint instance;
    uint offset;
    uint2 window_size;
    MutImage2D out;
    uint num_verticies;
}

[[vk::push_constant]] DrawVertexBindings b;

[shader("compute")]
[numthreads(64, 1, 1)]
void draw(uint dtid: SV_DispatchThreadID) {
    if (dtid > b.num_verticies) {
        return;
    }

    let ptr = b.instances.val[b.instance] + b.offset;
    let verticies = (Vertex *)ptr;
    let vertex = verticies[dtid];
    let world_space = float4(vertex.position_and_uv1.xyz, 1.0);
    let view_space = mul(b.view, world_space);
    let clip_space = mul(b.proj, view_space);
    let window_space = ((clip_space.xy / clip_space.w + 1.0) / 2.0) * float2(b.window_size);

    for (int y = 0; y < 8; y++) {
        for (int x = 0; x < 8; x++) {
            let pos = uint2(window_space) + uint2(x, y);
            if (all(pos < b.window_size) && all(pos > uint2(0)) && clip_space.z > 0.0 && clip_space.z < 1.0 && clip_space.w > 0.0) {
                b.out.Store(pos, float4(1.0, 0.0, 1.0, 1.0));
            }
        }
    }
}