#include "random.slang"
#include "datatypes.slang"
#include "bindless.slang"

struct VSOutput {
    float4 position : SV_Position;
    float4 color;
    float2 uv;
    nointerpolation uint2 texture;
};

struct UIVertex {
    float4 pos_and_uv;
    float4 color;
    uint2 texture_index;
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
    output.position = float4(vertex.pos_and_uv.xy, 0.5, 1.0);
    output.color = vertex.color;
    output.uv = vertex.pos_and_uv.zw;
    output.texture = vertex.texture_index;
    return output;
}

struct PSOutput {
    float4 color : SV_Target;
};

[shader("fragment")]
PSOutput fragment(in VSOutput input)
{
    Image<Texture2D<float4>> texture = { input.texture.x, input.texture.y };
    PSOutput output;
    output.color = texture.Sample(input.uv) * input.color;
    return output;
}