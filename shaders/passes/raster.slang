#define USE_CLOCKS
// Opaque and alpha-tested meshlets, written into the G-buffer, which the composite pass lights.
#include "scene_raster.slang"

[shader("fragment")]
uint4 raster_fragment(in VSOutput input) : SV_Target {
    float alpha;
    return material_surface(input, alpha).pack().data0;
}
