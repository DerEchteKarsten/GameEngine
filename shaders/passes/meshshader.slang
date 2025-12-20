#include "random.slang"
#include "datatypes.slang"
#include "bindless.slang"

struct Fragment
{
    float4 color;
};

[shader("fragment")]
Fragment mesh_fragment(uint id : SV_PrimitiveID, uint meshletid, float3 color) : SV_Target
{
    var rng = RandomSamplerState(uint2(124531, 13564345), meshletid);
    Fragment out;
    out.color = float4(rng.uniform_float(), rng.uniform_float(), rng.uniform_float(), 1.0) * float4(color, 1.0);
    return out;
}

struct MeshOutput {
    float4 position : SV_Position;
}


struct MeshShaderBindings {
    column_major float4x4 proj;
    column_major float4x4 view;
    column_major float4x4 model;
    Buf<Vertex> vertices;
    Buf<uint8_t> indecies;
    Buf<Meshlet> meshlets;
}

struct Primitive {
    uint id : SV_PrimitiveID;
    uint meshletid;
    float3 color;
};


#define AS_GROUP_SIZE 64
#define MAX_PRIMITIVES 128
#define MAX_VERTICES 256

// struct MeshPayload {
//     Cluster instances[AS_GROUP_SIZE];
// }

// groupshared MeshPayload sPayload;
[[vk::push_constant]] MeshShaderBindings b;

[shader("task")]
[numthreads(64, 1, 1)]
void amp(uint dtid: SV_DispatchThreadID, uint gtid: SV_GroupThreadID) {
    // if (dtid < b.const.num_clusters) {
    //     sPayload.instances[gtid] = b.clusters[dtid];
    // }


    // let meshlet = bindings.meshlets[instance_index.meshlet];
    // let instance = bindings.instances[instance_index.instance];
    
    // float3x3 worldLinear = (float3x3)instance.transform;
    // float3 basisLen = float3(length(worldLinear[0]), length(worldLinear[1]), length(worldLinear[2]));
    // float sphereRadiusWS = meshlet.radius * max(basisLen.x, max(basisLen.y, basisLen.z));

    // let vp = mul(gconst.proj, gconst.view);
    // float4 planes[6];
    // planes[0] = vp[3] + vp[0];
    // planes[1] = vp[3] - vp[0];
    // planes[2] = vp[3] + vp[1];
    // planes[3] = vp[3] - vp[1];
    // planes[4] = vp[3] + vp[2];
    // planes[5] = vp[3] - vp[2];

    // [unroll] for (uint i = 0; i < 6; ++i)
    //     planes[i] = NormalizePlane(planes[i]);

    // bool visible = true;
    // [unroll] 
    // for (uint i = 0; i < 6; ++i)
    // {
    //     float dist = distance_to_plane(mul(float4(meshlet.center, 1.0), instance.transform).xyz, planes[i].xyz, planes[i].w);
    //     if(dist < -sphereRadiusWS) {
    //         visible = false;
    //         break;
    //     }
    // }
   
    // if(instance_index.instance == 0 && instance_index.meshlet == 10) {
    //     printf("%f %f\n", meshlet.radius, sphereRadiusWS);
    // }

    // let count = WaveActiveCountBits(dtid < b.const.num_clusters);
    // DispatchMesh(count, 1, 1, sPayload);
}

#define MESH_GROUP_SIZE MAX_PRIMITIVES

// in payload MeshPayload p,

[shader("mesh")]
[outputtopology("triangle")]
[numthreads(MESH_GROUP_SIZE, 1, 1)]
void mesh( out indices uint3 triangles[MAX_PRIMITIVES], out primitives Primitive primitives[MAX_PRIMITIVES], out vertices MeshOutput vertices[MAX_VERTICES], uint gtid: SV_GroupThreadID, uint3 gid: SV_GroupID, uint3 dtid: SV_DispatchThreadID) {
    // let n = b.const.cluster_dispatch;
    let gid_liniar = gid.x; // gid.z * n * n + gid.y * n + gid.x;
    let dtid_liniar = dtid.x;//gid_liniar * MESH_GROUP_SIZE + gtid;

    // if (gid_liniar > b.const.num_clusters) {
    //     SetMeshOutputCounts(0, 0);
    //     return;
    // }

    // let cluster = b.clusters[gid_liniar];

    // let transform = b.cluster_transforms[cluster.transform];
    let meshlet = b.meshlets.val[gid_liniar];

    SetMeshOutputCounts(meshlet.vertex_count, meshlet.triangle_count);

    if (gtid < meshlet.triangle_count)
    {
        triangles[gtid] = uint3((uint32_t)b.indecies.val[meshlet.triangle_index + uint32_t(gtid * 3 + 0)],
                               (uint32_t)b.indecies.val[meshlet.triangle_index + uint32_t(gtid * 3 + 1)],
                               (uint32_t)b.indecies.val[meshlet.triangle_index + uint32_t(gtid * 3 + 2)]);
        primitives[gtid] = Primitive(dtid_liniar, gid_liniar, float3(1.0));
    }

    for (int i = 0; i < 2; i++) {
        if (gtid*i < meshlet.vertex_count)
        {
            let position = b.vertices.val[meshlet.vertex_index + gtid * i].position;
            let world_space = mul(b.model, float4(position, 1.0f));
            let view_space = mul(b.view, world_space);
            let clip_space = mul(b.proj, view_space);
            vertices[gtid * i].position = clip_space;
        }
    }
}