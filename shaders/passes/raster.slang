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
    Buf<Vertex> verticies;
    Buf<uint8_t> indicies;
    Buf<Meshlet> meshlets;
    Buf<InstancedOffset> instance_offsets;
    Buf<float4x4> instance_transforms;
}

[[vk::push_constant]] RasterBindings b;

[shader("vertex")]
VSOutput vertex(uint instance_index: SV_InstanceID, uint vertex_index : SV_VertexID)
{
    let instance_offset = b.instance_offsets.val[instance_index];
    var meshlet = b.meshlets.val[instance_offset.offset];

    if (vertex_index / 3u >= meshlet.triangle_count) { 
        return VSOutput(float4(0.0, 0.0, 0.0, 0.0), float3(0.0));
    }

    let vertex_id = b.indicies.val[meshlet.triangle_index + vertex_index];
    let vertex = b.verticies.val[meshlet.vertex_index + vertex_id];

    VSOutput output;
    let transform = b.instance_transforms.val[instance_offset.instance];
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