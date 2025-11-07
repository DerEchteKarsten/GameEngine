#include "random.slang"
#include "datatypes.slang"

struct VSOutput {
    float4 position : SV_Position;
    float3 color; 
};

struct Bindings {
    float4x4 view;
    float4x4 proj;
    Vertex *verticies;
    uint8_t *indicies;
    Meshlet *meshlets;
    InstancedOffset *instance_offsets;
    float4x4 *instance_transforms;
}

[[vk::push_constant]] Bindings b;

[shader("vertex")]
VSOutput vertex(uint instance_index: SV_InstanceID, uint vertex_index : SV_VertexID)
{
    let instance_offset = b.instance_offsets[instance_index];
    var meshlet = b.meshlets[instance_offset.offset];

    if (vertex_index / 3u >= meshlet.triangle_count) { 
        return VSOutput(float4(0.0, 0.0, 0.0, 0.0), float3(0.0));
    }

    let vertex_id = b.indicies[meshlet.triangle_index + vertex_index];
    let vertex = b.verticies[meshlet.vertex_index + vertex_id];

    VSOutput output;
    let transform = b.instance_transforms[instance_offset.instance];
    let world_space = float4(vertex.position, 1.0);
    let view_space = mul(b.view, world_space);
    let clip_space = mul(b.proj, view_space);
    output.position = clip_space;
    var rand = RandomSamplerState(uint2(0, 0), instance_offset.offset);
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
    output.color = float4(input.color, 1.0);
    return output;
}