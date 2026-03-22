#include "random.slang"
#include "datatypes.slang"
#include "bindless.slang"
struct VSOutput {
    float4 position : SV_Position;
    float3 color; 
};

struct RasterBindings {
    float4x4 view;
    float4x4 proj;
    Buf<float4x4> instance_transforms;
    Buf<InstancedMeshlet> meshlets;
}

[[vk::push_constant]] RasterBindings b;

[shader("vertex")]
VSOutput vertex(uint instance_index: SV_InstanceID, uint vertex_index : SV_VertexID)
{
    let instanced_meshlet = b.meshlets.val[instance_index];
    let meshlet = *(instanced_meshlet.meshlet);

    if (vertex_index / 3u >= meshlet.triangle_count) {
        return VSOutput(float4(0.0 / 0.0, 0.0 / 0.0, 0.0 / 0.0, 0.0 / 0.0), float3(0.0));
    }

    let vertex_id = meshlet.triangle_index[vertex_index];
    let vertex = meshlet.vertex_index[uint(vertex_id)];

    VSOutput output;
    let transform = b.instance_transforms.val[instanced_meshlet.instance];
    let world_space = mul(transform, float4(vertex.position_and_uv1.xyz, 1.0));
    let view_space = mul(b.view, world_space);
    let clip_space = mul(b.proj, view_space);
    output.position = clip_space;
    var rand = RandomSamplerState(uint2(uint64_t(instanced_meshlet.meshlet), instanced_meshlet.instance), 0);
    output.color = float3(rand.uniform_float(), rand.uniform_float(), rand.uniform_float());
    return output;
}

struct PSOutput {
    float4 color : SV_Target;
};

[shader("fragment")]
PSOutput fragment(in VSOutput input)
{
    PSOutput output;
    output.color = float4(input.color.xyz, 1.0);
    return output;
}