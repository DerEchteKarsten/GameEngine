#include "random.slang"
#include "datatypes.slang"
#include "bindless.slang"
struct VSOutput {
    float4 position : SV_Position;
};

struct RasterOutlineBindings {
    float4x4 view;
    float4x4 proj;
    Buf<float4x4> instance_transforms;
    Buf<InstancedMeshlet> meshlets;
    Buf<uint> instance_flags; 
}

[[vk::push_constant]] RasterOutlineBindings b;

[shader("vertex")]
VSOutput vertex(uint instance_index: SV_InstanceID, uint vertex_index : SV_VertexID)
{
    let instanced_meshlet = b.meshlets.val[instance_index];
    if ((b.instance_flags.val[instanced_meshlet.instance] & OUTLINE_BIT) == 0) {
        return VSOutput(float4(0.0 / 0.0, 0.0 / 0.0, 0.0 / 0.0, 0.0 / 0.0));
    }

    let meshlet = *(instanced_meshlet.meshlet);

    if (vertex_index / 3u >= meshlet.triangle_count) {
        return VSOutput(float4(0.0 / 0.0, 0.0 / 0.0, 0.0 / 0.0, 0.0 / 0.0));
    }

    let vertex_id = meshlet.triangle_index[vertex_index];
    let vertex = meshlet.vertex_index[uint(vertex_id)];

    VSOutput output;
    let transform = b.instance_transforms.val[instanced_meshlet.instance];
    let world_space = mul(transform, float4(vertex.position_and_uv1.xyz, 1.0));
    let view_space = mul(b.view, world_space);
    let clip_space = mul(b.proj, view_space);
    output.position = clip_space;
    return output;
}

[shader("fragment")]
void fragment()
{
}