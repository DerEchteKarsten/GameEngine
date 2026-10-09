#define USE_CLOCKS
#include "bindless.slang"
#include "profile.slang"
#include "math.slang"

struct TonemapBindings {
    // The opaque scene in linear HDR colour, where it has a depth.
    Tex2D<float4> hdr;
    // Still cleared (0) where nothing was drawn, which shows the sky.
    Tex2D<float> depth;
    // The weighted blended OIT targets of the raster pass's blended fragments. This pass resets
    // them where it reads them, so the blended pass doesn't clear every pixel. `revealage` is
    // read through the sampler: a storage load of it cost a quarter of the pass.
    MutImage2D<float4> accum;
    MutTex2D<float> revealage;
    MutImage2D<float4> out;
    column_major float4x4 inverse_proj;
    column_major float4x4 inverse_view;
    uint2 view_port_size;
    float exposure;
}

[SpecializationConstant] const uint sky;

[[vk::push_constant]] TonemapBindings bindings;
// Drawn where there is no sky texture, or while it loads.
static const float3 NO_SKY_COLOR = float3(0.05);

float3 view_dir(uint2 pixelCoord) {
    const var pixelCenter = float2(pixelCoord) + float2(0.5);
    const var inUV = pixelCenter / float2(bindings.view_port_size);
    var dir = inUV * 2.0 - 1.0;
    var target = mul(bindings.inverse_proj, float4(dir.x, dir.y, 1, 1));
    var direction = mul(bindings.inverse_view, float4(normalize(target.xyz), 0));
    return normalize(direction.xyz);
}

// Z is up and u grows towards +Y (to the right of +X); v = 0 is the top row of the texture.
float2 equirectangular_uv(float3 dir) {
    return float2(atan2(dir.y, dir.x) * 0.5 * FRAC_1_PI + 0.5, acos(clamp(dir.z, -1.0, 1.0)) * FRAC_1_PI);
}

float3 sky_color(uint2 pixel) {
    if (sky == NULL_HANDLE) {
        return NO_SKY_COLOR;
    }
    let texture = Tex2D<float4>(sky, NULL_HANDLE);
    let dir = view_dir(pixel);
    // The mip whose texels are about a pixel apart: the angle between neighbouring pixels
    // times the texels per radian of the texture's rows.
    let pixel_angle = length(view_dir(pixel + uint2(1, 0)) - dir);
    let lod = log2(max(pixel_angle * float(texture.GetDimensions().y) * FRAC_1_PI, 1.0));
    // Sampler 2 repeats, so the seam where u wraps is filtered across.
    return texture.SampleLevel(equirectangular_uv(dir), lod, 2).rgb;
}

float3 aces(float3 x) {
    return saturate((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14));
}

float3 linear_to_srgb(float3 c) {
    return select(c <= 0.0031308, c * 12.92, 1.055 * pow(c, 1.0 / 2.4) - 0.055);
}

[shader("compute")]
[numthreads(8, 8, 1)]
void tonemap(uint2 dtid: SV_DispatchThreadID) {
    PROFILE(PROFILE_COMPUTE);
    if (any(dtid >= bindings.view_port_size)) {
        return;
    }
    var hdr: float3;
    if (bindings.depth.Load(dtid) == 0.0) {
        hdr = sky_color(dtid);
    } else {
        hdr = bindings.hdr.Load(dtid).rgb;
    }
    let revealage = bindings.revealage.Load(dtid);
    // Most pixels have no blended layer (revealage stays 1), so they skip the 16-byte `accum`.
    if (revealage < 1.0) {
        let accum = bindings.accum.Load(dtid);
        // The weighted average colour of the blended layers, over what they let through.
        let blended = accum.rgb / max(accum.a, 1e-5);
        hdr = lerp(blended, hdr, revealage);
        bindings.accum.Store(dtid, float4(0.0));
        bindings.revealage.Store(dtid, 1.0);
    }
    hdr *= bindings.exposure;
    bindings.out.Store(dtid, float4(linear_to_srgb(aces(hdr)), 1.0));
}
