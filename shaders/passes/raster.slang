#define USE_CLOCKS
// Opaque and alpha-tested meshlets, drawn into the HDR target.
#include "scene_raster.slang"

[shader("fragment")]
float3 raster_fragment(in VSOutput input) : SV_Target {
    let color = shade(input).rgb;
    // The B10G11R11 target truncates, which darkened every channel, blue (5 mantissa bits) the
    // most. Half its last place (from the exponent) makes it round to nearest.
    let exponent = asfloat(asuint(color) & 0x7F800000u);
    return color + exponent * float3(1.0 / 128.0, 1.0 / 128.0, 1.0 / 64.0);
}
