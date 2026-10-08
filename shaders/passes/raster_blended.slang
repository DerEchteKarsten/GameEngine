// Meshlets of blended materials, accumulated for weighted blended order-independent
// transparency (McGuire and Bavoil 2013); the tonemap pass composites them.
#include "scene_raster.slang"

struct BlendedOutput {
    // The weighted premultiplied colour and the weight, added up.
    float4 accum : SV_Target0;
    // Attenuates the revealage.
    float alpha : SV_Target1;
};

SCENE_MESHLET_SHADERS(raster_blended)

[shader("fragment")]
BlendedOutput raster_blended_fragment(in VSOutput input) {
    let color = shade(input);
    // Eq. 10 of the paper: nearer fragments weigh more.
    let distance = length(b.camera_position.xyz - input.world_position);
    let weight = color.a * clamp(10.0 / (1e-5 + pow(distance / 5.0, 2.0) + pow(distance / 200.0, 6.0)), 1e-2, 3e3);
    return BlendedOutput(float4(color.rgb * color.a, color.a) * weight, color.a);
}
