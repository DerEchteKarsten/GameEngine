#include "random.slang"
#include "datatypes.slang"
#include "bindless.slang"

struct VSOutput {
    float4 position : SV_Position;
    float3 color; 
};

struct UIVertex {
    float2 pos;
    float2 uv;
    uint color;
    uint texture_index;
    uint2 pad;
}

struct RasterMeshUiBindings {
    Buf<UIVertex> verticies;
}

[[vk::push_constant]] RasterMeshUiBindings b;

[shader("vertex")]
VSOutput vertex(uint instance_index: SV_InstanceID, uint vertex_index : SV_VertexID)
{
    let vertex = b.verticies.val[vertex_index];
    VSOutput output;
    output.position = float4(vertex.pos, 1.0, 0.0);
    var rand = RandomSamplerState(uint2(0, 0), instance_index);
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