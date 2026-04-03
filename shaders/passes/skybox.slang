#include "bindless.slang"

struct SkyBoxBindings {
    column_major float4x4 inverse_proj;
    column_major float4x4 inverse_view;
    MutImage2D<float4> out;
    int2 view_port_offset;
    uint2 view_port_size;
    uint2 swpachain_size;
}

[[vk::push_constant]] SkyBoxBindings bindings;

float3 view_dir(uint2 pixelCoord) {
    const var pixelCenter = float2(pixelCoord) + float2(0.5);
    const var inUV = pixelCenter / float2(bindings.view_port_size);
    var d = inUV * 2.0 - 1.0;
    var dir = inUV * 2.0 - 1.0;
    var target = mul(bindings.inverse_proj, float4(dir.x, dir.y, 1, 1));
    var direction = mul(bindings.inverse_view, float4(normalize(target.xyz), 0));
    return direction.xyz;
}

[shader("compute")]
[numthreads(8, 8, 1)]
void skybox(uint2 dtid: SV_DispatchThreadID) {
    let ndtid = dtid + bindings.view_port_offset;
    if (any(ndtid >= (bindings.view_port_offset + bindings.view_port_size)) || any(ndtid >= bindings.swpachain_size)){
        return;
    }
    
    bindings.out.Store(ndtid, float4(view_dir(dtid), 1.0));
}