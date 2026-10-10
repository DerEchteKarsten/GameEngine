// The scene's lighting (one directional light, GGX + Lambert, plus ambient), shared by the
// composite pass for the G-buffer and the forward-rendered blended surfaces.
#pragma once
#include "gbuffer_helpers.slang"
#include "math.slang"

static const float3 LIGHT_DIRECTION = normalize(float3(0.1, 0.3, 0.1));
static const float3 LIGHT_COLOR = float3(3.0);
static const float3 AMBIENT = float3(0.03);

// The linear HDR colour of the surface seen along `view`, which points towards the camera.
float3 light(GbufferData s, float3 view) {
    let halfway = normalize(view + LIGHT_DIRECTION);
    let ndotl = max(dot(s.normal, LIGHT_DIRECTION), 0.0);
    let ndotv = max(dot(s.normal, view), 1e-4);
    let ndoth = max(dot(s.normal, halfway), 0.0);
    let vdoth = max(dot(view, halfway), 0.0);

    let roughness = max(s.roughness, 0.02);
    let a2 = roughness * roughness * roughness * roughness;
    let denom = ndoth * ndoth * (a2 - 1.0) + 1.0;
    let d = a2 * FRAC_1_PI / (denom * denom);
    let k = (roughness + 1.0) * (roughness + 1.0) / 8.0;
    let g = (ndotv / (ndotv * (1.0 - k) + k)) * (ndotl / (ndotl * (1.0 - k) + k));
    let f0 = lerp(float3(0.04), s.albedo, s.metalness);
    let f = f0 + (1.0 - f0) * pow(1.0 - vdoth, 5.0);
    let specular = d * g * f / (4.0 * ndotv * ndotl + 1e-4);
    let diffuse = (1.0 - f) * (1.0 - s.metalness) * s.albedo * FRAC_1_PI;

    return (diffuse + specular) * ndotl * LIGHT_COLOR
        + AMBIENT * s.albedo * s.occlusion
        + s.emissive;
}
