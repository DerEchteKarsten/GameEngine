use std::{
    collections::{HashMap, HashSet},
    env,
    fmt::format,
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
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

pub fn extract_push_constant(root: &Root) -> Option<Vec<Field>> {
    for p in &root.parameters {
        if p.binding.kind == "pushConstantBuffer" || p.binding.kind == "constantBuffer" {
            let elem = p.type_info.element_type.as_ref()?;
            if elem.kind == "struct" {
                return Some(elem.fields.clone());
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
        "vector" => format!(
            "{}Vec{}",
            match t
                .element_type
                .as_ref()
                .unwrap()
                .scalarType
                .as_ref()
                .unwrap()
                .as_str()
            {
                "float32" => "",
                "uint32" => "U",
                "int32" => "I",
                _ => "COMPILER_ERROR",
            },
            t.elementCount.unwrap()
        ),
        "matrix" => {
            let cc = t.columnCount.unwrap();
            let rc = t.rowCount.unwrap();
            if cc == rc {
                format!("Mat{}", cc)
            } else {
                format!("Mat{}x{}", cc, rc)
            }
        }
        "array" => {
            format!(
                "[{}; {}]",
                rust_type(t.element_type.as_ref().unwrap(), structs),
                t.elementCount.unwrap()
            )
        }
        "struct" => {
            let name = t.name.clone().unwrap_or("UnknownStruct".into());
            if !structs.contains_key(&name) {
                let mut body = String::new();
                for f in &t.fields {
                    body.push_str(
                        format!(
                            "    pub {}: {},\n",
                            &f.name,
                            &gpu_type(&f.type_info, structs)
                        )
                        .as_str(),
                    );
                }
                structs.insert(name.clone(), body);
            }

            name
        }
        "pointer" => {
            rust_type(&t.valueType.as_ref().unwrap(), structs);
            "u64".to_string()
        }
        other => format!("compile_error!(\"Unsupported Type {}\")", other),
    }
}

pub fn gpu_type(t: &TypeInfo, structs: &mut HashMap<String, String>) -> String {
    match t.kind.as_str() {
        "struct" => match t.name.as_ref().unwrap().as_str() {
            "Image" | "MutImage" | "Texture" => "BindlessHandle".into(),
            "MutBuf" | "Buf" => "u64".into(),
            _ => rust_type(t, structs),
        },
        _ => rust_type(t, structs),
    }
}

pub fn cpu_type(t: &TypeInfo, structs: &mut HashMap<String, String>) -> String {
    match t.kind.as_str() {
        "struct" => match t.name.as_ref().unwrap().as_str() {
            "MutImage" | "Image" => "StorageImageViewBinding".into(),
            "Texture" => "SampledImageViewBinding".into(),
            "MutBuf" | "Buf" => {
                let inner = cpu_type(
                    &t.fields[0].type_info.valueType.as_ref().unwrap().clone(),
                    structs,
                );
                format!("BufferSlice<{}>", inner)
            }
            _ => rust_type(t, structs),
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
    if struct_name != "MutImage"
        && struct_name != "Texture"
        && struct_name != "Image"
        && struct_name != "Buf"
        && struct_name != "MutBuf"
    {
        return None;
    }

    let access = if struct_name == "MutBuf" {
        "vk::AccessFlags2::SHADER_STORAGE_WRITE | vk::AccessFlags2::SHADER_STORAGE_READ"
    } else if struct_name == "Buf" {
        "vk::AccessFlags2::SHADER_STORAGE_READ"
    } else if struct_name == "MutImage" {
        "vk::AccessFlags2::SHADER_STORAGE_WRITE | vk::AccessFlags2::SHADER_STORAGE_READ"
    } else if struct_name == "Image" {
        "vk::AccessFlags2::SHADER_STORAGE_READ"
    } else {
        "vk::AccessFlags2::SHADER_SAMPLED_READ"
    };

    let is_image = struct_name == "MutImage" || struct_name == "Image" || struct_name == "Texture";
    
   
    let mut resource_state = format!(
        r#"
ResourceState {{
    stages,
    access: {access},
"#
    );

    if is_image {
        resource_state.push_str(&format!(
            r#"    layout: bindings.{}.prefered_layout,"#,
        field.name));
    }
    resource_state.push_str(r#"
    ..Default::default()
}"#);

    if !is_image {
        Some(format!(
            "(ResourceHandle::Buffer(bindings.{}.into()),{}),",
            field.name, resource_state
        ))
    } else {
        Some(format!(
            "(ResourceHandle::Image(bindings.{}.into()),{}),",
            field.name, resource_state
        ))
    }
}

pub fn generate_push_constant(
    name: &str,
    fields: &[Field],
    structs: &mut HashMap<String, String>,
) -> String {
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
        .map(
            |f| match f.type_info.name.as_ref().map(|e| e.as_str()).unwrap_or("") {
                "MutBuf" | "Buf" => {
                    format!("{}: bindings.{}.gpu_address() as u64,", f.name, f.name)
                }
                "Image" | "MutImage" | "Texture" => {
                    format!("{}: bindings.{}.handle,", f.name, f.name)
                }
                _ => format!("{}: bindings.{},", f.name, f.name),
            },
        )
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

pub struct {name} {{
{cpu_fields}
}}

unsafe impl bytemuck::Pod for {cname} {{}}
unsafe impl bytemuck::Zeroable for {cname} {{}}

impl Binding for {cname} {{
    type CpuBinding = {name};

    fn from_cpu_binding(bindings: &Self::CpuBinding) -> Self {{
        Self {{
            {constructors}
        }}
    }}

    fn resources(
        bindings: &Self::CpuBinding,
        stages: vk::PipelineStageFlags2,
    ) -> Vec<(ResourceHandle, ResourceState)> {{
        vec![
            {resources}
        ]
    }}
}}
"#
    )
}

pub fn generate_from_json_str(
    file_path: &str,
    file_name: &str,
    json: &str,
    structs: &mut HashMap<String, String>,
) -> String {
    let file_name = file_name.split(".").next().unwrap();
    let root: Root =
        serde_json::from_str(json).expect(&format!("Invalid Slang JSON in file {}", file_name));

    let fields = extract_push_constant(&root).expect("No pushConstantBuffer found!");
    let pc_name = format!("{file_name}Bindings");
    let mut out = String::new();
    out.push_str(&generate_push_constant(&pc_name, &fields, structs));

    let entrys = extract_entry_points(&root);
    let mut stages = entrys.iter().map(|e| e.1.as_str()).collect::<Vec<_>>();
    stages.sort();

    out.push_str(&format!("pub struct {file_name};\n"));

    if stages.as_slice() == ["fragment", "vertex"] {
        let fragment_entry = &entrys
            .iter()
            .find(|(_, stage)| stage == "fragment")
            .unwrap()
            .0;
        let vertex_entry = &entrys
            .iter()
            .find(|(_, stage)| stage == "vertex")
            .unwrap()
            .0;
        out.push_str(&format!(r#"

impl RasterPass for {file_name} {{
    type GpuBinding = C{pc_name};
}}

impl RasterVertexShaderPass for {file_name} {{
    const VERTEX: &'static str = "{vertex_entry}\0";
    const FRAGMENT: &'static str = "{fragment_entry}\0";
    const BYTES: &[u8] = include_bytes!("{file_path}");
    
    fn module_cache() -> &'static OnceLock<vk::ShaderModule> {{
        static CACHE: OnceLock<vk::ShaderModule> = OnceLock::new();
        &CACHE
    }}
    fn pipeline_cache() -> &'static Mutex<LazyCell<HashMap<RasterHash, vk::Pipeline>>> {{
        static CACHE: Mutex<LazyCell<HashMap<RasterHash, vk::Pipeline>>> = Mutex::new(LazyCell::new(|| HashMap::new()));
        &CACHE
    }}
}}
    "#));
    } else if stages.as_slice() == ["fragment", "mesh"]
        || stages.as_slice() == ["amplification", "fragment", "mesh"]
    {
        let fragment_entry = &entrys
            .iter()
            .find(|(_, stage)| stage == "fragment")
            .unwrap()
            .0;
        let mesh_entry = &entrys.iter().find(|(_, stage)| stage == "mesh").unwrap().0;
        let task_entry =
            if let Some(task) = &entrys.iter().find(|(_, stage)| stage == "amplification") {
                let task = &task.0;
                &format!("Some(\"{task}\\0\")")
            } else {
                "None"
            };
        out.push_str(&format!(r#"impl RasterPass for {file_name} {{
    type GpuBinding = C{pc_name};
}}

impl RasterMeshShaderPass for {file_name} {{
    const MESH: &'static str = "{mesh_entry}\0";
    const FRAGMENT: &'static str = "{fragment_entry}\0";
    const BYTES: &[u8] = include_bytes!("{file_path}");
    const TASK: Option<&'static str> = {task_entry};

    fn module_cache() -> &'static OnceLock<vk::ShaderModule> {{
        static CACHE: OnceLock<vk::ShaderModule> = OnceLock::new();
        &CACHE
    }}

    fn pipeline_cache() -> &'static Mutex<LazyCell<HashMap<RasterHash, vk::Pipeline>>> {{
        static CACHE: Mutex<LazyCell<HashMap<RasterHash, vk::Pipeline>>> = Mutex::new(LazyCell::new(|| HashMap::new()));
        &CACHE
    }}
}}"#));
    } else if stages.as_slice() == ["compute"] {
        let entry = &entrys
            .iter()
            .find(|(_, stage)| stage == "compute")
            .unwrap()
            .0;
        out.push_str(&format!(
            r#"

impl ComputePass for {file_name} {{
    type GpuBinding = C{pc_name};

    const ENTRY: &'static str = "{entry}\0";
    const BYTES: &[u8] = include_bytes!("{file_path}");
    fn cache() -> &'static OnceLock<vk::Pipeline> {{
        static CACHE: OnceLock<vk::Pipeline> = OnceLock::new();
        &CACHE
    }}
}}"#
        ));
    } else if stages.as_slice() == ["raygen", "closest_hit", "miss"] {
        let raygen = &entrys
            .iter()
            .find(|(_, stage)| stage == "raygen")
            .unwrap()
            .0;
        let hit = &entrys
            .iter()
            .find(|(_, stage)| stage == "closest_hit")
            .unwrap()
            .0;
        let miss = &entrys.iter().find(|(_, stage)| stage == "miss").unwrap().0;
        out.push_str(&format!(
            r#"

impl RaytracingPass for {file_name} {{
    type GpuBinding = C{pc_name};
    const RAYGEN_HASH: &'static str = "{raygen}\0";
    const HIT_HASH: &'static str = "{hit}\0";
    const MISS_HASH: &'static str = "{miss}\0";
    const BYTES: &[u8] = include_bytes!("{file_path}");

    fn cache() -> &'static OnceLock<RaytracingPipeline> {{
        static CACHE: OnceLock<RaytracingPipeline> = OnceLock::new();
        &CACHE
    }}
}}
"#
        ));
    } else {
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

    std::process::Command::new(format!("{}/../shaders/compile.sh", manifest_dir))
        .spawn()
        .unwrap();

    let shader_path = [manifest_dir, "..", "shaders", "bin"]
        .iter()
        .copied()
        .collect::<PathBuf>();

    let mut bindings = r#"
use std::collections::HashMap;
use std::sync::{OnceLock, Mutex}; 
use glam::*;
use bytemuck::{Pod, Zeroable};
use lava::command_buffer::{Binding, ResourceHandle, ResourceState, ShaderHash, RasterHash, ComputePass, RasterPass, RayTracingPass, RasterMeshShaderPass, RasterVertexShaderPass};
use lava::bindless::BindlessHandle;
use lava::buffer::slice::BufferSlice;
use std::cell::{LazyCell};
use ash::vk;
use lava::image::slice::{StorageImageViewBinding, SampledImageViewBinding};
"#.to_string();
    let mut structs = HashMap::<String, String>::new();
    for entry in WalkDir::new(&shader_path)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let p = entry.path();
        if let Some(extention) = p.extension()
            && extention == "json"
            && let Ok(json) = fs::read_to_string(p)
        {
            let file_path = p.to_str().unwrap().replace(".json", ".spv");
            let file_name = entry.file_name().to_str().unwrap();
            let file_name = file_name
                .split("_")
                .map(capitalize_first)
                .collect::<String>();
            bindings.push_str(&generate_from_json_str(
                &file_path,
                &file_name,
                &json,
                &mut structs,
            ));
        }
    }
    for (name, fields) in &structs {
        bindings.push_str(
            format!(
                r#"
#[derive(Pod, Copy, Clone, Zeroable, Debug)]
#[repr(C)]
pub struct {name} {{
{fields}}}"#,
            )
            .as_str(),
        );
    }

    fs::write(out, bindings.as_bytes()).unwrap();

    unsafe {
        std::env::set_var("VK_LAYER_PRINTF_BUFFER_SIZE", "10000");
    }
}
