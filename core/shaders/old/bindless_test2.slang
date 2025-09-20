#include "bindless.slang"
#include "random.slang"
#include "datatypes.slang"
#include "DGFDecompression.hlsl"

static const uint MS_MAX_VERTS = 64;
static const uint MS_MAX_TRIS = 64;

#define CLAMP_VERT_IDX(x__) clamp((x__), 0, MS_MAX_VERTS - 1)
#define CLAMP_VERT_IDX_UINT3(x__) (uint3(CLAMP_VERT_IDX((x__).x), CLAMP_VERT_IDX((x__).y), CLAMP_VERT_IDX((x__).z)))
#define CLAMP_TRI_IDX(x__) clamp((x__), 0, MS_MAX_TRIS - 1)
#define CLAMP_MAX_VERTS(x__) clamp((x__), 0, MS_MAX_VERTS)
#define CLAMP_MAX_TRIS(x__) clamp((x__), 0, MS_MAX_TRIS)

struct Fragment
{
    float4 color;
};

[shader("fragment")]
Fragment fragment(uint id : SV_PrimitiveID, uint meshletid, float3 color) : SV_Target
{
    var rng = RandomSamplerState(uint2(124531, 13564345), meshletid);
    Fragment out;
    out.color = float4(rng.uniform_float(), rng.uniform_float(), rng.uniform_float(), 1.0) * float4(color, 1.0); 
    return out;
}

struct MeshOutput {
    float4 position : SV_Position;
}

struct Bindings{
    Buffer<uint8_t> dgf_blocks;
    Buffer<Material> materials;
    Buffer<Instance> instances;
    Buffer<InstanceIndecies> instance_indicies;
}

struct Primitive {
    uint id : SV_PrimitiveID;
    uint geom_id;
    uint block_id;
    float3 color;
};

struct InstancePayload {
    uint instance;
    uint block;
    uint geom_id;
}

#define AS_GROUP_SIZE 64
#define MAX_PRIMITIVES 124
#define MAX_VERTICES 64

struct MeshPayload {
    InstancePayload instances[AS_GROUP_SIZE];
}

groupshared MeshPayload sPayload;

float4 NormalizePlane(float4 p)
{
    float len = length(p.xyz);
    return p / len;
}

[shader("task")]
[numthreads(64, 1, 1)]
void amp(uint dtid: SV_DispatchThreadID, uint gtid: SV_GroupThreadID) {
    let bindings = loadBindings<Bindings>();
    let gconst = loadConstants<GConst>();

    let instance_index = bindings.instance_indicies[dtid];

    sPayload.instances[gtid] = InstancePayload(instance_index.instance_id, instance_index.block_id, instance_index.geometry_id);

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

    let count = WaveActiveCountBits(true);
    DispatchMesh(count, 1, 1, sPayload);
}

#define MESH_GROUP_SIZE MS_MAX_TRIS

[shader("mesh")]
[outputtopology("triangle")]
[numthreads(MESH_GROUP_SIZE, 1, 1)]
void mesh(out indices uint3 triangles[MAX_PRIMITIVES], out primitives Primitive primitives[MAX_PRIMITIVES], out vertices MeshOutput vertices[MAX_VERTICES], in payload MeshPayload p,  uint gid : SV_GroupID, uint gtid : SV_GroupThreadID) {
    let g_Const = loadConstants<GConst>();
    let bindings = loadBindings<Bindings>();
    let gPayload = p.instances[gid];
    
    let instance = bindings.instances[gPayload.instance];

    DGFBlockInfo s = DGFInit(bindings.dgf_blocks.raw(), gPayload.block);

    SetMeshOutputCounts(CLAMP_MAX_VERTS(s.header.numVerts), CLAMP_MAX_TRIS(s.header.numTriangles));

    if (gtid < s.header.numTriangles)
    {
        triangles[CLAMP_TRI_IDX(gtid)] = CLAMP_VERT_IDX_UINT3(DGFGetTriangle_BitScan_Wave(s, gtid));
        primitives[CLAMP_TRI_IDX(gtid)] = Primitive(gPayload.block + gtid, gPayload.geom_id, gPayload.block, float3(1.0));
    }

    if (gtid < s.header.numVerts)
    {
        const float4 p = float4(DGFGetVertex(s, gtid), 1.0f);
        vertices[CLAMP_VERT_IDX(gtid)].position = mul(instance.transform * g_Const.view * g_Const.proj, p);
    }
}