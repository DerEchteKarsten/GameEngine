
struct VSInput {
    float3 position : POSITION;   
    float3 normal : NORMAL;
    float2 uv : UV;

};

struct VSOutput {
    float4 position : SV_Position;
    float3 color; 
};

[shader("vertex")]
VSOutput vertex(VSInput input, uint vertex_id : SV_VertexID)
{
    VSOutput output;
    output.position = float4(input.position, 1.0);
    output.color = float3(1.0, 0.0, 0.0);
    return output;
}

struct PSOutput {
    float4 color : SV_Target;
};

[shader("fragment")]
PSOutput fragment(VSOutput input)
{
    PSOutput output;
    output.color = float4(input.color, 1.0);
    return output;
}