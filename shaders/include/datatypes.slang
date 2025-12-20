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

struct Vertex {
    float3 position;
    float3 normal;
    float2 uv;
};

struct InstancedOffset {
    uint instance;
    int offset;
}

struct CullData {
    AabbErrorOffset aabb;
    BoundingSphere lod_group_sphere;
}

struct DrawIndirectCommand {
    uint vertex_count;
    uint instance_count;
    uint first_vertex;
    uint first_instance;
};

struct DispatchIndirectCommand {
    uint x;
    uint y;
    uint z;
};

struct DispatchParams {
    uint node_head;
    uint node_tail;
    uint done;
    uint meshlet_count;
    DrawIndirectCommand indirect_draw;
    DispatchIndirectCommand indirect_dispatch;
}