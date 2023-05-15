#![doc = include_str!("../README.md")]

use proc_macro::TokenStream;
use proc_macro_error::{abort, proc_macro_error};
use quote::{quote, quote_spanned};
use syn::{
    punctuated::Punctuated, DataEnum, DeriveInput, Expr, ExprLit, Lit, Meta, MetaNameValue, Token,
};

#[proc_macro_derive(CMake2, attributes(cmake))]
#[proc_macro_error]
pub fn cmake_derive2(input: TokenStream) -> TokenStream {
    let ast: DeriveInput = syn::parse(input).unwrap();

    let cmake_attr = cmake_attribute(&ast.attrs).unwrap_or_default();
    let cmake_parse_path = if let Some(crate_path) = cmake_attr.pkg.as_ref() {
        quote! { #crate_path }
    } else {
        quote! { ::cmake_parser }
    };

    if cmake_attr.positional {
        impl_cmake_positional(ast, cmake_parse_path)
    } else {
        impl_cmake_regular(ast, cmake_attr, cmake_parse_path)
    }
}

fn impl_cmake_regular(
    ast: syn::DeriveInput,
    cmake_attr: CMakeAttribute,
    crate_path: proc_macro2::TokenStream,
) -> TokenStream {
    let cmake_impl = CMakeImpl::new(ast, crate_path.clone());

    let fns_cmake = match cmake_impl.to_cmake_fields() {
        CMakeFields::StructNamedFields(fields) => {
            let (positional_field_opts, regular_field_opts): (Vec<_>, Vec<_>) =
                fields.into_iter().partition(|field| field.attr.positional);

            let pos_var_defs = positional_var_defs(&positional_field_opts);
            let pos_fields = positional_fields(&positional_field_opts);

            let reg_var_defs = regular_var_defs(&regular_field_opts);
            let reg_fields = regular_fields(&regular_field_opts);
            let reg_buf_fields = regular_buf_fields(&regular_field_opts);
            let reg_enum_defs = regular_enum_defs(&regular_field_opts);
            let reg_enum_match = regular_enum_match(&regular_field_opts);
            let reg_if_stms = regular_if_stms(&regular_field_opts);

            let mode_default = cmake_attr
                .default
                .map(|def| {
                    use inflections::Inflect;

                    let def = quote::format_ident!("{}", def.to_pascal_case());
                    quote! { Some(CMakeParserMode::#def) }
                })
                .unwrap_or_else(|| {
                    quote! { None }
                });

            let fn_cmake_parse = cmake_impl.fn_cmake_parse(
                positional_field_opts.is_empty(),
                quote! {
                    use #crate_path::{CommandParseError, CMakeParse, CMakePositional, Token};

                    #[derive(Default)]
                    struct Buffers<'b> {
                        #(#reg_buf_fields,)*
                    }
                    enum CMakeParserMode {
                        #(#reg_enum_defs,)*
                    }
                    let mut buffers = Buffers::default();
                    let mut current_mode = #mode_default;

                    #(#pos_var_defs;)*
                    #(#reg_var_defs;)*

                    loop {
                        let Some((first, rest)) = tokens.split_first() else { break; };
                        tokens = rest;
                        let keyword = first.as_bytes();
                        #(#reg_if_stms)* {
                            match &current_mode {
                                Some(mode) => match mode {
                                    #(#reg_enum_match,)*
                                },
                                None => {
                                    return Err(crate::CommandParseError::UnknownOption(
                                        String::from_utf8_lossy(keyword).to_string(),
                                    ))
                                }
                            }
                        }
                    }

                    Ok((Self {
                        #(#pos_fields,)*
                        #(#reg_fields,)*
                    }, tokens))
                },
            );

            quote! {
                #fn_cmake_parse
            }
        }
        CMakeFields::EnumVariants(fields) => {
            // Box::new(cmake_impl.fn_cmake_parse(quote! {
            //     use #crate_path::CMakePositional;
            //     use #crate_path::CMakeParse;
            //     #(#pos_var_defs)*
            //     Ok((Self {
            //         #(#pos_fields,)*
            //     }, tokens))
            // }))
            quote! {}
        }
    };

    cmake_impl
        .trait_cmake_parse(quote! {
            #fns_cmake
        })
        .into()
}

fn impl_cmake_positional(
    ast: syn::DeriveInput,
    crate_path: proc_macro2::TokenStream,
) -> TokenStream {
    let cmake_impl = CMakeImpl::new(ast, crate_path.clone());

    let CMakeFields::StructNamedFields(struct_named_fields) = cmake_impl.to_cmake_fields() else {
        abort!(cmake_impl.ast.ident, "positional top level attribute allowed only for structs with named fields.");
    };

    let var_defs = positional_var_defs(&struct_named_fields);

    let fields = positional_fields(&struct_named_fields);

    let fn_cmake_parse = cmake_impl.fn_cmake_parse(
        false,
        quote! {
            use #crate_path::CMakePositional;
            #(#var_defs;)*
            Ok((Self {
                #(#fields,)*
            }, tokens))
        },
    );

    cmake_impl
        .trait_cmake_parse(quote! {
            #fn_cmake_parse
        })
        .into()
}

fn positional_var_defs(
    fields: &[CMakeOption],
) -> impl Iterator<Item = proc_macro2::TokenStream> + '_ {
    fields.iter().enumerate().map(
        |(index, CMakeOption {
             ident, lit_bstr, ..
         })| {
            let def_mut = if index == fields.len() - 1 { quote! { mut } } else { quote! {} };
            quote_spanned! { ident.span() => let (#ident, #def_mut tokens) = CMakePositional::positional(#lit_bstr, tokens)? }
        },
    )
}

fn positional_fields(
    fields: &[CMakeOption],
) -> impl Iterator<Item = proc_macro2::TokenStream> + '_ {
    fields.iter().map(|CMakeOption { ident, .. }| {
        quote_spanned! { ident.span() => #ident }
    })
}

fn regular_var_defs(fields: &[CMakeOption]) -> impl Iterator<Item = proc_macro2::TokenStream> + '_ {
    fields.iter().map(|CMakeOption { ident, .. }| {
        quote_spanned! { ident.span() => let mut #ident = CMakeParse::default_value() }
    })
}

fn regular_enum_defs(
    fields: &[CMakeOption],
) -> impl Iterator<Item = proc_macro2::TokenStream> + '_ {
    fields.iter().map(
        |CMakeOption {
             ident, ident_mode, ..
         }| {
            quote_spanned! { ident.span() => #ident_mode }
        },
    )
}

fn regular_enum_match(
    fields: &[CMakeOption],
) -> impl Iterator<Item = proc_macro2::TokenStream> + '_ {
    fields.iter().map(
        |CMakeOption {
             ident, ident_mode, ..
         }| {
            quote_spanned! { ident.span() => CMakeParserMode::#ident_mode => buffers.#ident.push(first.clone()) }
        },
    )
}

fn regular_fields(fields: &[CMakeOption]) -> impl Iterator<Item = proc_macro2::TokenStream> + '_ {
    fields.iter().map(|CMakeOption { ident, lit_str, .. }| {
        quote_spanned! { ident.span() => #ident: #ident.cmake_event_end(&buffers.#ident)?.ok_or_else(|| CommandParseError::MissingToken(#lit_str.to_string()))? }
    })
}

fn regular_buf_fields(
    fields: &[CMakeOption],
) -> impl Iterator<Item = proc_macro2::TokenStream> + '_ {
    fields.iter().map(|CMakeOption { ident, .. }| {
        quote_spanned! { ident.span() => #ident: Vec<Token<'b>> }
    })
}

fn regular_if_stms(fields: &[CMakeOption]) -> impl Iterator<Item = proc_macro2::TokenStream> + '_ {
    fields.iter().map(
        |CMakeOption {
             ident,
             ident_mode,
             lit_bstr,
             ..
         }| {
            quote_spanned! { ident.span() => if #ident.cmake_field_matches(#lit_bstr, keyword) {
                current_mode = if #ident.cmake_event_start(#lit_bstr, first, &buffers.#ident)? {
                    Some(CMakeParserMode::#ident_mode)
                } else {
                    None
                };
                buffers.#ident.clear();
            } else }
        },
    )
}

