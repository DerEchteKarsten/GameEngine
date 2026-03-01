// #include "random.slang"
// #include "datatypes.slang"
// #include "bindless.slang"
struct VSOutput {
    float4 position : SV_Position;
    float4 color; 
};

struct RasterBindings {
    float4x4 view;
    float4x4 proj;
    // Buf<uint64_t> instances;
    uint instance;
    uint offset;
}

[[vk::push_constant]] RasterBindings b;

[shader("vertex")]
VSOutput vertex(uint instance_index: SV_InstanceID, uint vertex_index : SV_VertexID)
{
    // let root = b.instances.val[b.instance];
    // let ptr = root + b.offset;
    // var meshlet = ((Meshlet *)ptr)[instance_index];

    // if (vertex_index / 3u >= meshlet.triangle_count) { 
    //     return VSOutput(float4(0.0, 0.0, 0.0, 0.0), float3(0.0));
    // }

    // let vertex_id = meshlet.triangle_index[vertex_index];
    // let vertex = meshlet.vertex_index[vertex_id];

    VSOutput output;
    // let world_space = float4(vertex.position_and_uv1.xyz, 1.0);
    // let view_space = mul(b.view, world_space);
    // let clip_space = mul(b.proj, view_space);
    // output.position = clip_space;
    // var rand = RandomSamplerState(uint2(instance_index, vertex_index), 0);
    // output.color = float3(rand.uniform_float(), rand.uniform_float(), rand.uniform_float());
    output.position = float4(0.0);
    output.color = float4(1.0);
    // printf("TEst");
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