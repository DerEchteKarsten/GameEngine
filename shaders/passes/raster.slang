#include "random.slang"
#include "datatypes.slang"
#include "bindless.slang"
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
    Buf<Material> materials;
}

[[vk::push_constant]] RasterBindings b;

[shader("vertex")]
VSOutput raster_vertex(uint instance_index: SV_InstanceID, uint vertex_index : SV_VertexID)
{
    let instanced_meshlet = b.meshlets.val[instance_index];
    let meshlet = *(instanced_meshlet.meshlet);

    if (vertex_index / 3u >= meshlet.triangle_count) {
        return VSOutput(float4(0.0 / 0.0, 0.0 / 0.0, 0.0 / 0.0, 0.0 / 0.0), float3(0.0), float3(0.0), float2(0.0), 0, 0);
    }

    let vertex_id = meshlet.triangle_index[vertex_index];
    let vertex = meshlet.vertex_index[uint(vertex_id)];

    VSOutput output;
    let transform = b.instance_transforms.val[instanced_meshlet.instance];
    let world_space = mul(transform, float4(vertex.position_and_uv1.xyz, 1.0));
    let view_space = mul(b.view, world_space);
    let clip_space = mul(b.proj, view_space);
    output.position = clip_space;
    output.world_position = world_space.xyz;
    output.normal = normalize(mul((float3x3)transform, vertex.normal_and_uv2.xyz));
    output.uv = float2(vertex.position_and_uv1.w, vertex.normal_and_uv2.w);
    output.instance = uint(instanced_meshlet.instance);
    output.meshlet = uint(uint64_t(instanced_meshlet.meshlet));
    return output;
}

struct PSOutput {
    float4 color;
};

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

static const float3 LIGHT_DIRECTION = normalize(float3(0.4, 1.0, 0.3));
static const float3 LIGHT_COLOR = float3(3.0);
static const float3 AMBIENT = float3(0.03);

[shader("fragment")]
PSOutput raster_fragment(in VSOutput input)
{
    let material = b.materials.val[input.instance];

    var color = material.color;
    if (material.color_texture != NO_TEXTURE) {
        color *= sample_texture(material.color_texture, input.uv);
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
    if (material.normal_texture != NO_TEXTURE) {
        var tangent_normal = sample_texture(material.normal_texture, input.uv).xyz * 2.0 - 1.0;
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

    PSOutput output;
    output.color = float4(lit, 1.0);
    // output.color = float4(random.uniform_float(), random.uniform_float(), random.uniform_float(), 1.0);
    return output;
}