/// A derive macros for parsing CMake tokens to Rust structures and enums.
///
/// Requires dependency to `cmake-parser` crate.
#[proc_macro_derive(CMake, attributes(cmake))]
#[proc_macro_error]
pub fn cmake_derive(input: TokenStream) -> TokenStream {
    let ast: DeriveInput = syn::parse(input).unwrap();

    let cmake_attr = cmake_attribute(&ast.attrs).unwrap_or_default();
    let cmake_parse_path = if let Some(crate_path) = cmake_attr.pkg.as_ref() {
        quote! { #crate_path }
    } else {
        quote! { ::cmake_parser }
    };

    impl_cmake(&ast, cmake_parse_path)
}

struct CMakeImpl {
    ast: syn::DeriveInput,
    crate_path: proc_macro2::TokenStream,
}

enum CMakeFields {
    StructNamedFields(Vec<CMakeOption>),
    EnumVariants(Vec<CMakeOption>),
}
struct CMakeOption {
    id: String,
    attr: CMakeAttribute,
    ident: syn::Ident,
    ident_mode: syn::Ident,
    lit_str: proc_macro2::Literal,
    lit_bstr: proc_macro2::Literal,
}

impl CMakeOption {
    fn from_fields_named(fields_named: &syn::FieldsNamed) -> Vec<Self> {
        fields_named
            .named
            .iter()
            .filter_map(|f| {
                f.ident
                    .as_ref()
                    .map(|ident| (ident.clone(), cmake_attribute(&f.attrs).unwrap_or_default()))
            })
            .map(|(ident, attr)| {
                let id = ident.to_string();
                use inflections::Inflect;
                let ident_mode = quote::format_ident!("{}", id.to_pascal_case());
                let cmake_keyword = attr.rename.clone().unwrap_or_else(|| id.to_uppercase());
                let lit_str = proc_macro2::Literal::string(&cmake_keyword);
                let lit_bstr = proc_macro2::Literal::byte_string(cmake_keyword.as_bytes());
                CMakeOption {
                    id,
                    attr,
                    ident,
                    ident_mode,
                    lit_str,
                    lit_bstr,
                }
            })
            .collect()
    }
}

impl CMakeImpl {
    fn new(ast: syn::DeriveInput, crate_path: proc_macro2::TokenStream) -> Self {
        Self { ast, crate_path }
    }

    fn trait_cmake_parse(
        &self,
        content: impl quote::ToTokens,
    ) -> impl Into<proc_macro::TokenStream> {
        let Self { ast, crate_path } = self;

        let name = &ast.ident;
        let generics = &ast.generics;
        let type_params = generics.type_params();
        let (_, ty_generics, where_clause) = generics.split_for_impl();

        quote! {
            #[automatically_derived]
            impl <'t #(, #type_params)*> #crate_path::CMakeParse<'t> for #name #ty_generics #where_clause {
                #content
            }
        }
    }

    fn fn_cmake_parse(&self, is_mut: bool, content: impl quote::ToTokens) -> impl quote::ToTokens {
        let crate_path = &self.crate_path;
        let def_mut = if is_mut {
            quote! { mut }
        } else {
            quote! {}
        };

        quote! {
            fn cmake_parse<'tv>(
                #def_mut tokens: &'tv [#crate_path::Token<'t>],
            ) -> Result<(Self, &'tv [#crate_path::Token<'t>]), #crate_path::CommandParseError> {
                #content
            }
        }
    }

    fn to_cmake_fields(&self) -> CMakeFields {
        let name = &self.ast.ident;

        match &self.ast.data {
            syn::Data::Struct(data_struct) => match &data_struct.fields {
                syn::Fields::Named(fields_named) => {
                    CMakeFields::StructNamedFields(CMakeOption::from_fields_named(fields_named))
                }
                syn::Fields::Unnamed(_) => {
                    abort!(data_struct.fields, "unnamed fields are not supported")
                }
                syn::Fields::Unit => {
                    abort!(name, "unit fields are not supported")
                }
            },
            syn::Data::Enum(DataEnum { variants, .. }) => {
                let fields: Vec<_> = variants
                    .iter()
                    .map(|f| (f.ident.clone(), cmake_attribute(&f.attrs)))
                    .map(|(ident, cmake_attr)| {
                        let id = ident.to_string();
                        use inflections::Inflect;
                        let cmake_keyword = cmake_attr
                            .and_then(|a| a.rename)
                            .unwrap_or_else(|| id.to_constant_case());
                        let lit_cmake_keyword_str = proc_macro2::Literal::string(&cmake_keyword);
                        let lit_cmake_keyword_bstr =
                            proc_macro2::Literal::byte_string(cmake_keyword.as_bytes());
                        (ident, id, lit_cmake_keyword_str, lit_cmake_keyword_bstr)
                    })
                    .collect();
                todo!()
            }
            syn::Data::Union(_) => {
                abort!(name, "unions are not supported")
            }
        }
    }
}

