#define USE_CLOCKS
// Meshlets of blended materials, accumulated for weighted blended order-independent
// transparency (McGuire and Bavoil 2013); the tonemap pass composites them.
#include "scene_raster.slang"

// Attachment 0 is the opaque pass's HDR colour, which this pass leaves alone. Slang numbers the
// outputs from 0 whatever their `SV_Target` index, hence the explicit locations.
struct BlendedOutput {
    // The weighted premultiplied colour and the weight, added up.
    [[vk::location(1)]] float4 accum : SV_Target1;
    // Attenuates the revealage.
    [[vk::location(2)]] float alpha : SV_Target2;
};

[shader("fragment")]
BlendedOutput raster_blended_fragment(in VSOutput input) {
    let color = shade(input);
    // Too transparent to lower the 8-bit revealage below 1, where the tonemap wouldn't reset
    // the weight it adds to `accum`.
    if (color.a < 1.0 / 255.0) {
        discard;
    }
    // Eq. 10 of the paper: nearer fragments weigh more. `accum` holds half floats (at most 65504),
    // so the weighted colour is clamped, which clips only highlights right in front of the camera
    // (an opaque colour above 5 at 30 cm, above 65 at 1 m). Scaling the weight down instead
    // darkened far layers, whose colours became subnormal.
    let distance = length(b.camera_position.xyz - input.world_position);
    let weight = color.a * clamp(10.0 / (1e-5 + pow(distance / 5.0, 2.0) + pow(distance / 200.0, 6.0)), 1e-2, 3e3);
    return BlendedOutput(float4(min(color.rgb * weight, 16384.0), weight) * color.a, color.a);
}
