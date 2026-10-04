#include "bindless.slang"

struct SkyBoxBindings {
    column_major float4x4 inverse_proj;
    column_major float4x4 inverse_view;
    MutImage2D<float4> out;
    // Size of the view inside `out`, which can be larger.
    uint2 view_port_size;
}

[[vk::push_constant]] SkyBoxBindings bindings;

float3 view_dir(uint2 pixelCoord) {
    const var pixelCenter = float2(pixelCoord) + float2(0.5);
    const var inUV = pixelCenter / float2(bindings.view_port_size);
    var dir = inUV * 2.0 - 1.0;
    var target = mul(bindings.inverse_proj, float4(dir.x, dir.y, 1, 1));
    var direction = mul(bindings.inverse_view, float4(normalize(target.xyz), 0));
    return direction.xyz;
}

[shader("compute")]
[numthreads(8, 8, 1)]
void skybox(uint2 dtid: SV_DispatchThreadID) {
    if (any(dtid >= bindings.view_port_size)){
        return;
    }

    bindings.out.Store(dtid, float4(view_dir(dtid), 1.0));
}
