#pragma once

static const float BACKGROUND_DEPTH = 100000.0;

struct RayPayload {
    float depth;
    float2 uv;
    uint geometryIndex;
    uint primitiveId;
};

struct Aabb {
    float3 center;
    float pad1;
    float3 half_extend;
    float pad2;
}

struct AabbErrorOffset {
    float3 center;
    float error;
    float3 half_extend;
    uint offset;
}

struct BoundingSphere {
    float3 center;
    float radius;
}

struct BvhNode {
    AabbErrorOffset aabbs[8];
    BoundingSphere lod_bounds[8];
    uint8_t child_counts[8];
    uint pad[2];
}

struct Meshlet {
    uint vertex_count;
    uint vertex_index;
    uint triangle_count;
    uint triangle_index;
};

struct Atomics {
    uint vertex_stack;
    uint index_stack;
    uint cluster_stack;
}

struct Vertex {
    float3 position;
    float3 normal;
    float2 uv;
};

struct Cluster {
    uint instance;
    uint meshlet;
}