#pragma once

static const float BACKGROUND_DEPTH = 100000.0;

struct RayPayload {
    float depth;
    float2 uv;
    uint geometryIndex;
    uint primitiveId;
}

struct AabbPtr {
    float4 center_and_offset_high;
    float4 half_extent_and_offset_low;
}

BvhNode *get_ptr(AabbPtr aabb) {
    return (__asuint64(uint2(asuint(aabb.center_and_offset_high.w), asuint(aabb.half_extent_and_offset_low.w))) as BvhNode*).value; 
}

struct BvhNode {
    AabbPtr aabb_and_offsets[8];
    float errors[8];
    float4 lod_bounds[8];
    uint64_t child_counts;
    uint64_t pad;
}

uint8_t get_byte(uint i, uint64_t value) {
    return uint8_t((value >> (8 * i)) & 0xFF);
}

struct Meshlet {
    Vertex* vertex_index;
    uint8_t* triangle_index;
    uint vertex_count;
    uint triangle_count;
    uint2 pad;
}

struct AabbError {
    float4 center_and_error;
    float4 half_extent;
}

struct Vertex {
    float4 position_and_uv1;
    float4 normal_and_uv2;
}

struct InstancedOffset {
    uint instance;
    int offset;
}

struct CullData {
    AabbError aabb;
    float4 lod_group_sphere;
}

struct DrawIndirectCommand {
    uint vertex_count;
    uint instance_count;
    uint first_vertex;
    uint first_instance;
}

struct DispatchIndirectCommand {
    uint x;
    uint y;
    uint z;
}

struct DispatchParams {
    uint node_head;
    uint node_tail;
    uint done;
    uint meshlet_count;
    DrawIndirectCommand indirect_draw;
    DispatchIndirectCommand indirect_dispatch;
}