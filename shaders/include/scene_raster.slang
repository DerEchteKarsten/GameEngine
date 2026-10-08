// The scene's meshlet mesh shader and material shading, shared by the scene's raster passes.
#pragma once
#include "random.slang"
#include "datatypes.slang"
#include "bindless.slang"
#include "profile.slang"
#include "math.slang"
struct VSOutput {
    float4 position : SV_Position;
    float3 world_position;
    float3 normal;
    float2 uv;
    uint instance;
    uint meshlet;
};

struct RasterBindings {
    float4x4 view;
    float4x4 proj;
    float4 camera_position;
    Buf<float4x4> instance_transforms;
    Buf<InstancedMeshlet> meshlets;
    Buf<MeshletDraw> draw;
    Buf<Material *> instance_materials;
}

[[vk::push_constant]] RasterBindings b;

// The meshlet builder's limits (`build_meshlets` in `core/src/assets/mesh.rs`). Fewer vertices
// leave the mesh shader less output to reserve, which made it faster.
#define MAX_MESHLET_VERTICES 128
#define MAX_MESHLET_TRIANGLES 128
// Invocations per mesh workgroup, what the driver prefers (`maxPreferredMeshWorkGroupInvocations`
// on Intel); a thread per triangle was slower.
#define MESH_THREADS 16

VSOutput scene_vertex(InstancedMeshlet instanced_meshlet, Meshlet meshlet, uint index)
{
    let vertex = meshlet.vertex_index[index];
    let transform = b.instance_transforms.val[instanced_meshlet.instance];
    let world_space = mul(transform, float4(vertex.position_and_uv1.xyz, 1.0));

    VSOutput output;
    output.position = mul(b.proj, mul(b.view, world_space));
    output.world_position = world_space.xyz;
    output.normal = normalize(mul((float3x3)transform, vertex.normal_and_uv2.xyz));
    output.uv = float2(vertex.position_and_uv1.w, vertex.normal_and_uv2.w);
    output.instance = uint(instanced_meshlet.instance);
    output.meshlet = uint(uint64_t(instanced_meshlet.meshlet));
    return output;
}

// The mesh shader of every scene raster pass: a workgroup per meshlet of `b.meshlets` (see
// `MeshletDraw`).
[shader("mesh")]
[outputtopology("triangle")]
[numthreads(MESH_THREADS, 1, 1)]
void scene_mesh(
    uint thread: SV_GroupThreadID,
    uint2 group: SV_GroupID,
    out indices uint3 triangles[MAX_MESHLET_TRIANGLES],
    out vertices VSOutput vertices[MAX_MESHLET_VERTICES]
) {
    PROFILE(PROFILE_MESH);
    let index = group.x * MESHLET_STRIDE + group.y;
    if (index >= b.draw.val.meshlet_count) {
        // Past the end of the list, in the last stride.
        SetMeshOutputCounts(0, 0);
        return;
    }
    let instanced_meshlet = b.meshlets.val[index];
    let meshlet = *instanced_meshlet.meshlet;
    SetMeshOutputCounts(meshlet.vertex_count, meshlet.triangle_count);
    for (uint i = thread; i < meshlet.triangle_count; i += MESH_THREADS) {
        let corners = meshlet.triangle_index + i * 3;
        triangles[i] = uint3(corners[0], corners[1], corners[2]);
    }
    for (uint i = thread; i < meshlet.vertex_count; i += MESH_THREADS) {
        vertices[i] = scene_vertex(instanced_meshlet, meshlet, i);
    }
}

float4 sample_texture(uint texture, float2 uv) {
    return Tex2D<float4>(texture, NO_TEXTURE).Sample(uv, 2);
}

float3 perturb_normal(float3 normal, float3 position, float2 uv, float3 tangent_normal) {
    let dp1 = ddx(position);
    let dp2 = ddy(position);
    let duv1 = ddx(uv);
    let duv2 = ddy(uv);
    let dp2perp = cross(dp2, normal);
    let dp1perp = cross(normal, dp1);
    let tangent = dp2perp * duv1.x + dp1perp * duv2.x;
    let bitangent = dp2perp * duv1.y + dp1perp * duv2.y;
    let inv_max = rsqrt(max(dot(tangent, tangent), dot(bitangent, bitangent)));
    let tbn = float3x3(tangent * inv_max, bitangent * inv_max, normal);
    return normalize(mul(tangent_normal, tbn));
}

