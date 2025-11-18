use std::{fs, path::PathBuf};

use bytemuck::Pod;
use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{Attribute, DeriveInput, Expr, Type, TypePtr, TypeTuple, parse_macro_input};


#[proc_macro_attribute]
pub fn shader(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let func = parse_macro_input!(item as syn::ItemFn);
    let f = func.clone();
    let func_name = func.sig.ident;

    let stage = func.attrs
        .iter()
        .find(|e| e.path().is_ident("spirv"))
        .expect("Function does not have a spirv attribute")
        .parse_args::<proc_macro2::TokenStream>()
        .unwrap()
        .to_string();

    let stage = match stage {
        s if s.contains("compute") => quote!(ash::vk::PipelineStageFlags2::COMPUTE_SHADER),
        s if s.contains("vertex") => quote!(ash::vk::PipelineStageFlags2::VERTEX_SHADER),
        s if s.contains("fragment") => quote!(ash::vk::PipelineStageFlags2::FRAGMENT_SHADER),
        s if s.contains("ray_generation") => quote!(ash::vk::PipelineStageFlags2::RAY_TRACING_SHADER_KHR),
        _ => panic!("Shader Stage not suported (yet)")
    };

    let binding_type = func
        .sig
        .inputs
        .iter()
        .filter_map(|arg| if let syn::FnArg::Typed(pat_type) = arg { Some(pat_type) } else {None})
        .find(|arg| {
            let Some(attrib) = arg.attrs.iter().find(|e| e.path().is_ident("spirv")) else { return false;  };
            attrib.parse_args::<proc_macro2::TokenStream>().unwrap().to_string().contains("push_constant")
        }).map(|e| {
            let ty = e.ty.as_ref().clone();
            if let Type::Reference(ty) = ty
                && let Type::Path(ty) = ty.elem.as_ref().clone() {
                let path = ty.path;
                quote!(shaders::#path)
            }else {
                quote!(compile_error!("Push Constant Type needs to be a struct"))
            }
        })
        .unwrap_or(quote!(()));

    let func_name_string = func_name.to_string();
    let expandet = quote! {
        struct #func_name;

        impl Shader for #func_name {
            const STAGE: ash::vk::PipelineStageFlags2 = #stage;
            type GpuBinding = #binding_type;
            const ENTRY: &'static str = #func_name_string;
        }
    };

    let out_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let mut cpu_file = PathBuf::from(out_dir);
    cpu_file.push("target");
    cpu_file.push("bindings");
    if !fs::exists(&cpu_file).unwrap() {
        fs::create_dir(&cpu_file).unwrap();
    }
    cpu_file.push(format!("{}.cpu.rs", func_name_string));

    fs::write(&cpu_file, expandet.to_string()).expect("Could not write CPU bindings");

    quote!(#f).into()
}


#[proc_macro_derive(Binding)]
pub fn bindings(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let struct_name = &input.ident;
    let cpu_name = format_ident!("C{}", struct_name.to_string());

    let fields = match &input.data {
        syn::Data::Struct(str) => &str.fields,
        _ => return quote!(compile_error!("Texture can not be written to but still hast write_only tag")).into(), 
    };

    let mut cpu_fields = Vec::new();
    let mut field_constructors = Vec::new(); 
    let mut states = Vec::new();
    let mut checks = Vec::new();
    for field in fields {
        let name = field.ident.as_ref().unwrap();
        let mut writeable = field.attrs.iter().any(|e| e.path().is_ident("write_only"));
        let readable = !writeable;
        let mut pc = false;
        let (cpu_type, image) = match &field.ty {
            Type::Ptr(ty) => {
                if ty.mutability.is_some() {
                    writeable = true;
                }
                let el = ty.elem.clone();
                (quote!(&'a lava::vkobjects::buffer::Buffer<#el>), false)
            },
            Type::Path(ty) => {
                let name = ty.path.segments.last().unwrap().ident.to_string();
                let image = name.as_str() == "ImageHandle";
                writeable = image;
                (if name.as_str() == "TextureHandle" || image {
                    quote!(&'a lava::vkobjects::image::Image)  
                } else { 
                    pc = true;
                    quote!(#ty)
                }, true)
            }
            _ => (quote!(compile_error!("Unsuported Type")), false)
        };
        cpu_fields.push(quote! {
            pub #name: #cpu_type
        });
        if pc {
            field_constructors.push(quote! {
                #name: bindings.#name
            });
            continue;
        }
     

        states.push(match &field.ty {
            Type::Ptr(ty) => {
                let el = ty.elem.clone();
                let access = if readable && writeable {
                    quote!(ash::vk::AccessFlags2::SHADER_STORAGE_READ | ash::vk::AccessFlags2::SHADER_STORAGE_WRITE)
                }else if writeable {
                    quote!(ash::vk::AccessFlags2::SHADER_STORAGE_WRITE)
                }else {
                    field_constructors.push(quote! {
                        #name: bindings.#name.address as usize as *const #el
                    });
                    quote!(ash::vk::AccessFlags2::SHADER_STORAGE_READ)
                };
                if writeable {
                    field_constructors.push(quote! {
                        #name: bindings.#name.address as usize as *mut #el
                    });
                }
                quote!(
                    (
                        lava::command_buffer::ResourceHandle::Buffer(bindings.#name.handle),
                        lava::command_buffer::ResourceState {
                            access: #access,
                            stages,
                            layout: ash::vk::ImageLayout::UNDEFINED,
                            aspect: ash::vk::ImageAspectFlags::empty(),
                        }
                    )
                )
            },
            Type::Path(ty) => {
                let type_name = ty.path.segments.last().unwrap().ident.to_string();
                let (layout, access, usage) = match type_name.as_str() {
                    "TextureHandle" => {
                        field_constructors.push(quote! {
                            #name: lava::command_buffer::TextureHandle {index: bindings.#name.bindless_handle.unwrap() as u64}
                        });
                        (
                            quote!(ash::vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL),
                            quote!(ash::vk::AccessFlags2::SHADER_SAMPLED_READ),
                            quote!(ash::vk::ImageUsageFlags::SAMPLED)
                        )
                    },
                    "ImageHandle" => {
                        field_constructors.push(quote! {
                            #name: lava::command_buffer::ImageHandle {index: bindings.#name.bindless_handle.unwrap() as u64}
                        });
                        (
                            quote!(ash::vk::ImageLayout::GENERAL),
                            if readable && writeable {
                                quote!(ash::vk::AccessFlags2::SHADER_STORAGE_READ | ash::vk::AccessFlags2::SHADER_STORAGE_WRITE)
                            }else if writeable {
                                quote!(ash::vk::AccessFlags2::SHADER_STORAGE_WRITE)
                            }else {
                                quote!(ash::vk::AccessFlags2::SHADER_STORAGE_READ)
                            },
                            quote!(ash::vk::ImageUsageFlags::STORAGE)
                        )
                    },
                    _ => unreachable!()
                };
                checks.push(quote! {
                    assert!(bindings.#name.usage.contains(#usage), "Field needs {:#?} usage flag", #usage);
                });
                quote!(
                    (
                        lava::command_buffer::ResourceHandle::Image((bindings.#name.view, bindings.#name.handle)),
                        lava::command_buffer::ResourceState {
                            access: #access,
                            stages,
                            layout: #layout,
                            aspect: lava::vkobjects::image::get_aspects(bindings.#name.format),
                        }
                    )
                )
            },
            _ => quote!(compile_error!("Unsupported Type"))
        });
    }


    let expandet = quote! {    
        #input
            
        pub struct #cpu_name <'a> {
            #(#cpu_fields,)*
        }

        unsafe impl bytemuck::Pod for #struct_name {}
        unsafe impl bytemuck::Zeroable for #struct_name {}

        impl Binding for #struct_name {
            type CpuBinding<'a> = #cpu_name<'a>;
            fn from_cpu_binding<'a>(bindings: &Self::CpuBinding<'a>) -> Self {
                #(#checks)*
                
                Self {
                    #(#field_constructors,)*
                }
            }
            fn resources<'a>(bindings: &Self::CpuBinding<'a>, stages: ash::vk::PipelineStageFlags2) -> Vec<(lava::command_buffer::ResourceHandle, lava::command_buffer::ResourceState)> {
                vec![
                    #(#states,)*
                ]
            }
        }
    };

    let out_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let mut cpu_file = PathBuf::from(out_dir);
    cpu_file.push("target");
    cpu_file.push("bindings");
    if !fs::exists(&cpu_file).unwrap() {
        fs::create_dir(&cpu_file).unwrap();
    }
    cpu_file.push(format!("{}.cpu.rs", struct_name));


    fs::write(&cpu_file, expandet.to_string()).expect("Could not write CPU bindings");

    TokenStream::new()
}
