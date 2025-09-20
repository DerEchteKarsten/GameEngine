#include "bindless.slang"
#include "datatypes.slang"
#include "voxel_raytracing.slang"

struct Constants {
    column_major float4x4 proj_inverse;
    column_major float4x4 view_inverse;
    float2 window_size;
    float3 camara_position;
    Node* world;
    ImageHandle2D out;
}

[[vk::push_constant]] Constants bindings;

float3 view_dir(uint2 pixelCoord) {
    const var pixelCenter = float2(pixelCoord) + float2(0.5);
    const var inUV = pixelCenter / float2(bindings.window_size);
    var d = inUV * 2.0 - 1.0;
    var dir = inUV * 2.0 - 1.0;
    var target = mul(bindings.proj_inverse, float4(dir.x, dir.y, 1, 1));
    var direction = mul(bindings.view_inverse, float4(normalize(target.xyz), 0));
    return direction.xyz;
}

[shader("compute")]
[numthreads(8, 8, 1)]
void main(uint3 threadId: SV_DispatchThreadID) {
    var pixelCoord = threadId.xy;

    var world = VoxelMap(4, bindings.world);

    printf("%i, %i, %llu \n", bindings.world[0].ChildPtr, bindings.world[0].IsLeaf, bindings.world[0].PopMask);

    let direction = view_dir(pixelCoord);
    let hit_info = world.Traversal_HDDA(bindings.camara_position, direction, false);

    float3 color;
    if (!hit_info.Miss) {
        color = float3(exp(-float3(hit_info.Dist) * 0.3));
    } else {
        color = direction;
    }

    bindings.out.Store(pixelCoord, float4(color, 1.0));
}