#include "bindless.slang"
#include "datatypes.slang"

struct Block {
    uint64_t data[8];
}

struct Bindings{
    Buffer<Block> world;
    Image4 out;
}

float3 view_dir(uint2 pixelCoord, GConst view) {
    const var pixelCenter = float2(pixelCoord) + float2(0.5);
    const var inUV = pixelCenter / float2(view.window_size);
    var d = inUV * 2.0 - 1.0;
    var dir = inUV * 2.0 - 1.0;
    var target = mul(view.proj_inverse, float4(dir.x, dir.y, 1, 1));
    var direction = mul(view.view_inverse, float4(normalize(target.xyz), 0));
    return direction.xyz;
}


float intersectRayAABB(float3 rayOrigin, float3 rayDir, float3 boxMin, float3 boxMax)
{
    float3 invDir = 1.0 / rayDir;

    float3 t0 = (boxMin - rayOrigin) * invDir;
    float3 t1 = (boxMax - rayOrigin) * invDir;

    float3 tMin3 = min(t0, t1);
    float3 tMax3 = max(t0, t1);

    float tNear = max(max(tMin3.x, tMin3.y), tMin3.z);
    float tFar = min(min(tMax3.x, tMax3.y), tMax3.z);
    bool inside = all(rayOrigin >= boxMin) && all(rayOrigin <= boxMax);
    if (inside) return 0.0;

    return (tFar >= max(tNear, 0.0)) ? tNear : -1.0;
}

float dda(uint block_index, RWByteAddressBuffer world, RayDesc ray) {
    Block s = world.LoadAligned<Block>(block_index, 64);

    var t = intersectRayAABB(ray.Origin, ray.Direction, float3(0.0), float3(8.0));
    if (t < 0.0) {
        return ray.TMax;
    }
    let origin = ray.Origin;
    ray.Origin += ray.Direction * (t + 0.001);
    let flat_pos = floor(ray.Origin);
    var mapPos = int3(flat_pos);
    let deltaDist = abs(rcp(ray.Direction));
    let rayStep = int3(sign(ray.Direction));
    var sideDist = (rayStep * (.5 - fract(ray.Origin)) + .5) * deltaDist;

    bool3 mask;
    for (int i = 0; i < 18; i++) {
        if (any(mapPos < 0) || any(mapPos >= 8)) {
            return ray.TMax;
        }

        uint i64 = mapPos.x + (mapPos.y << 3);
        uint64_t slice =
            (mapPos.z & uint64_t(1)) ? ((mapPos.z & uint64_t(2)) ? ((mapPos.z & uint64_t(4)) ? s.data[7] : s.data[6])
                                                                 : ((mapPos.z & uint64_t(4)) ? s.data[5] : s.data[4]))
                                     : ((mapPos.z & uint64_t(2)) ? ((mapPos.z & uint64_t(4)) ? s.data[3] : s.data[2])
                                                                 : ((mapPos.z & uint64_t(4)) ? s.data[1] : s.data[0]));
    
        if ((slice & uint64_t(i64)) > uint64_t(0)) {
            return distance(float3(mapPos), origin);
        }

        mask = step(sideDist.xyz, min(sideDist.yzx, sideDist.zxy));
        sideDist += mask * deltaDist;
        mapPos += mask * rayStep;
    }
    return ray.TMax;
}

[shader("compute")]
[numthreads(8, 8, 1)]
void main(uint3 threadId: SV_DispatchThreadID) {
    var bindings = loadBindings<Bindings>();
    let gconst = loadConstants<GConst>();

    var pixelCoord = threadId.xy;
    var ray = RayDesc(
        gconst.camera_pos,
        0.0,
        view_dir(pixelCoord, gconst),
        1000.0,
    );

    let t = dda(0, bindings.world.raw(), ray);
    
    var color = float3(t);
    if(t >= ray.TMax-1.0) {
        color = ray.Direction;
    } else {
        color = float3(exp(-float3(t) * 0.02));
    }


    bindings.out[pixelCoord] = float4(color, 1.0);
}