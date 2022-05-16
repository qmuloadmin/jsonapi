extern crate proc_macro;

use darling::{ast, util, FromDeriveInput, FromField};
use proc_macro::TokenStream;
use proc_macro2::TokenStream as TS2;
use quote::quote;
use syn;

#[derive(FromDeriveInput)]
#[darling(attributes(jsonapi), supports(struct_named))]
struct ResourceProps {
    ident: syn::Ident,
    data: ast::Data<util::Ignored, ResourceField>,
    name: Option<String>,
}

#[derive(FromField, Clone)]
struct ResourceField {
    ident: Option<syn::Ident>,
    ty: syn::Type,
}

#[derive(FromDeriveInput)]
#[darling(attributes(jsonapi), supports(struct_named))]
struct RelationsProps {
    ident: syn::Ident,
    data: ast::Data<util::Ignored, RelationsField>,
}

#[derive(FromField, Clone)]
struct RelationsField {
    ident: Option<syn::Ident>,
    name: Option<String>,
}

struct RelationNames {
    resource_name: String,
    field_name: syn::Ident,
    relation_name: String,
}

#[proc_macro_derive(Resource, attributes(jsonapi))]
pub fn resource_macro_derive(input: TokenStream) -> TokenStream {
    let ast = syn::parse(input).unwrap();
    impl_resource_macro(&ast)
}

#[proc_macro_derive(Relations, attributes(jsonapi))]
pub fn relations_macro_derive(input: TokenStream) -> TokenStream {
    impl_relations_macro(&syn::parse(input).unwrap())
}

fn impl_relations_macro(ast: &syn::DeriveInput) -> TokenStream {
    let props: RelationsProps = RelationsProps::from_derive_input(ast).unwrap();
    let fields = match props.data {
        ast::Data::Struct(data) => data.fields.into_iter().map(|field| {
            let resource_name = match field.name {
                Some(name) => name,
                None => format!("{}s", field.ident.clone().unwrap()),
            };
            RelationNames {
                resource_name,
                field_name: field.ident.clone().unwrap(),
                relation_name: field.ident.clone().unwrap().to_string(),
            }
        }),
        _ => panic!("unreachable"),
    };
    let struct_name = props.ident;
    let statements: Vec<TS2> = fields
        .into_iter()
        .map(|names| {
            let name = names.relation_name;
            let resource = names.resource_name;
            let field = names.field_name;
            let ts = quote! {
                rels.insert(#name.to_string(), ::jsonapi::Relatable::into_relation(&self.#field, #resource).into());
            };
            ts
        })
        .collect();
    (quote! {
        impl Relations for #struct_name {
            fn relationships(&self) -> Option<::std::collections::BTreeMap<String, ::jsonapi::RelationshipData>> {
                let mut rels = ::std::collections::BTreeMap::new();
                #(#statements)*
                Some(rels)
            }
        }
    })
    .into()
}

fn impl_resource_macro(ast: &syn::DeriveInput) -> TokenStream {
    let props = ResourceProps::from_derive_input(ast).unwrap();
    let name = &props.ident;
    let mut type_name = format!("{}s", name);
    if let Some(custom_name) = props.name {
        type_name = custom_name;
    }
    // try to identify the id, attributes fields.
    let mut id_field: Option<ResourceField> = None;
    let mut attr_field: Option<ResourceField> = None;
    let mut relations_field: Option<ResourceField> = None;
    match props.data {
        ast::Data::Struct(data) => {
            for field in &data.fields {
                if let Some(i) = &field.ident {
                    if i == "id" {
                        id_field = Some(field.clone())
                    } else if i == "attributes" {
                        attr_field = Some(field.clone())
                    } else if i == "relations" {
                        relations_field = Some(field.clone())
                    }
                }
            }
        }
        _ => panic!("unsupported macro input: must use Struct"),
    }

    let attr_type = &attr_field.as_ref().unwrap().ty;
    let attr_name = attr_field.as_ref().unwrap().ident.as_ref().unwrap();
    let (relations_fn, relations_type) = match relations_field.as_ref() {
        None => (quote! { () }, quote! {()}),
        Some(field) => {
            let relations_name = field.ident.as_ref().unwrap();
            let field_type = &field.ty;
            (
                quote! {
                    self.#relations_name.clone()
                },
                quote! {
                    #field_type
                },
            )
        }
    };
    let id_name = id_field.unwrap().ident.unwrap();
    let gen = quote! {
        impl Resource for #name {
            type Attributes = #attr_type;
            type Relations = #relations_type;

            fn name(&self) -> String {
                #type_name.to_owned().to_lowercase()
            }

            fn id(&self) -> String {
                self.#id_name.clone()
            }

            fn attributes(&self) -> #attr_type {
                self.#attr_name.clone()
            }

            fn relations(&self) -> #relations_type {
                #relations_fn
            }
        }

    };
    gen.into()
}
