#include "bindless.slang"

struct TonemapBindings {
    Image2D<float4> hdr;
    MutImage2D<float4> out;
    uint2 view_port_size;
    float exposure;
}

[[vk::push_constant]] TonemapBindings bindings;

float3 aces(float3 x) {
    return saturate((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14));
}

float3 linear_to_srgb(float3 c) {
    return select(c <= 0.0031308, c * 12.92, 1.055 * pow(c, 1.0 / 2.4) - 0.055);
}

[shader("compute")]
[numthreads(8, 8, 1)]
void tonemap(uint2 dtid: SV_DispatchThreadID) {
    if (any(dtid >= bindings.view_port_size)) {
        return;
    }
    let hdr = bindings.hdr.Load(dtid).rgb * bindings.exposure;
    bindings.out.Store(dtid, float4(linear_to_srgb(aces(hdr)), 1.0));
}
