#include "bindless.slang"
#include "datatypes.slang"

#define WORKGROUP_SIZE 32
#include "voxel_raytracing.slang"

struct Constants {
    column_major float4x4 proj_inverse;
    column_major float4x4 view_inverse;
    float2 window_size;
    uint tree_scale;
    uint pad1;
    float4 camara_position;
}

struct Bindings {
    Constants const;
    Node* world;
    ImageHandle2D out;
}

[[vk::push_constant]] Bindings bindings;

float3 view_dir(uint2 pixelCoord) {
    const var pixelCenter = float2(pixelCoord) + float2(0.5);
    const var inUV = pixelCenter / float2(bindings.const.window_size);
    var d = inUV * 2.0 - 1.0;
    var dir = inUV * 2.0 - 1.0;
    var target = mul(bindings.const.proj_inverse, float4(dir.x, dir.y, 1, 1));
    var direction = mul(bindings.const.view_inverse, float4(normalize(target.xyz), 0));
    return direction.xyz;
}

[shader("compute")]
[numthreads(8, 4, 1)]
void main(uint3 threadId: SV_DispatchThreadID) {
    var pixelCoord = threadId.xy;

    // printf("%i", bindings.const.tree_scale);

    var world = VoxelMap(4, bindings.world);

    let direction = view_dir(pixelCoord);
    let hit_info = world.RayCast(int3(0), bindings.const.camara_position.xyz / float(bindings.const.tree_scale), direction, true);

    float3 color;
    if (!hit_info.Miss) {
        color = float3((dot(hit_info.Normal, normalize(float3(0.3, -0.8, 0.5))) + 1.0) / 2.0);
    } else {
        color = direction;
    }

    bindings.out.Store(pixelCoord, float4(color, 0.0));
}