#define USE_CLOCKS
// Opaque and alpha-tested meshlets, drawn into the HDR target.
#include "scene_raster.slang"

[shader("fragment")]
float4 raster_fragment(in VSOutput input) : SV_Target {
    return shade(input);
}
