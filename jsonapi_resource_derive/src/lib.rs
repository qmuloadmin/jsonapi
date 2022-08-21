extern crate proc_macro;

use darling::{ast, util, FromDeriveInput, FromField, FromMeta};
use proc_macro::TokenStream;
use proc_macro2::TokenStream as TS2;
use quote::quote;
use syn::{self, Type};

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
    ty: syn::Type,
}

struct RelationNames {
    resource_name: String,
    field_name: syn::Ident,
    relation_name: String,
    is_option: bool,
}

#[proc_macro_derive(Responder, attributes(jsonapi))]
pub fn resource_macro_derive(input: TokenStream) -> TokenStream {
    let ast = syn::parse(input).unwrap();
    impl_responder_macro(&ast)
}

#[proc_macro_derive(IntoRelationships, attributes(jsonapi))]
pub fn into_relations_macro_derive(input: TokenStream) -> TokenStream {
    impl_relations_macro(&syn::parse(input).unwrap())
}

#[proc_macro_derive(FromRelationships, attributes(jsonapi))]
pub fn from_relations_macro_derive(input: TokenStream) -> TokenStream {
    impl_from_relations_macro(&syn::parse(input).unwrap())
}

#[proc_macro_derive(FromRequest, attributes(jsonapi))]
pub fn from_request_macro_derive(input: TokenStream) -> TokenStream {
    impl_from_request_macro(&syn::parse(input).unwrap())
}

