#include "bindless.slang"
#include "math.slang"

struct SkyBoxBindings {
    column_major float4x4 inverse_proj;
    column_major float4x4 inverse_view;
    MutImage2D<float4> out;
    // Size of the view inside `out`, which can be larger.
    uint2 view_port_size;
    // Bindless index of an equirectangular texture, NULL_HANDLE for none.
    uint sky;
}

[[vk::push_constant]] SkyBoxBindings bindings;

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

// Y is up; v = 0 is the top row of the texture.
float2 equirectangular_uv(float3 dir) {
    return float2(atan2(dir.z, dir.x) * 0.5 * FRAC_1_PI + 0.5, acos(clamp(dir.y, -1.0, 1.0)) * FRAC_1_PI);
}

[shader("compute")]
[numthreads(8, 8, 1)]
void skybox(uint2 dtid: SV_DispatchThreadID) {
    if (any(dtid >= bindings.view_port_size)){
        return;
    }

    let dir = view_dir(dtid);
    var color = NO_SKY_COLOR;
    if (bindings.sky != NULL_HANDLE) {
        let sky = Tex2D<float4>(bindings.sky, NULL_HANDLE);
        // The mip whose texels are about a pixel apart: the angle between neighbouring pixels
        // times the texels per radian of the texture's rows.
        let pixel_angle = length(view_dir(dtid + uint2(1, 0)) - dir);
        let lod = log2(max(pixel_angle * float(sky.GetDimensions().y) * FRAC_1_PI, 1.0));
        // Sampler 2 repeats, so the seam where u wraps is filtered across.
        color = sky.SampleLevel(equirectangular_uv(dir), lod, 2).rgb;
    }
    bindings.out.Store(dtid, float4(color, 1.0));
}
