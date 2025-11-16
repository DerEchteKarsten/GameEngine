use ash::vk::{AccessFlags2, ImageAspectFlags, ImageLayout, PipelineStageFlags2};
use bytemuck::Pod;
use lava::command_buffer::ResourceHandle;
use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{Attribute, DeriveInput, Expr, Type, TypePtr, TypeTuple, parse_macro_input};


#[proc_macro_attribute]
pub fn shader(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let func = parse_macro_input!(item as syn::ItemFn);
    
    let stage = func.attrs
        .iter()
        .find(|e| e.path().is_ident("spirv"))
        .expect("Function does not have a spirv attribute")
        .parse_args::<proc_macro2::TokenStream>()
        .unwrap()
        .to_string();

    let stage = match stage {
        s if s.contains("compute") => quote!(PipelineStageFlags2::COMPUTE_SHADER),
        s if s.contains("vertex") => quote!(PipelineStageFlags2::VERTEX_SHADER),
        s if s.contains("fragment") => quote!(PipelineStageFlags2::FRAGMENT_SHADER),
        s if s.contains("ray_generation") => quote!(PipelineStageFlags2::RAY_TRACING_SHADER_KHR),
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
        }).map(|e| quote!(#e.ty.as_ref().clone()))
        .unwrap_or(quote!(()));

    let expandet = quote! {
        impl Shader for fn(#func) {
            const STAGE: PipelineStageFlags2 = #stage;
            type GpuBinding = #binding_type;
        }
    };
    expandet.into()
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
    let mut binding_entries = Vec::new();
    let mut handles = Vec::new();
    let mut aspects = Vec::new();
    let mut layout = Vec::new();
    for field in fields {
        let name = field.ident.as_ref().unwrap();
        let is_write_only = field.attrs.iter().any(|e| e.path().is_ident("write_only"));
        let (access, cpu_type, handle, pc, aspect, layout) = match &field.ty {
            Type::Ptr(ty) => {
                let el = ty.elem.as_ref().clone();
                (if ty.mutability.is_some() {
                    if is_write_only {
                        quote!(ash::vk::AccessFlags2::SHADER_STORAGE_WRITE)
                    }else {
                        quote!(ash::vk::AccessFlags2::SHADER_STORAGE_WRITE | ash::vk::AccessFlags2::SHADER_STORAGE_READ)
                    }
                }else {
                    if is_write_only {
                        quote!(compile_error!("Texture can not be written to but still hast write_only tag"))
                    } else {
                        quote!(ash::vk::AccessFlags2::SHADER_STORAGE_READ)
                    }
                },
                quote!(&'a lava::vkobjects::Buffer<#el>),
                quote!(lava::command_buffer::ResourceHandle::Buffer(binding.#name.handle)),
                false,
                ash::vk::ImageAspectFlags::empty(),
                ash::vk::ImageLayout::UNDEFINED,
            )},
            Type::Path(ty) => {
                let name = ty.path.segments.last().unwrap().ident.to_string();
                let handle = quote!(lava::command_buffer::ResourceHandle::Image(binding.#name.view, binding.#name.image));
                let aspect = quote!(binding.#name.get_aspects());
                match name.as_str() {
                    "ImageHandle" => {
                        (if is_write_only {
                            quote!(ash::vk::AccessFlags2::SHADER_STORAGE_WRITE)
                        }else {
                            quote!(ash::vk::AccessFlags2::SHADER_STORAGE_WRITE | ash::vk::AccessFlags2::SHADER_STORAGE_READ)
                        }, 
                        quote!(&'a lava::vkobjects::Image),
                        handle,
                        false,
                        aspect,
                        ash::vk::ImageLayout::GENERAL)
                    }
                    "TextureHandle" => {
                        (if is_write_only {
                            quote!(compile_error!("Texture can not be written to but still hast write_only tag"))
                        } else {
                            quote!(ash::vk::AccessFlags2::SHADER_SAMPLED_READ)
                        }, 
                        quote!(&'a lava::vkobjects::Image),
                        handle,
                        false)
                    },
                    _ => {
                        (quote!(), quote!{#ty}, quote!(), true)
                    }
                }
            }
            _ => (quote!(::None), quote!(compile_error!("Texture can not be written to but still hast write_only tag")))
        };

        if !pc {
            binding_entries.push(access);
        }
        if !pc {
            handle.push(handle);
        }
        cpu_fields.push(quote! {
            pub #name: #cpu_type
        });
    }

    let expandet = quote! {
        struct #cpu_name <'a> {
            #(#cpu_fields,)*
        }

        impl Binding for #struct_name {
            type CpuBinding = #cpu_name;
            fn from_cpu_binding(binding: &Self::CpuBinding, stage: ash::vk::PipelineStageFlags2) -> (Self, Vec<(ResourceHandle, ResourceState)>) {
                vec![
                    #((#handles,ResourceState {
                        access: #binding_entries,
                        aspect: #aspects,
                        stage,
                    }),)*
                ];
            }
        }
    };

    TokenStream::from(expandet)
}
