#define USE_CLOCKS
// Editor gizmos: one instanced draw per shape (boxes, spheres, arrows) of alpha-blended
// instances, each a transform and a colour from the gizmo buffer.
#include "bindless.slang"
#include "profile.slang"
#include "shapes.slang"

struct VSOutput {
    float4 position : SV_Position;
    float4 color;
};

struct Gizzmo {
    float4x4 transform;
    float4 color;
}

// Mirrors `GizzmoType`.
static const uint SHAPE_BOX = 0;
static const uint SHAPE_SPHERE = 1;

struct DrawGizzmosBindings {
    float4x4 world_to_clip;
    // Every frame slot's gizmos.
    Buf<Gizzmo> gizzmos;
    // Index of the draw's first instance in `gizzmos`.
    uint first;
    uint shape;
}

[[vk::push_constant]] DrawGizzmosBindings b;

float4 shape_vertex(uint vertex_index) {
    switch (b.shape) {
    case SHAPE_BOX:
        return BoxVertices[BoxIndices[vertex_index]];
    case SHAPE_SPHERE:
        return SphereVertices[SphereIndices[vertex_index]];
    default:
        return ArrowVertices[ArrowIndices[vertex_index]];
    }
}

[shader("vertex")]
VSOutput gizzmo_vertex(uint instance_index: SV_InstanceID, uint vertex_index : SV_VertexID)
{
    PROFILE(PROFILE_VERTEX);
    let gizzmo = b.gizzmos.val[b.first + instance_index];
    VSOutput output;
    output.position = mul(b.world_to_clip, mul(gizzmo.transform, shape_vertex(vertex_index)));
    output.color = gizzmo.color;
    return output;
}

struct PSOutput {
    float4 color : SV_Target;
};

[shader("fragment")]
PSOutput gizzmo_fragment(in VSOutput input)
{
    PSOutput output;
    output.color = input.color;
    return output;
}
