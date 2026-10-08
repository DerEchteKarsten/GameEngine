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

struct InstanceBvhRoot {
    uint64_t instance;
    BvhNode *node;
}

BvhNode *get_ptr(AabbPtr aabb) {
    return (BvhNode *)(((uint64_t(asuint(aabb.center_and_offset_high.w))) << 32) | uint64_t(asuint(aabb.half_extent_and_offset_low.w)));
}

struct BvhNode {
    AabbPtr aabb_and_offsets[8];
    float errors[8];
    float4 lod_bounds[8];
    uint64_t child_counts;
    uint2 pad;
}

uint8_t get_byte(uint i, uint64_t value) {
    return uint8_t((value >> (8 * i)) & 0xFF);
}

struct Meshlet {
    Vertex* vertex_index;
    uint8_t* triangle_index;
    uint vertex_count;
    uint triangle_count;
}

struct AabbError {
    float4 center_and_error;
    float4 half_extent;
}

struct Vertex {
    float4 position_and_uv1;
    float4 normal_and_uv2;
}

#define OPAQUE_DRAW 0
#define BLENDED_DRAW 1
// The meshlets of `OUTLINE` instances, also in one of the lists above.
#define OUTLINED_DRAW 2

// A list of meshlets and the mesh draw for it, a workgroup per meshlet: workgroup (x, y) draws
// meshlet x * MESHLET_STRIDE + y, so `groups_y` is MESHLET_STRIDE and `groups_x` counts strides
// (one dimension may be capped at 65535, on Intel). Workgroups start in x order, so neighbours in
// the list, which are mostly neighbours in the scene, are drawn apart; that drew fewer hidden
// fragments and was faster than drawing the list in order.
#define MESHLET_STRIDE 64
struct MeshletDraw {
    uint groups_x;
    uint groups_y;
    uint groups_z;
    uint meshlet_count;
}

struct TraversalVariables {
    uint node_batch_read_offset;
    uint total_meshlets;
    uint node_write_offset;
    uint node_count;
    // Indexed by `OPAQUE_DRAW`, `BLENDED_DRAW` and `OUTLINED_DRAW`.
    MeshletDraw draws[3];
    uint candidate_meshlet_write_offset;
    uint meshlet_batch_read_offset;
}

struct InstanceMeshletIndex {
    uint instance;
    uint meshlet;
}

struct InstancedMeshlet {
    uint64_t instance;
    Meshlet* meshlet;
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

struct InstanceHeader {
    Meshlet *meshlets;
    CullData *cull_data;
}

#define NO_TEXTURE 0xFFFFFFFFu

struct Material {
    float4 color;
    float3 emissive;
    float metalic_factor;
    float roughness_factor;
    float normal_scale;
    float occlusion_strength;
    // Fragments with less alpha are discarded. 0 for opaque materials, whose alpha is ignored.
    // A negative cutoff blends the material over what is behind it.
    float alpha_cutoff;
    uint color_texture;
    uint metallic_roughness_texture;
    uint normal_texture;
    uint occlusion_texture;
    uint emissive_texture;
    // Keeps the size a multiple of 16 bytes, the alignment of `color` on the CPU side.
    float3 pad;
}


// `InstanceFlags` in `core/src/render/world.rs`: each instance owns INSTANCE_FLAG_BITS
// consecutive bits of a packed `uint` buffer.
#define INSTANCE_FLAG_BITS 2
#define OUTLINE_BIT 0b01
// Set when the instance's material doesn't blend.
#define OPAQUE_BIT 0b10

uint instance_flags(uint *flags, uint instance) {
    let bit = instance * INSTANCE_FLAG_BITS;
    return (flags[bit / 32] >> (bit % 32)) & ((1u << INSTANCE_FLAG_BITS) - 1);
}