static const float3 LIGHT_DIRECTION = normalize(float3(0.1, 0.3, 0.1));
static const float3 LIGHT_COLOR = float3(3.0);
static const float3 AMBIENT = float3(0.03);

/// The lit colour of the fragment and its alpha; discards it below the alpha cutoff.
float4 shade(VSOutput input)
{
    let material = *b.instance_materials.val[input.instance];

    var color = material.color;
    if (material.color_texture != NO_TEXTURE) {
        color *= sample_texture(material.color_texture, input.uv);
    }
    // Alpha test; a negative cutoff keeps every fragment and blends it instead.
    if (color.a < material.alpha_cutoff) {
        discard;
    }
    var metallic = material.metalic_factor;
    var roughness = material.roughness_factor;
    if (material.metallic_roughness_texture != NO_TEXTURE) {
        let mr = sample_texture(material.metallic_roughness_texture, input.uv);
        roughness *= mr.g;
        metallic *= mr.b;
    }
    var occlusion = 1.0;
    if (material.occlusion_texture != NO_TEXTURE) {
        occlusion = lerp(1.0, sample_texture(material.occlusion_texture, input.uv).r, material.occlusion_strength);
    }
    var emissive = material.emissive;
    if (material.emissive_texture != NO_TEXTURE) {
        emissive *= sample_texture(material.emissive_texture, input.uv).rgb;
    }
    var normal = normalize(input.normal);
    let normal_sample = material.normal_texture != NO_TEXTURE
        ? sample_texture(material.normal_texture, input.uv).xy
        : float2(1.0);
    // (1, 1) is the white placeholder of a texture that is still loading; no BC5 unit normal decodes to it.
    if (any(normal_sample != 1.0)) {
        // Normal maps are baked with two channels (BC5): z follows from the unit length.
        var tangent_normal = float3(normal_sample * 2.0 - 1.0, 0.0);
        tangent_normal.z = sqrt(saturate(1.0 - dot(tangent_normal.xy, tangent_normal.xy)));
        tangent_normal.xy *= material.normal_scale;
        normal = perturb_normal(normal, input.world_position, input.uv, tangent_normal);
    }

    // Single directional light, GGX + Lambert.
    let view = normalize(b.camera_position.xyz - input.world_position);
    let halfway = normalize(view + LIGHT_DIRECTION);
    let ndotl = max(dot(normal, LIGHT_DIRECTION), 0.0);
    let ndotv = max(dot(normal, view), 1e-4);
    let ndoth = max(dot(normal, halfway), 0.0);
    let vdoth = max(dot(view, halfway), 0.0);

    roughness = max(roughness, 0.02);
    let a2 = roughness * roughness * roughness * roughness;
    let denom = ndoth * ndoth * (a2 - 1.0) + 1.0;
    let d = a2 * FRAC_1_PI / (denom * denom);
    let k = (roughness + 1.0) * (roughness + 1.0) / 8.0;
    let g = (ndotv / (ndotv * (1.0 - k) + k)) * (ndotl / (ndotl * (1.0 - k) + k));
    let f0 = lerp(float3(0.04), color.rgb, metallic);
    let f = f0 + (1.0 - f0) * pow(1.0 - vdoth, 5.0);
    let specular = d * g * f / (4.0 * ndotv * ndotl + 1e-4);
    let diffuse = (1.0 - f) * (1.0 - metallic) * color.rgb * FRAC_1_PI;

    let lit = (diffuse + specular) * ndotl * LIGHT_COLOR
        + AMBIENT * color.rgb * occlusion
        + emissive;

    var random = RandomSamplerState(uint2(input.meshlet, 0), 0);

    // return float4(random.uniform_float(), random.uniform_float(), random.uniform_float(), 1.0);
    // return float4(color.rgb, 1.0);
    return float4(lit, color.a);
}
