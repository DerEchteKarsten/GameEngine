
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
    uint index_count;
    uint index_index;
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

struct IndirectIndexedDrawCommand {
    uint index_count;
    uint instance_count;
    uint first_index;
    int vertex_offset;
    uint first_instance;
};

struct Bindings {
    // float4x4 projection_matrix;
    // float4x4 view_matrix;
    // float3 camera_position;
    // float padding1;
    // float3 camera_direction;
    // float padding2;
    // float4x4* instance_transforms;
    // Aabb* instance_aabbs;
    // uint* instance_bvh_root_nodes;
    // void* bvh_nodes;  
    // Meshlet* meshlets;
    // Atomics* atomics;
    // uint* cluster_queue;
    Vertex* vertex_buffer;
    uint* index_buffer;
    IndirectIndexedDrawCommand* indirect_draw;
};

[[vk::push_constant]] Bindings b;

[shader("compute")]
[numthreads(8, 8, 1)]
void main() {
    b.vertex_buffer[0] = Vertex(float3(1.0, 0.0, 1.0), float3(0.0, 1.0, 0.0), float2(0.0, 0.0));
    b.vertex_buffer[1] = Vertex(float3(0.0, 1.0, 1.0), float3(0.0, 1.0, 0.0), float2(0.0, 0.0));
    b.vertex_buffer[2] = Vertex(float3(-1.0, -1.0, 1.0), float3(0.0, 1.0, 0.0), float2(0.0, 0.0));

    b.index_buffer[0] = 0;
    b.index_buffer[1] = 1;
    b.index_buffer[2] = 2;

    b.indirect_draw[0] = IndirectIndexedDrawCommand(3, 1, 0, 0, 0);
}
