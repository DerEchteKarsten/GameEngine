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
    Cluster *clusters;
}

[[vk::push_constant]] Bindings b;

[shader("vertex")]
VSOutput vertex(uint instance_index: SV_InstanceID, uint vertex_index : SV_VertexID)
{
    let cluster = b.clusters[instance_index];
    var meshlet = b.meshlets[cluster.meshlet];

    let triangle_id = vertex_index / 3u;
    if (triangle_id >= meshlet.triangle_count) { 
        return VSOutput(float4(0.0 / 0.0), float3(0.0));
    }

    let vertex_id = b.indicies[meshlet.triangle_index + vertex_index];
    let vertex = b.verticies[meshlet.vertex_index + vertex_id];

    VSOutput output;
    let world_space = float4(vertex.position, 1.0);
    let view_space = mul(b.view, world_space);
    let clip_space = mul(b.proj, view_space);
    output.position = clip_space;
    var rand = RandomSamplerState(uint2(0, 0), vertex_index);
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