fn impl_from_request_macro(ast: &syn::DeriveInput) -> TokenStream {
    let desc = ResourceFieldDescription::from(ResourceProps::from_derive_input(ast).unwrap());
    let missing_id_err = format!(
        "missing required id field in request for resource {}",
        desc.type_name
    );
    let id_not_allowed_err = format!(
        "'id' field not allowed in request for resource {}",
        desc.type_name
    );
    let id_let_statement = match desc.id_field {
        Some(_) => {
            // if there is an id field, require the request to have an ID
            quote! {
                let id = req.data.id.ok_or(::jsonapi::Error::new_bad_request(#missing_id_err))?;
            }
        }
        None => {
            // if there is no id field, don't allow the request to have an ID
            quote! {
                if req.data.id.is_some() {
                    return Err(::jsonapi::Error::new_bad_request(#id_not_allowed_err));
                }
            }
        }
    };
    let relations_let_statement = match &desc.relations_field {
        Some(field) => {
            let ty = &field.ty;
            quote! {
                let rels: #ty = ::jsonapi::FromRelationships::from_relationships(req.data.relationships)?;
            }
        }
        None => {
            quote! {
                let _: () = ::jsonapi::FromRelationships::from_relationships(req.data.relationships)?;
            }
        }
    };
    let id_statement = match desc.id_field {
        Some(field) => {
            let name = field.ident.unwrap();
            quote! {
                #name: ::jsonapi::FromID::from_id(id)?,
            }
        }
        None => TS2::new(),
    };
    let relations_statement = match desc.relations_field {
        Some(field) => {
            let name = field.ident.unwrap();
            quote! {
                #name: rels,
            }
        }
        None => TS2::new(),
    };
    let name = desc.name;
    let attr_type;
    let attributes_statement;
    match desc.attr_field {
        None => {
            // TODO using Option<()> seems unnecessary. We should be able to just use ()
            // or a wrapper type and implement some custom serde rules for that type
            // to make it not require the `attributes` object in the request/response
            attr_type = Type::from_string("Option<()>".into()).unwrap();
            attributes_statement = TS2::new();
        }
        Some(field) => {
            attr_type = field.ty;
            let attr_name = Some(field.ident);
            attributes_statement = quote! {
                #attr_name: req.data.attributes
            }
        }
    }

    let gen = quote! {
        impl ::jsonapi::FromRequest for #name {
            type Attributes = #attr_type;
            fn from_request(req: ::jsonapi::Request<#attr_type>) -> Result<Self, ::jsonapi::Error> {
                #id_let_statement
                #relations_let_statement
                let result = #name {
                    #id_statement
                    #relations_statement
                    #attributes_statement
                };
                Ok(result)
            }
        }
    };
    gen.into()
}

fn impl_from_relations_macro(ast: &syn::DeriveInput) -> TokenStream {
    let desc = RelationFieldDescription::from(RelationsProps::from_derive_input(ast).unwrap());
    let mut all_options = true;
    let var_statements: Vec<TS2> = desc
        .fields
        .iter()
        .map(|names| {
            if !names.is_option {
                all_options = false;
            }
            let name = &names.relation_name;
            let field = &names.field_name;
            let ts = if names.is_option {
                quote! {
                    let #field;
                    if let Some(t) = rels.remove(#name) {
                        #field = Some(::jsonapi::FromRelationship::from_relationship(t.data)?);
                    } else {
                        #field = None;
                    };
                }
            } else {
                let err_msg = format!("missing mandatory relationship '{}'", name);
                quote! {
                    let #field;
                    if let Some(t) = rels.remove(#name) {
                        #field = ::jsonapi::FromRelationship::from_relationship(t.data)?;
                    } else {
                        return Err(::jsonapi::Error::new_bad_request(#err_msg));
                    };
                }
            };
            ts
        })
        .collect();
    let struct_statements: Vec<TS2> = desc
        .fields
        .into_iter()
        .map(|names| {
            let field = names.field_name;
            let ts = quote! {
                #field,
            };
            ts
        })
        .collect();
    let none_handler = if all_options {
        // TODO this isn't the most efficient approach in the world
        quote! {
			let mut rels: ::std::collections::BTreeMap<String, ::jsonapi::RelationshipData> = match rels {
				None => ::std::collections::BTreeMap::new(),
				Some(b) => b
			};
		}
    } else {
        quote! {
            let mut rels = rels.ok_or_else(|| ::jsonapi::Error::new_bad_request("missing mandatory relationships object"))?;
        }
    };
    let struct_name = desc.name;
    let gen = quote! {
        impl ::jsonapi::FromRelationships for #struct_name {
            fn from_relationships(rels: Option<::std::collections::BTreeMap<String, ::jsonapi::RelationshipData>>) -> Result<Self, ::jsonapi::Error> {
                #none_handler
                #(#var_statements)*
                Ok(#struct_name {
                    #(#struct_statements)*
                })
            }
        }
    };
    gen.into()
}

fn impl_relations_macro(ast: &syn::DeriveInput) -> TokenStream {
    let props: RelationsProps = RelationsProps::from_derive_input(ast).unwrap();
    let desc = RelationFieldDescription::from(props);
    let statements: Vec<TS2> = desc.fields
        .into_iter()
        .map(|names| {
            let name = names.relation_name;
            let resource = names.resource_name;
            let field = names.field_name;
            let ts = if names.is_option {
				quote! {
				if let Some(field) = self.#field {
					rels.insert(#name.to_string(), ::jsonapi::IntoRelationship::into_relationship(field, #resource).into());
				}
				}
			} else {
				 quote! {
                rels.insert(#name.to_string(), ::jsonapi::IntoRelationship::into_relationship(self.#field, #resource).into());
				 }
			};
            ts
        })
        .collect();
    let struct_name = desc.name;
    (quote! {
        impl ::jsonapi::IntoRelationships for #struct_name {
            fn into_relationships(self) -> Option<::std::collections::BTreeMap<String, ::jsonapi::RelationshipData>> {
                let mut rels = ::std::collections::BTreeMap::new();
                #(#statements)*
                Some(rels)
            }
        }
    })
    .into()
}

fn impl_responder_macro(ast: &syn::DeriveInput) -> TokenStream {
    let props = ResourceProps::from_derive_input(ast).unwrap();
    let desc = ResourceFieldDescription::from(props);
    let (relations_fn, relations_type) = match desc.relations_field.as_ref() {
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
    let (attr_fn, attr_type) = match desc.attr_field {
        None => (quote! { None }, quote! { Option<()> }),
        Some(field) => {
            let attr_name = field.ident.as_ref().unwrap();
            let field_type = &field.ty;
            (
                quote! {
                    self.#attr_name.clone()
                },
                quote! {
                    #field_type
                },
            )
        }
    };
    let id_name = desc.id_field.unwrap().ident.unwrap();
    let name = desc.name;
    let type_name = desc.type_name;
    // TODO find a way to remove the clone() of attributes
    let gen = quote! {
        impl ::jsonapi::Responder for #name {
            type Attributes = #attr_type;
            type Relations = #relations_type;

            fn name() -> String {
                #type_name.to_owned().to_lowercase()
            }

            fn id(&self) -> ::jsonapi::ID {
                ToString::to_string(&self.#id_name).into()
            }

            fn attributes(&self) -> #attr_type {
                #attr_fn
            }

            fn relations(&self) -> #relations_type {
                #relations_fn
            }
        }

    };
    gen.into()
}

struct ResourceFieldDescription {
    name: syn::Ident,
    type_name: String,
    id_field: Option<ResourceField>,
    attr_field: Option<ResourceField>,
    relations_field: Option<ResourceField>,
}

struct RelationFieldDescription {
    name: syn::Ident,
    fields: Vec<RelationNames>,
}

impl From<RelationsProps> for RelationFieldDescription {
    fn from(props: RelationsProps) -> RelationFieldDescription {
        RelationFieldDescription {
            fields: match props.data {
                ast::Data::Struct(data) => data
                    .fields
                    .into_iter()
                    .map(|field| {
                        let resource_name = match field.name {
                            Some(name) => name,
                            None => format!("{}s", field.ident.clone().unwrap()),
                        };
                        let is_option = match field.ty {
				syn::Type::Path(path) => {
					if path.path.leading_colon.is_none() && path.path.segments.len() == 1 {
						match path.path.segments.into_iter().next().unwrap().ident {
							i if i == "Option" => true,
							_ => false,
						}
					} else {
						panic!("unsupported type name for deriving Relations, Option<T> or T where T: Into<ID> supported")
					}
				},
				_ => panic!("unsupported type for deriving Relations, Option<T> or T where T:Into<ID supported")
			};
                        RelationNames {
                            resource_name,
                            field_name: field.ident.clone().unwrap(),
                            relation_name: field.ident.clone().unwrap().to_string(),
                            is_option,
                        }
                    })
                    .collect(),
                _ => panic!("unreachable"),
            },
            name: props.ident,
        }
    }
}

impl From<ResourceProps> for ResourceFieldDescription {
    fn from(props: ResourceProps) -> Self {
        let name = props.ident;
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
        ResourceFieldDescription {
            name,
            type_name,
            id_field,
            attr_field,
            relations_field,
        }
    }
}
