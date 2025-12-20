#include "bindless.slang"

struct PostBindings {
    column_major float4x4 inverse_proj;
    column_major float4x4 inverse_view;
    float4 window_size;
    Image2D<float> depth;
    Image2D<float4> color;
    MutImage2D<float4> out;
}

[[vk::push_constant]] PostBindings bindings;

float3 view_dir(uint2 pixelCoord) {
    const var pixelCenter = float2(pixelCoord) + float2(0.5);
    const var inUV = pixelCenter / float2(bindings.window_size.xy);
    var d = inUV * 2.0 - 1.0;
    var dir = inUV * 2.0 - 1.0;
    var target = mul(bindings.inverse_proj, float4(dir.x, dir.y, 1, 1));
    var direction = mul(bindings.inverse_view, float4(normalize(target.xyz), 0));
    return direction.xyz;
}

[shader("compute")]
[numthreads(8, 8, 1)]
void post(uint2 dtid: SV_DispatchThreadID) {
    float4 color = bindings.color.Load(dtid);
    float depth = bindings.depth.Instance[dtid];

    if (depth == 0.0) {
        bindings.out.Store(dtid, float4(view_dir(dtid), 1.0));
    } else {
        bindings.out.Store(dtid, color);
    }
}