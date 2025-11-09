use spirv_std::{glam::*, spirv};

#[repr(C)]
struct PostBindings {
    inverse_proj: Mat4,
    inverse_view: Mat4,
    window_size: Vec4,
    depth: TextureHandle2D<float>,
    color: TextureHandle2D<float4>,
    out: ImageHandle2D<float4>,
}

fn view_dir(bindings: &PostBindings, pixel_coord: UVec2) -> Vec3 {
    let pixel_center = pixel_coord.as_vec2() + Vec2::splat(0.5);
    let in_uv = pixel_center / bindings.window_size.xy();
    let d = in_uv * 2.0 - 1.0;
    let dir = in_uv * 2.0 - 1.0;
    let target = bindings.inverse_proj * Vec4::new(dir.x, dir.y, 1.0, 1.0);
    let direction = bindings.inverse_view * target.xyz().normalize().extend(0.0);
    return direction.xyz();
}


#[spirv("gl_compute")]
#[spirv(compute(threads(8, 8, 1)))]
fn main(#[spirv(global_invocation_id)] dtid: UVec2, #[spirv(push_constant)] bindings: &PostBindings) {
    let color = bindings.color.Load(dtid);
    let depth = bindings.depth.Instance[dtid];

    if depth == 0.0 {
        bindings.out.Store(dtid, view_dir(bindings, dtid).extend(1.0));
    } else {
        bindings.out.Store(dtid, color);
    }
}