fn impl_cmake(ast: &syn::DeriveInput, crate_path: proc_macro2::TokenStream) -> TokenStream {
    let name = &ast.ident;
    let generics = &ast.generics;
    let type_params = generics.type_params();
    let (_, ty_generics, where_clause) = &ast.generics.split_for_impl();

    let data = &ast.data;

    let gen = match data {
        syn::Data::Struct(data_struct) => match &data_struct.fields {
            syn::Fields::Named(fields_named) => {
                let fields: Vec<_> = fields_named
                    .named
                    .iter()
                    .filter_map(|f| {
                        f.ident
                            .as_ref()
                            .map(|ident| (ident, cmake_attribute(&f.attrs)))
                    })
                    .map(|(ident, cmake_attr)| {
                        let id = ident.to_string();
                        let cmake_keyword = cmake_attr
                            .and_then(|a| a.rename)
                            .unwrap_or_else(|| id.to_uppercase());
                        let lit_cmake_keyword_str = proc_macro2::Literal::string(&cmake_keyword);
                        let lit_cmake_keyword_bstr =
                            proc_macro2::Literal::byte_string(cmake_keyword.as_bytes());
                        (ident, id, lit_cmake_keyword_str, lit_cmake_keyword_bstr)
                    })
                    .collect();

                let variables = fields.iter().map(|(ident, _, _, lit_cmake_keyword_bstr)| {
                    quote_spanned! { ident.span() => let mut #ident = #crate_path::CMakeCommand::init(#lit_cmake_keyword_bstr, &mut keywords) }
                });
                let matches = fields.iter().map(|(ident, _, _, lit_cmake_keyword_bstr)| {
                    quote_spanned! { ident.span() => if #crate_path::CMakeCommand::update(&mut #ident, #lit_cmake_keyword_bstr, decl.option(), decl.args())? { continue; } }
                });

                let struct_fields = fields.iter().map(|(ident, _, lit_cmake_keyword, _)| {
                    quote_spanned! { ident.span() => #ident: #ident.ok_or_else(|| #crate_path::CommandParseError::MissingToken(#lit_cmake_keyword.to_string()))? }
                });

                quote! {
                    #[automatically_derived]
                    impl <'t #(, #type_params)*> #crate_path::CMakeCommand<'t> for #name #ty_generics #where_clause {

                        fn parse<'tv>(
                            mut tokens: &'tv [#crate_path::Token<'t>],
                        ) -> Result<(Self, &'tv [#crate_path::Token<'t>]), #crate_path::CommandParseError> {
                            let mut keywords = vec![];

                            #(#variables;)*

                            let declarations = #crate_path::declarations_by_keywords(tokens, &keywords);

                            for decl in declarations {
                                #(#matches)*
                                return Err(#crate_path::CommandParseError::UnknownOption(
                                    String::from_utf8_lossy(decl.option().as_bytes()).to_string(),
                                ));
                            }

                            Ok((
                                Self {
                                    #(#struct_fields,)*
                                },
                                &[],
                            ))
                        }
                    }
                }
            }
            syn::Fields::Unnamed(_) => {
                abort!(data_struct.fields, "unnamed fields are not supported")
            }
            syn::Fields::Unit => abort!(name, "unit fields are not supported"),
        },
        syn::Data::Enum(DataEnum { variants, .. }) => {
            let fields: Vec<_> = variants
                .iter()
                .map(|f| (f.ident.clone(), cmake_attribute(&f.attrs)))
                .map(|(ident, cmake_attr)| {
                    let id = ident.to_string();
                    use inflections::Inflect;
                    let cmake_keyword = cmake_attr
                        .and_then(|a| a.rename)
                        .unwrap_or_else(|| id.to_constant_case());
                    let lit_cmake_keyword_str = proc_macro2::Literal::string(&cmake_keyword);
                    let lit_cmake_keyword_bstr =
                        proc_macro2::Literal::byte_string(cmake_keyword.as_bytes());
                    (ident, id, lit_cmake_keyword_str, lit_cmake_keyword_bstr)
                })
                .collect();

            let enum_keywords = fields.iter().map(|(ident, _, _, lit_cmake_keyword_bstr)| {
                quote_spanned! {ident.span() => #lit_cmake_keyword_bstr }
            });
            let matches = fields.iter().map(|(ident, _, _, lit_cmake_keyword_bstr)| {
                quote_spanned! {ident.span() => #lit_cmake_keyword_bstr => Self::#ident }
            });
            quote! {
                #[automatically_derived]
                impl <'t #(, #type_params)*> #crate_path::CMakeCommand<'t> for #name #ty_generics #where_clause {

                    fn parse<'tv>(
                        mut tokens: &'tv [#crate_path::Token<'t>],
                    ) -> Result<(Self, &'tv [#crate_path::Token<'t>]), #crate_path::CommandParseError> {
                        todo!();
                    }

                    fn init(_default_name: &'static [u8], keywords: &mut Vec<&'static [u8]>) -> Option<Self> {
                        let enum_keywords: &[&[u8]] = &[
                            #(#enum_keywords,)*
                        ];
                        keywords.extend(enum_keywords);
                        Self::default_value()
                    }

                    fn update(
                        command: &mut Option<Self>,
                        _expected: &'static [u8],
                        option: & #crate_path::Token<'t>,
                        tokens: &[#crate_path::Token<'t>],
                    ) -> Result<bool, #crate_path::CommandParseError> {
                        let cmd = Some(match option.as_bytes() {
                            #(#matches,)*
                            _ => return Ok(false),
                        });

                        if !tokens.is_empty() {
                            return Err(#crate_path::CommandParseError::Incomplete);
                        }

                        *command = cmd;

                        Ok(true)
                    }

                }
            }
        }
        syn::Data::Union(_) => abort!(name, "unions are not supported"),
    };
    gen.into()
}

