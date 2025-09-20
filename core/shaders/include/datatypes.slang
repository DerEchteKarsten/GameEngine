#pragma once

static const float BACKGROUND_DEPTH = 100000.0;

struct Instance {
    row_major float4x4 transform;
}

struct InstanceIndecies {
    uint instance_id;
    uint material_id;
    uint block_id;
    uint geometry_id;
}

struct RayPayload {
    float depth;
    float2 uv;
    uint geometryIndex;
    uint primitiveId;
};