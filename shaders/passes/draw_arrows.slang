#include "random.slang"
#include "datatypes.slang"
#include "bindless.slang"
#include "shapes.slang"
struct VSOutput {
    float4 position : SV_Position;
    float4 color; 
};

struct Gizzmo {
    float4x4 transform;
    float4 color;
}

struct GizzmosBindings {
    float4x4 world_to_clip;
    Buf<Gizzmo> gizzmos;
} 

[[vk::push_constant]] GizzmosBindings b;

[shader("vertex")]
VSOutput vertex(uint instance_index: SV_InstanceID, uint vertex_index : SV_VertexID)
{
    let aabb = b.gizzmos.val[instance_index];
    let index = ArrowIndices[vertex_index];
    let vertex = ArrowVertices[index];
    VSOutput output;
    let world_space = mul(aabb.transform, vertex);
    let clip_space = mul(b.world_to_clip, world_space);
    output.position = clip_space;
    output.color = aabb.color;
    return output;
}

struct PSOutput {
    float4 color : SV_Target;
};

[shader("fragment")]
PSOutput fragment(in VSOutput input)
{
    PSOutput output;
    output.color = input.color;
    return output;
}