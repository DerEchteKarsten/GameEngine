#include "spherical_harmonics.slang"
#include "packing.slang"
#include "math.slang"
#include "random.slang"

[[vk::binding(0, 1)]] RWTexture2D<float4> probe_atlas;
[[vk::binding(0, 0)]] RWStructuredBuffer<float3x3> out;

// Solid angle of the spherical triangle with unit corners a, b, c (Van Oosterom and Strackee).
float spherical_triangle_area(float3 a, float3 b, float3 c) {
    return 2.0 * atan2(abs(dot(a, cross(b, c))), 1.0 + dot(a, b) + dot(b, c) + dot(c, a));
}

[shader("compute")]
[numthreads(8,8,1)]
void main(uint3 threadId : SV_DispatchThreadID, uint2 local_thread_id : SV_GroupThreadID, uint2 work_group_index : SV_GroupID) {
    var direction_index = local_thread_id.y * 8 + local_thread_id.x;
    var direction_2d = float2(uint(direction_index % 8), direction_index / 8);
    float3 local_ray_direction = octa_decode((direction_2d + 0.5) / 8.0);
    // The octahedral map isn't equal-area: each texel weighs the solid angle of its cell.
    float3 c00 = octa_decode(direction_2d / 8.0);
    float3 c10 = octa_decode((direction_2d + float2(1, 0)) / 8.0);
    float3 c01 = octa_decode((direction_2d + float2(0, 1)) / 8.0);
    float3 c11 = octa_decode((direction_2d + float2(1, 1)) / 8.0);
    float solid_angle = spherical_triangle_area(c00, c10, c11) + spherical_triangle_area(c00, c11, c01);

    float3 ray_color = probe_atlas[threadId.xy].xyz * solid_angle;

    sh3 sh = sh3Evaluate(local_ray_direction);

    sh3 shR = WaveActiveSum(sh3Scale(sh, ray_color.x));
    sh3 shG = WaveActiveSum(sh3Scale(sh, ray_color.y));
    sh3 shB = WaveActiveSum(sh3Scale(sh, ray_color.z));

    if(WaveIsFirstLane()) {

        out[ZCurveToLinearIndex(uint2(work_group_index.x * 3 + 0, work_group_index.y))] = shR;
        out[ZCurveToLinearIndex(uint2(work_group_index.x * 3 + 1, work_group_index.y))] = shG;
        out[ZCurveToLinearIndex(uint2(work_group_index.x * 3 + 2, work_group_index.y))] = shB;
    }
}