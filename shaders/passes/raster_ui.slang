#include "random.slang"
#include "datatypes.slang"
#include "bindless.slang"

struct VSOutput {
    float4 position : SV_Position;
    float4 color;
    nointerpolation Image2D<float> texture;
    float2 uv;
};

struct UIVertex {
    float4 pos_and_uv;
    float4 color;
    Image2D<float4> texture_index;
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
    output.position = float4(vertex.pos_and_uv.xy, 1.0, 1.0);
    var rng = RandomSamplerState(uint2(4, 23), vertex_index);
    output.color = float4(rng.uniform_float(), rng.uniform_float(), rng.uniform_float(), 1.0);
    output.uv = vertex.pos_and_uv.zw; 
    return output;
}

struct PSOutput {
    float4 color : SV_Target;
};

[shader("fragment")]
PSOutput fragment(in VSOutput input)
{
    PSOutput output;
    output.color = input.color * input.texture.Sample(input.uv);
    return output;
}