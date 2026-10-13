// Rings the outlined meshlets: colours every pixel within `radius` of a marked pixel of
// `mask` that isn't marked itself. The workgroup's 8x8 tile plus an apron of `MAX_RADIUS` is
// loaded into groupshared memory once (9 loads per thread), then the disc is tested there.
#include "bindless.slang"

struct Bindings {
    // Nonzero where `raster_outline` drew an outlined meshlet.
    Tex2D<float> mask;
    MutImage2D<float4> out;
    // The radius is capped at MAX_RADIUS.
    float4 outline_color_and_radius;
    // Size of the view inside `out` and `mask`, which can be larger.
    uint2 viewport_size;
}

[[vk::push_constant]] Bindings b;

#define TILE 8
#define MAX_RADIUS 8
#define APRON (TILE + 2 * MAX_RADIUS)

groupshared float tile[APRON * APRON];

[shader("compute")]
[numthreads(TILE, TILE, 1)]
void computeMain(int2 dtid: SV_DispatchThreadID, int2 gtid: SV_GroupThreadID, int2 gid: SV_GroupID)
{
    let origin = gid * TILE - MAX_RADIUS;
    for (uint i = gtid.y * TILE + gtid.x; i < APRON * APRON; i += TILE * TILE) {
        let p = origin + int2(i % APRON, i / APRON);
        let inside = all(p >= 0) && all(p < int2(b.viewport_size));
        tile[i] = inside ? b.mask.Load(p) : 0.0;
    }
    GroupMemoryBarrierWithGroupSync();

    if (any(dtid >= int2(b.viewport_size))) {
        return;
    }
    let local = gtid + MAX_RADIUS;
    if (tile[local.y * APRON + local.x] != 0.0) {
        return;
    }
    let radius = min(b.outline_color_and_radius.w, float(MAX_RADIUS));
    let iradius = int(ceil(radius));
    for (int dy = -iradius; dy <= iradius; dy++) {
        for (int dx = -iradius; dx <= iradius; dx++) {
            if (float(dx * dx + dy * dy) > radius * radius) continue;
            if (tile[(local.y + dy) * APRON + local.x + dx] != 0.0) {
                b.out.Store(dtid, float4(b.outline_color_and_radius.xyz, 1.0));
                return;
            }
        }
    }
}
