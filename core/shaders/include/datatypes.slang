#pragma once

static const float BACKGROUND_DEPTH = 100000.0;

struct RayPayload {
    float depth;
    float2 uv;
    uint geometryIndex;
    uint primitiveId;
};