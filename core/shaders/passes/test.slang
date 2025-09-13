#include "bindless.slang"
#include "random.slang"
#include "datatypes.slang"
// #include "DGFDecompression.hlsl"

struct Fragment
{
    float4 color: SV_TARGET0;
};

[shader("fragment")]
Fragment frag(uint id : SV_PrimitiveID, uint meshletid, float3 color)
{
    // var rng = RandomSamplerState(uint2(124531, 13564345), meshletid);
    Fragment out;
    printf("test");

    out.color = float4(1.0, 0.0, 0.0, 1.0);//rng.uniform_float(), rng.uniform_float(), rng.uniform_float(), 1.0);// * float4(color, 1.0); 
    return out;
}

struct VertOutput {
    float4 position : SV_Position;
    uint meshletid;
    float3 color : COLOR0;
}

struct Bindings{
    Buffer<uint8_t> dgf_blocks;
    Buffer<Material> materials;
    Buffer<Instance> instances;
    Buffer<InstanceIndecies> instance_indicies;
}

static float3[] positions = {
    float3(-0.1, 0.1, 0.0),
    float3(0.1, -0.1, 0.0),
    float3(0.1, 0.1, 0.0),
};

[shader("vertex")]
VertOutput vert(uint vertex_id: SV_VertexID) {
    let g_Const = loadConstants<GConst>();
    let bindings = loadBindings<Bindings>();

    return VertOutput(
        float4(positions[vertex_id], 1.0),
        0,
        float3(1.0, 0.0, 1.0)
    );
}