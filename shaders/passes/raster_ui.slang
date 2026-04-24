#include "datatypes.slang"
#include "bindless.slang"

struct VSOutput {
    float4 position : SV_Position;
    float4 color;
    float2 uv;
};

struct UIVertex {
    float2 pos;
    float2 uv;
    float4 color;
}

struct RasterMeshUiBindings {
    Buf<UIVertex> verticies;
    Tex2D<float> font_atlas;
}

[[vk::push_constant]] RasterMeshUiBindings b;

[shader("vertex")]
VSOutput vertex(uint instance_index: SV_InstanceID, uint index : SV_VertexID)
{
    let vertex = b.verticies.val[index];
    VSOutput output;
    output.position = float4(vertex.pos.xy, 0.5, 1.0);
    output.color = vertex.color;
    output.uv = vertex.uv.xy;
    return output;
}

struct PSOutput {
    float4 color : SV_Target;
};

[shader("fragment")]
PSOutput fragment(in VSOutput input)
{
    PSOutput output;
    output.color = float4(input.color.xyz, input.color.a * b.font_atlas.Sample(input.uv));
    return output;
}