#[derive(Default)]
struct CMakeAttribute {
    default: Option<String>,
    positional: bool,
    list: bool,

    pkg: Option<syn::Path>,
    rename: Option<String>,
}

fn cmake_attribute(attrs: &[syn::Attribute]) -> Option<CMakeAttribute> {
    let attr = attrs.iter().find(|attr| attr.path().is_ident("cmake"))?;

    let nested = attr
        .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
        .unwrap();

    let mut rename = None;
    let mut pkg = None;
    let mut list = false;
    let mut positional = false;
    let mut default = None;

    for meta in nested {
        match meta {
            Meta::Path(p) if p.is_ident("list") => list = true,
            Meta::Path(p) if p.is_ident("positional") => positional = true,
            Meta::NameValue(MetaNameValue {
                ref path,
                value:
                    Expr::Lit(ExprLit {
                        lit: Lit::Str(s), ..
                    }),
                ..
            }) => {
                if path.is_ident("default") {
                    default = Some(s.value());
                } else if path.is_ident("rename") {
                    rename = Some(s.value());
                } else if path.is_ident("pkg") {
                    pkg = s.parse().ok();
                }
            }
            _ => (),
        }
    }

    Some(CMakeAttribute {
        pkg,
        rename,
        default,
        positional,
        list,
    })
}

#[cfg(test)]
mod tests {
    use syn::{parse_quote, Attribute};

    use super::*;

    #[test]
    fn check_def_attr() {
        let attr: Attribute = parse_quote! {
            #[cmake(default = "COMMAND",
                rename = "mmm",
                pkg = "crate",
                list,
                positional
            )]
        };

        let cmake_attr = cmake_attribute(&[attr]).expect("attrs");
        assert!(cmake_attr.pkg.is_some());
        assert_eq!(Some("mmm"), cmake_attr.rename.as_deref());
        assert_eq!(Some("COMMAND"), cmake_attr.default.as_deref());
        assert!(cmake_attr.positional);
        assert!(cmake_attr.list);
    }
}