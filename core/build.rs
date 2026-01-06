use std::{
    collections::{HashMap, HashSet}, env, fmt::format, fs::{self, File}, io::{self, Write}, path::{Path, PathBuf}
};


use serde::Deserialize;
use walkdir::WalkDir;

#[derive(Debug, Deserialize)]
pub struct Root {
    pub parameters: Vec<Parameter>,
    pub entryPoints: Vec<EntryPoint>,
}

#[derive(Debug, Deserialize)]
pub struct Parameter {
    pub name: String,
    pub binding: Binding,
    #[serde(rename = "type")]
    pub type_info: TypeInfo,
}

#[derive(Debug, Deserialize)]
pub struct EntryPoint {
    pub name: String,
    pub stage: String,
    pub bindings: Vec<EntryPointBinding>,
}

#[derive(Debug, Deserialize)]
pub struct EntryPointBinding {
    pub name: String,
    pub binding: Binding,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Binding {
    pub kind: String,
    #[serde(default)]
    pub index: Option<u32>,
    #[serde(default)]
    pub offset: Option<u32>,
    #[serde(default)]
    pub size: Option<u32>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TypeInfo {
    pub kind: String,

    #[serde(default)]
    pub name: Option<String>,

    #[serde(default)]
    pub fields: Vec<Field>,

    #[serde(default)]
    pub scalarType: Option<String>,

    #[serde(default)]
    pub rowCount: Option<u32>,

    #[serde(default)]
    pub columnCount: Option<u32>,

    #[serde(default)]
    pub valueType: Option<Box<TypeInfo>>,

    #[serde(default)]
    pub elementCount: Option<u32>,

    #[serde(default, rename = "elementType")]
    pub element_type: Option<Box<TypeInfo>>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Field {
    pub name: String,
    #[serde(rename = "type")]
    pub type_info: TypeInfo,
    pub binding: Binding,
}

pub fn extract_push_constant(root: &Root) -> Option<(String, Vec<Field>)> {
    for p in &root.parameters {
        if p.binding.kind == "pushConstantBuffer" || p.binding.kind == "constantBuffer"{
            let elem = p.type_info.element_type.as_ref()?;
            if elem.kind == "struct" {
                return Some((elem.name.clone().unwrap(), elem.fields.clone()));
            }
        }
    }
    None
}

pub fn extract_entry_points(root: &Root) -> Vec<(String, String)> {
    root.entryPoints
        .iter()
        .map(|ep| (ep.name.clone(), ep.stage.clone()))
        .collect()
}

pub fn rust_type(t: &TypeInfo, structs: &mut HashMap<String, String>) -> String {
    match t.kind.as_str() {
        "scalar" => match t.scalarType.as_deref() {
            Some("uint32") => "u32".into(),
            Some("int32") => "i32".into(),
            Some("float32") => "f32".into(),
            Some("uint8") => "u8".into(),
            Some("uint64") => "u64".into(),
            _ => "u32".into(),
        },
        "vector" => format!("{}Vec{}", match t.element_type.as_ref().unwrap().scalarType.as_ref().unwrap().as_str() {
            "float32" => "",
            "uint32" => "U",
            "int32" => "I",
            _ => "COMPILER_ERROR",
        },t.elementCount.unwrap()),
        "matrix" => {
            let cc = t.columnCount.unwrap();
            let rc = t.rowCount.unwrap();
            if cc == rc {
                format!("Mat{}", cc)
            }else {
                format!("Mat{}x{}", cc, rc)
            }
        }, 
        "array" => {
            format!("[{}; {}]", rust_type(t.element_type.as_ref().unwrap(), structs), t.elementCount.unwrap())
        }
        "struct" => {
            let name = t.name.clone().unwrap_or("UnknownStruct".into());
            if !structs.contains_key(&name) {
                let mut body = String::new();
                for f in &t.fields {
                    body.push_str(format!("    pub {}: {},\n", &f.name, &cpu_type(&f.type_info, structs)).as_str());
                }
                structs.insert(name.clone(), body);
            }

            name
        },
        "pointer" => "u64".to_string(),
        other => format!("compile_error!(\"Unsupported Type {}\")", other),
    }
}

pub fn gpu_type(t: &TypeInfo, structs: &mut HashMap<String, String>) -> String {
    match t.kind.as_str() {
        "struct" => match t.name.as_ref().unwrap().as_str() {
            "Image" | "MutImage" => "BindlessHandle".into(),
            "MutBuf" | "Buf"=> "u64".into(),
            _ => rust_type(t, structs)
        },
        _ => rust_type(t, structs),
    }
}

pub fn cpu_type(t: &TypeInfo, structs: &mut HashMap<String, String>) -> String {
    match t.kind.as_str() {
        "struct" => match t.name.as_ref().unwrap().as_str() {
            "MutImage" | "Image" => "&'a lava::vkobjects::image::Image".into(),
            "MutBuf" | "Buf" => {
                let inner = cpu_type(&t.fields[0].type_info.valueType.as_ref().unwrap().clone(), structs);
                format!("&'a lava::vkobjects::buffer::Buffer<{}>", inner)
            },
            _ => rust_type(t, structs)
        },
        _ => rust_type(t, structs),
    }
}

pub fn resource(field: &Field, name: &str) -> Option<String> {
    if field.type_info.kind != "struct" {
        return None;
    }
    let field_name = field.name.clone();
    let struct_name = field.type_info.name.clone().unwrap();
    if struct_name != "MutImage" && struct_name != "Image" && struct_name != "Buf" && struct_name != "MutBuf"{
        return None;
    }

    let access = if struct_name == "MutBuf" {
        "ash::vk::AccessFlags2::SHADER_STORAGE_WRITE | ash::vk::AccessFlags2::SHADER_STORAGE_READ".into()
    }else if struct_name == "Buf" {
        "ash::vk::AccessFlags2::SHADER_STORAGE_READ".into()
    }else if struct_name == "MutImage" {
        format!("bindings.{field_name}.mut_access()")
    }else {
        format!("bindings.{field_name}.const_access()")
    };

    let is_image = struct_name == "MutImage" || struct_name == "Image";
    let layout_aspect = if is_image {
format!(r#"    layout: bindings.{field_name}.prefered_layout(),
    aspect: lava::vkobjects::image::get_aspects(bindings.{field_name}.format),"#)
    }else {
format!(r#"    layout: ash::vk::ImageLayout::UNDEFINED,
aspect: ash::vk::ImageAspectFlags::empty(), 
"#)
    };

    let resource_state = format!(r#"
ResourceState {{
    stages,
    access: {access},
{layout_aspect}
}}"#);

    if !is_image {
        Some(format!("(ResourceHandle::Buffer(bindings.{}.handle),{}),", field.name, resource_state))
    } else {
        Some(format!("(ResourceHandle::Image((bindings.{}.view, bindings.{}.handle)),{}),", field.name, field.name, resource_state))
    }
}

pub fn generate_push_constant(name: &str, fields: &[Field], structs: &mut HashMap<String, String>) -> String {
    let cname = format!("C{}", name);

    let gpu_fields = fields
        .iter()
        .map(|f| format!("    pub {}: {},", f.name, gpu_type(&f.type_info, structs)))
        .collect::<Vec<_>>()
        .join("\n");

    let cpu_fields = fields
        .iter()
        .map(|f| format!("    pub {}: {},", f.name, cpu_type(&f.type_info, structs)))
        .collect::<Vec<_>>()
        .join("\n");

    let constructors = fields
        .iter()
        .map(|f| match f.type_info.name.as_ref().map(|e| e.as_str()).unwrap_or("") {
            "MutBuf" | "Buf" => format!("{}: bindings.{}.address,", f.name, f.name),
            "Image" | "MutImage" => format!("{}: bindings.{}.bindless_handle.unwrap(),", f.name, f.name),
            _ => format!("{}: bindings.{},", f.name, f.name),
        })
        .collect::<Vec<_>>()
        .join("\n");

    let resources = fields
        .iter()
        .filter_map(|f| resource(f, &cname))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        r#"
#[derive(Clone, Copy)]
#[repr(C)]
pub struct {cname} {{
{gpu_fields}
}}

pub struct {name}<'a> {{
{cpu_fields}
}}

unsafe impl bytemuck::Pod for {cname} {{}}
unsafe impl bytemuck::Zeroable for {cname} {{}}

impl lava::command_buffer::Binding for {cname} {{
    type CpuBinding<'a> = {name}<'a>;

    fn from_cpu_binding<'a>(bindings: &Self::CpuBinding<'a>) -> Self {{
        Self {{
            {constructors}
        }}
    }}

    fn resources<'a>(
        bindings: &Self::CpuBinding<'a>,
        stages: ash::vk::PipelineStageFlags2,
    ) -> Vec<(ResourceHandle, ResourceState)> {{
        vec![
            {resources}
        ]
    }}
}}
"#
    )
}

pub fn generate_from_json_str(file_path: &str, file_name: &str, json: &str, structs: &mut HashMap<String, String>) -> String {
    let file_name = file_name.split(".").next().unwrap();

    let root: Root = serde_json::from_str(json).expect(&format!("Invalid Slang JSON in file {}", file_name));

    let (pc_name, fields) = extract_push_constant(&root)
        .expect("No pushConstantBuffer found!");

    let mut out = String::new();
    out.push_str(&generate_push_constant(&pc_name, &fields, structs));

    let entrys = extract_entry_points(&root);
    let mut stages = entrys.iter().map(|e|e.1.as_str()).collect::<Vec<_>>();
    stages.sort();

    out.push_str(&format!("pub struct {file_name};\n"));

    if stages.as_slice() == ["fragment", "vertex"] {
        let fragment_entry = &entrys.iter().find(|(_,stage)| stage == "fragment").unwrap().0;
        let vertex_entry = &entrys.iter().find(|(_,stage)| stage == "vertex").unwrap().0;
        let vertex_type = "()";
        out.push_str(&format!(r#"

impl lava::command_buffer::RasterPass for {file_name} {{
    type GpuBinding = C{pc_name};
}}

impl lava::command_buffer::RasterVertexShaderPass for {file_name} {{
    const VERTEX: &'static str = "{vertex_entry}\0";
    const FRAGMENT: &'static str = "{fragment_entry}\0";
    const BYTES: &[u8] = include_bytes!("{file_path}");
    type Vertex = {vertex_type};
    fn module_cache() -> &'static OnceLock<ash::vk::ShaderModule> {{
        static CACHE: OnceLock<ash::vk::ShaderModule> = OnceLock::new();
        &CACHE
    }}
    fn pipeline_cache() -> &'static Mutex<LazyCell<HashMap<RasterHash, ash::vk::Pipeline>>> {{
        static CACHE: Mutex<LazyCell<HashMap<RasterHash, ash::vk::Pipeline>>> = Mutex::new(LazyCell::new(|| HashMap::new()));
        &CACHE
    }}
}}
    "#));
    } else if stages.as_slice() == ["fragment", "mesh"] || stages.as_slice() == [ "amplification", "fragment", "mesh"] {
        let fragment_entry = &entrys.iter().find(|(_,stage)| stage == "fragment").unwrap().0;
        let mesh_entry = &entrys.iter().find(|(_,stage)| stage == "mesh").unwrap().0;
        let task_entry = if let Some(task) = &entrys.iter().find(|(_,stage)| stage == "amplification"){
            let task = &task.0;
            &format!("Some(\"{task}\\0\")")
        }else {
            "None"
        };
out.push_str(&format!(r#"impl lava::command_buffer::RasterPass for {file_name} {{
    type GpuBinding = C{pc_name};
}}

impl lava::command_buffer::RasterMeshShaderPass for {file_name} {{
    const MESH: &'static str = "{mesh_entry}\0";
    const FRAGMENT: &'static str = "{fragment_entry}\0";
    const BYTES: &[u8] = include_bytes!("{file_path}");
    const TASK: Option<&'static str> = {task_entry};

    fn module_cache() -> &'static OnceLock<ash::vk::ShaderModule> {{
        static CACHE: OnceLock<ash::vk::ShaderModule> = OnceLock::new();
        &CACHE
    }}

    fn pipeline_cache() -> &'static Mutex<LazyCell<HashMap<RasterHash, ash::vk::Pipeline>>> {{
        static CACHE: Mutex<LazyCell<HashMap<RasterHash, ash::vk::Pipeline>>> = Mutex::new(LazyCell::new(|| HashMap::new()));
        &CACHE
    }}
}}"#));
    }else if stages.as_slice() == ["compute"] {
        let entry = &entrys.iter().find(|(_,stage)| stage == "compute").unwrap().0;
out.push_str(&format!(r#"

impl lava::command_buffer::ComputePass for {file_name} {{
    type GpuBinding = C{pc_name};

    const ENTRY: &'static str = "{entry}\0";
    const BYTES: &[u8] = include_bytes!("{file_path}");
    fn cache() -> &'static OnceLock<ash::vk::Pipeline> {{
        static CACHE: OnceLock<ash::vk::Pipeline> = OnceLock::new();
        &CACHE
    }}
}}"#));
    }else if stages.as_slice() == ["raygen", "closest_hit", "miss"] {
        let raygen = &entrys.iter().find(|(_,stage)| stage == "raygen").unwrap().0;
        let hit = &entrys.iter().find(|(_,stage)| stage == "closest_hit").unwrap().0;
        let miss = &entrys.iter().find(|(_,stage)| stage == "miss").unwrap().0;
out.push_str(&format!(r#"

impl lava::command_buffer::RaytracingPass for {file_name} {{
    type GpuBinding = C{pc_name};
    const RAYGEN_HASH: &'static str = "{raygen}\0";
    const HIT_HASH: &'static str = "{hit}\0";
    const MISS_HASH: &'static str = "{miss}\0";
    const BYTES: &[u8] = include_bytes!("{file_path}");

    fn cache() -> &'static OnceLock<lava::vkobjects::rt_pipeline::RaytracingPipeline> {{
        static CACHE: OnceLock<lava::vkobjects::rt_pipeline::RaytracingPipeline> = OnceLock::new();
        &CACHE
    }}
}}
"#));
    }else {
        panic!("Entrys in file {file_name} didnt match any pass pattern.")
    }


    out
}

fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();

    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

fn main() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    println!("cargo:rerun-if-changed={manifest_dir}/../shaders/");
    
    let out = PathBuf::from("src/bindings.rs");

    std::process::Command::new(format!("{}/../shaders/compile.sh", manifest_dir)).spawn().unwrap();

    let shader_path = [manifest_dir, "..", "shaders", "bin"]
        .iter()
        .copied()
        .collect::<PathBuf>();

    let mut bindings = "use std::collections::HashMap;\nuse std::sync::{OnceLock, Mutex}; use glam::*;\nuse bytemuck::{Pod, Zeroable};\nuse lava::command_buffer::{ResourceHandle, ResourceState, ShaderHash, RasterHash};\nuse lava::bindless::BindlessHandle;\nuse std::cell::{LazyCell};".to_string();
    let mut structs = HashMap::<String, String>::new();
    for entry in WalkDir::new(&shader_path).into_iter().filter_map(|e| e.ok()) {
        let p = entry.path();
        if let Some(extention) = p.extension() && extention == "json" && let Ok(json) = fs::read_to_string(p){
            let file_path = p.to_str().unwrap().replace(".json", ".spv");
            let file_name = entry.file_name().to_str().unwrap();
            let file_name = file_name.split("_").map(capitalize_first).collect::<String>();
            bindings.push_str(&generate_from_json_str(&file_path, &file_name, &json, &mut structs));
        }
    }
    for (name, fields) in &structs {
        bindings.push_str(format!(r#"
#[derive(Pod, Copy, Clone, Zeroable, Debug)]
#[repr(C)]
pub struct {name} {{
{fields}}}"#,).as_str());
    }

    fs::write(out, bindings.as_bytes()).unwrap();

    unsafe {
        std::env::set_var("VK_LAYER_PRINTF_BUFFER_SIZE", "10000");
    }
}
