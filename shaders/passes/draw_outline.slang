#include "bindless.slang"

struct Bindings {
    Tex2D<float> depth;
    MutImage2D<float4> out;
    float4 outline_color_and_radius;
    int2 view_port_offset;
    uint2 view_port_size;
    uint2 swpachain_size;
}

[[vk::push_constant]] Bindings b;

bool is_in_radius(int2 dtid, float radius)
{
    int iradius = int(ceil(radius));
    for (int dy = -iradius; dy <= iradius; dy++)
    {
        for (int dx = -iradius; dx <= iradius; dx++)
        {
            if (length(float2(dx, dy)) > radius) continue;
            let offset = dtid + int2(dx, dy);
            if (any(offset >= (b.view_port_offset + b.view_port_size)) || any(offset >= b.swpachain_size)) continue;
            if (b.depth.Load(offset) != 0.0)
                return true;
        }
    }
    return false;
}

[shader("compute")]
[numthreads(8, 8, 1)]
void computeMain(int2 dtid: SV_DispatchThreadID)
{
    let ndtid = dtid + b.view_port_offset;
    if (any(ndtid >= (b.view_port_offset + b.view_port_size)) || any(ndtid >= b.swpachain_size)) {
        return;
    }
    if (b.depth.Load(ndtid) != 0.0) return;

    if (is_in_radius(ndtid, b.outline_color_and_radius.w)) {
        b.out.Store(ndtid, float4(b.outline_color_and_radius.xyz, 1.0));
    }
}