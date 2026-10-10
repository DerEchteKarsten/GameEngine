#include "bindless.slang"

struct Bindings {
    Tex2D<float> depth;
    MutImage2D<float4> out;
    float4 outline_color_and_radius;
    // Size of the view inside `out` and `depth`, which can be larger.
    uint2 viewport_size;
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
            if (any(offset < 0) || any(offset >= int2(b.viewport_size))) continue;
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
    if (any(dtid >= int2(b.viewport_size))) {
        return;
    }
    if (b.depth.Load(dtid) != 0.0) return;

    if (is_in_radius(dtid, b.outline_color_and_radius.w)) {
        b.out.Store(dtid, float4(b.outline_color_and_radius.xyz, 1.0));
    }
}
