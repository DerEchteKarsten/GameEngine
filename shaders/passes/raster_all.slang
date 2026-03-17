#include "random.slang"
#include "datatypes.slang"
#include "bindless.slang"
struct VSOutput {
    float4 position : SV_Position;
    float3 color; 
};

struct RasterAllBindings {
    float4x4 view;
    float4x4 proj;
    Buf<uint64_t> instance_roots;
    Buf<float4x4> instance_transforms;
    uint offset;
    uint instance;
}

[[vk::push_constant]] RasterAllBindings b;

[shader("vertex")]
VSOutput vertex(uint instance_index: SV_InstanceID, uint vertex_index : SV_VertexID)
{
    let ptr = b.instance_roots.val[b.instance] + uint64_t(b.offset);
    printf("ptr: %lu", ptr);
    printf("instance: %u", b.instance);
    let meshlet = ((Meshlet *)ptr)[instance_index];

    if (vertex_index / 3u >= meshlet.triangle_count) {
        return VSOutput(float4(0.0 / 0.0, 0.0 / 0.0, 0.0 / 0.0, 0.0 / 0.0), float3(0.0));
    }

    let vertex_id = meshlet.triangle_index[vertex_index];
    // if (vertex_id > meshlet.vertex_count) {
    //     return VSOutput(float4(0.0 / 0.0, 0.0 / 0.0, 0.0 / 0.0, 0.0 / 0.0), float3(0.0));
    // }
    let vertex = meshlet.vertex_index[uint(vertex_id)];

    VSOutput output;
    let transform = b.instance_transforms.val[b.instance];
    printf("transform: %lu", uint64_t(b.instance_transforms.val) +  uint64_t(b.instance * sizeof(float4x4)));
    let world_space = mul(transform, float4(vertex.position_and_uv1.xyz, 1.0));
    let view_space = mul(b.view, world_space);
    let clip_space = mul(b.proj, view_space);
    output.position = clip_space;
    let c = uint64_t(meshlet.vertex_index) + uint64_t(vertex_index * sizeof(Vertex));
    var rand = RandomSamplerState(uint2(c, c), 0);
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
    output.color = float4(input.color.xyz, 1.0);
    return output;
}