#pragma once

static const float BACKGROUND_DEPTH = 100000.0;

struct Material {
	float16_t metalic_factor;
    float16_t roughness_factor;
    float16_t color[3];
    uint16_t texture_index;
};

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