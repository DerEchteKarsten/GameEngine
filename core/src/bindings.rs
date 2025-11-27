use glam::*;
// ===== auto-included: /home/karsten/code/GameEngine/core/../shaders/target/bindings/FragBindings.cpu.rs =====
#[derive(Clone, Copy)] pub struct CFragBindings
{ pub mesh_id : u32, pub texture : u64, } pub struct FragBindings < 'a >
{ pub mesh_id : u32, pub texture : & 'a lava :: vkobjects :: image :: Image, }
unsafe impl bytemuck :: Pod for CFragBindings {} unsafe impl bytemuck ::
Zeroable for CFragBindings {} impl lava :: command_buffer :: Binding for
CFragBindings
{
    type CpuBinding < 'a > = FragBindings < 'a > ; fn from_cpu_binding < 'a >
    (bindings : & Self :: CpuBinding < 'a >) -> Self
    {
        assert!
        (bindings.texture.usage.contains(ash :: vk :: ImageUsageFlags ::
        SAMPLED), "Field {} needs {:#?} usage flag", "texture", ash :: vk ::
        ImageUsageFlags :: SAMPLED); Self
        {
            mesh_id : bindings.mesh_id, texture :
            bindings.texture.bindless_handle.unwrap() as u64,
        }
    } fn resources < 'a >
    (bindings : & Self :: CpuBinding < 'a > , stages : ash :: vk ::
    PipelineStageFlags2) -> Vec <
    (lava :: command_buffer :: ResourceHandle, lava :: command_buffer ::
    ResourceState) >
    {
        vec!
        [(lava :: command_buffer :: ResourceHandle ::
        Image((bindings.texture.view, bindings.texture.handle)), lava ::
        command_buffer :: ResourceState
        {
            access : ash :: vk :: AccessFlags2 :: SHADER_SAMPLED_READ, stages,
            layout : ash :: vk :: ImageLayout :: SHADER_READ_ONLY_OPTIMAL,
            aspect : lava :: vkobjects :: image ::
            get_aspects(bindings.texture.format),
        }),]
    }
}

// ===== auto-included: /home/karsten/code/GameEngine/core/../shaders/target/bindings/PostBindings.cpu.rs =====
#[derive(Clone, Copy)] pub struct CPostBindings
{
    pub inverse_proj : Mat4, pub inverse_view : Mat4, pub window_size : Vec4,
    pub asv : u64, pub depth : u64, pub color : u64, pub out : u64,
} pub struct PostBindings < 'a >
{
    pub inverse_proj : Mat4, pub inverse_view : Mat4, pub window_size : Vec4,
    pub asv : & 'a lava :: vkobjects :: buffer :: Buffer < u32 > , pub depth :
    & 'a lava :: vkobjects :: image :: Image, pub color : & 'a lava ::
    vkobjects :: image :: Image, pub out : & 'a lava :: vkobjects :: image ::
    Image,
} unsafe impl bytemuck :: Pod for CPostBindings {} unsafe impl bytemuck ::
Zeroable for CPostBindings {} impl lava :: command_buffer :: Binding for
CPostBindings
{
    type CpuBinding < 'a > = PostBindings < 'a > ; fn from_cpu_binding < 'a >
    (bindings : & Self :: CpuBinding < 'a >) -> Self
    {
        assert!
        (bindings.depth.usage.contains(ash :: vk :: ImageUsageFlags ::
        SAMPLED), "Field {} needs {:#?} usage flag", "depth", ash :: vk ::
        ImageUsageFlags :: SAMPLED); assert!
        (bindings.color.usage.contains(ash :: vk :: ImageUsageFlags ::
        SAMPLED), "Field {} needs {:#?} usage flag", "color", ash :: vk ::
        ImageUsageFlags :: SAMPLED); assert!
        (bindings.out.usage.contains(ash :: vk :: ImageUsageFlags :: STORAGE),
        "Field {} needs {:#?} usage flag", "out", ash :: vk :: ImageUsageFlags
        :: STORAGE); Self
        {
            inverse_proj : bindings.inverse_proj, inverse_view :
            bindings.inverse_view, window_size : bindings.window_size, asv :
            bindings.asv.address, depth :
            bindings.depth.bindless_handle.unwrap() as u64, color :
            bindings.color.bindless_handle.unwrap() as u64, out :
            bindings.out.bindless_handle.unwrap() as u64,
        }
    } fn resources < 'a >
    (bindings : & Self :: CpuBinding < 'a > , stages : ash :: vk ::
    PipelineStageFlags2) -> Vec <
    (lava :: command_buffer :: ResourceHandle, lava :: command_buffer ::
    ResourceState) >
    {
        vec!
        [(lava :: command_buffer :: ResourceHandle ::
        Buffer(bindings.asv.handle), lava :: command_buffer :: ResourceState
        {
            access : ash :: vk :: AccessFlags2 :: SHADER_STORAGE_READ | ash ::
            vk :: AccessFlags2 :: SHADER_STORAGE_WRITE, stages, layout : ash
            :: vk :: ImageLayout :: UNDEFINED, aspect : ash :: vk ::
            ImageAspectFlags :: empty(),
        }),
        (lava :: command_buffer :: ResourceHandle ::
        Image((bindings.depth.view, bindings.depth.handle)), lava ::
        command_buffer :: ResourceState
        {
            access : ash :: vk :: AccessFlags2 :: SHADER_SAMPLED_READ, stages,
            layout : ash :: vk :: ImageLayout :: SHADER_READ_ONLY_OPTIMAL,
            aspect : lava :: vkobjects :: image ::
            get_aspects(bindings.depth.format),
        }),
        (lava :: command_buffer :: ResourceHandle ::
        Image((bindings.color.view, bindings.color.handle)), lava ::
        command_buffer :: ResourceState
        {
            access : ash :: vk :: AccessFlags2 :: SHADER_SAMPLED_READ, stages,
            layout : ash :: vk :: ImageLayout :: SHADER_READ_ONLY_OPTIMAL,
            aspect : lava :: vkobjects :: image ::
            get_aspects(bindings.color.format),
        }),
        (lava :: command_buffer :: ResourceHandle ::
        Image((bindings.out.view, bindings.out.handle)), lava ::
        command_buffer :: ResourceState
        {
            access : ash :: vk :: AccessFlags2 :: SHADER_STORAGE_READ | ash ::
            vk :: AccessFlags2 :: SHADER_STORAGE_WRITE, stages, layout : ash
            :: vk :: ImageLayout :: GENERAL, aspect : lava :: vkobjects ::
            image :: get_aspects(bindings.out.format),
        }),]
    }
}

// ===== auto-included: /home/karsten/code/GameEngine/core/../shaders/target/bindings/post.cpu.rs =====
pub struct post; impl lava :: command_buffer :: Shader for post
{
    const STAGE : ash :: vk :: PipelineStageFlags2 = ash :: vk ::
    PipelineStageFlags2 :: COMPUTE_SHADER; type GpuBinding = CPostBindings;
    const ENTRY : & 'static str = "post::post";
}

// ===== auto-included: /home/karsten/code/GameEngine/core/../shaders/target/bindings/test_frag.cpu.rs =====
pub struct test_frag; impl lava :: command_buffer :: Shader for test_frag
{
    const STAGE : ash :: vk :: PipelineStageFlags2 = ash :: vk ::
    PipelineStageFlags2 :: FRAGMENT_SHADER; type GpuBinding = CFragBindings;
    const ENTRY : & 'static str = "post::test_frag";
}

// ===== auto-included: /home/karsten/code/GameEngine/core/../shaders/target/bindings/test_vertex.cpu.rs =====
pub struct test_vertex; impl lava :: command_buffer :: Shader for test_vertex
{
    const STAGE : ash :: vk :: PipelineStageFlags2 = ash :: vk ::
    PipelineStageFlags2 :: VERTEX_SHADER; type GpuBinding = CFragBindings;
    const ENTRY : & 'static str = "post::test_vertex";
}

