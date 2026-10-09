#define USE_CLOCKS
#include "datatypes.slang"
#include "bindless.slang"
#include "profile.slang"

struct VSOutput {
    float4 position : SV_Position;
    float4 color;
    float2 uv;
    nointerpolation uint2 texture;
};

struct UIVertex {
    float2 pos;
    float2 uv;
    float4 color;
    DynImg texture;
    uint2 pad;
}

// Made once with the UI resources.
SCONST(Tex2D<float>, font_atlas)

struct RasterMeshUiBindings {
    Buf<UIVertex> verticies;
}

[[vk::push_constant]] RasterMeshUiBindings b;

[shader("vertex")]
VSOutput raster_ui_vertex(uint instance_index: SV_InstanceID, uint index : SV_VertexID)
{
    PROFILE(PROFILE_VERTEX);
    let vertex = b.verticies.val[index];
    VSOutput output;
    output.position = float4(vertex.pos.xy, 0.5, 1.0);
    output.color = vertex.color;
    output.uv = vertex.uv.xy;
    output.texture = uint2(vertex.texture.SampledIndex, vertex.texture.StorageIndex);
    return output;
}

struct PSOutput {
    float4 color : SV_Target;
};

[shader("fragment")]
PSOutput raster_ui_fragment(in VSOutput input)
{
    PSOutput output;
    DynImg texture;
    texture.SampledIndex = input.texture.x;
    texture.StorageIndex = input.texture.y;
    if (texture.IsNull()) {
        output.color = float4(input.color.xyz, input.color.a * font_atlas.Sample(input.uv));
    } else {
        output.color = float4(texture.Sample(input.uv, 2).xyz * input.color.xyz, input.color.a);
    }
    return output;
}
