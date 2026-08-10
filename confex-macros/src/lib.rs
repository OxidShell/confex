use proc_macro::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields, Lit, Type, TypePath, parse_macro_input, spanned::Spanned as _};

/// Derive `ConfigSchema` for a struct.
///
/// ## Struct-level attributes
///
/// ```ignore
/// #[config(app_name = "myapp", version = 1)]
/// ```
///
/// Both `app_name` and `version` are required.
///
/// ## Field-level attributes
///
/// ```ignore
/// #[config(merge = "deep")]     // deep | extend | replace | replace_if_present
/// #[config(encrypted)]          // field value is encrypted at rest
/// #[config(flatten)]            // nested struct merged as a table (implies merge = "deep")
/// ```
///
/// `Option<T>` fields automatically get `ReplaceIfPresent` unless overridden with
/// `#[config(merge = "replace")]`.
#[proc_macro_derive(ConfigSchema, attributes(config))]
pub fn derive_config_schema(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match impl_config_schema(&input) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn impl_config_schema(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let (app_name, schema_version) = parse_struct_attrs(input)?;

    let fields = match &input.data {
        Data::Struct(s) => match &s.fields {
            Fields::Named(f) => &f.named,
            _ => {
                return Err(syn::Error::new(
                    input.span(),
                    "ConfigSchema requires a struct with named fields",
                ))
            }
        },
        _ => {
            return Err(syn::Error::new(
                input.span(),
                "ConfigSchema can only be derived for structs",
            ))
        }
    };

    let mut strategy_entries: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut encrypted_entries: Vec<String> = Vec::new();

    for field in fields {
        let field_name = field.ident.as_ref().unwrap().to_string();
        let field_attrs = parse_field_attrs(field)?;

        let strategy_ts: Option<proc_macro2::TokenStream> = match field_attrs.merge.as_ref() {
            Some(MergeAttr::ReplaceIfPresent) => {
                Some(quote! { ::confex::MergeStrategy::ReplaceIfPresent })
            }
            Some(MergeAttr::Extend) => Some(quote! { ::confex::MergeStrategy::Extend }),
            Some(MergeAttr::Deep) => Some(quote! { ::confex::MergeStrategy::Deep }),
            // No explicit attr: infer from field shape
            None if field_attrs.flatten => Some(quote! { ::confex::MergeStrategy::Deep }),
            None if is_option_type(&field.ty) => {
                Some(quote! { ::confex::MergeStrategy::ReplaceIfPresent })
            }
            // Some(Replace) or unguarded None → Replace is the default, emit nothing
            Some(MergeAttr::Replace) | None => None,
        };

        if let Some(ts) = strategy_ts {
            strategy_entries.push(quote! { (#field_name, #ts) });
        }

        if field_attrs.encrypted {
            encrypted_entries.push(field_name);
        }
    }

    let ident = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    Ok(quote! {
        impl #impl_generics ::confex::ConfigSchema for #ident #ty_generics #where_clause {
            const APP_NAME: &'static str = #app_name;
            const SCHEMA_VERSION: u32 = #schema_version;

            fn field_strategies() -> ::confex::FieldStrategies {
                &[ #(#strategy_entries),* ]
            }

            fn encrypted_fields() -> &'static [&'static str] {
                &[ #(#encrypted_entries),* ]
            }
        }
    })
}

#[derive(Debug)]
enum MergeAttr {
    Replace,
    ReplaceIfPresent,
    Extend,
    Deep,
}

#[derive(Default)]
struct FieldAttrs {
    merge: Option<MergeAttr>,
    encrypted: bool,
    flatten: bool,
}

fn parse_struct_attrs(input: &DeriveInput) -> syn::Result<(String, u32)> {
    let mut app_name: Option<String> = None;
    let mut version: Option<u32> = None;

    for attr in &input.attrs {
        if !attr.path().is_ident("config") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("app_name") {
                match meta.value()?.parse::<Lit>()? {
                    Lit::Str(s) => {
                        app_name = Some(s.value());
                        Ok(())
                    }
                    _ => Err(meta.error("app_name must be a string literal")),
                }
            } else if meta.path.is_ident("version") {
                match meta.value()?.parse::<Lit>()? {
                    Lit::Int(i) => {
                        version = Some(i.base10_parse()?);
                        Ok(())
                    }
                    _ => Err(meta.error("version must be an integer literal")),
                }
            } else {
                Err(meta.error("unknown attribute; expected app_name or version"))
            }
        })?;
    }

    let app_name = app_name.ok_or_else(|| {
        syn::Error::new(input.span(), "#[config(app_name = \"...\")] is required")
    })?;
    let version = version
        .ok_or_else(|| syn::Error::new(input.span(), "#[config(version = N)] is required"))?;

    Ok((app_name, version))
}

fn parse_field_attrs(field: &syn::Field) -> syn::Result<FieldAttrs> {
    let mut attrs = FieldAttrs::default();

    for attr in &field.attrs {
        if !attr.path().is_ident("config") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("encrypted") {
                attrs.encrypted = true;
                Ok(())
            } else if meta.path.is_ident("flatten") {
                attrs.flatten = true;
                Ok(())
            } else if meta.path.is_ident("merge") {
                match meta.value()?.parse::<Lit>()? {
                    Lit::Str(s) => {
                        attrs.merge = Some(match s.value().as_str() {
                            "replace" => MergeAttr::Replace,
                            "replace_if_present" => MergeAttr::ReplaceIfPresent,
                            "extend" => MergeAttr::Extend,
                            "deep" => MergeAttr::Deep,
                            other => {
                                return Err(syn::Error::new(
                                    s.span(),
                                    format!(
                                        "unknown merge strategy {other:?}; \
                                         expected replace, replace_if_present, extend, or deep"
                                    ),
                                ))
                            }
                        });
                        Ok(())
                    }
                    _ => Err(meta.error("merge value must be a string literal")),
                }
            } else {
                Err(meta.error(
                    "unknown field config attribute; expected merge, encrypted, or flatten",
                ))
            }
        })?;
    }

    Ok(attrs)
}

fn is_option_type(ty: &Type) -> bool {
    let Type::Path(TypePath { qself: None, path }) = ty else {
        return false;
    };
    let segs: Vec<_> = path.segments.iter().collect();
    match segs.as_slice() {
        [s] if s.ident == "Option" => true,
        [a, b] if a.ident == "option" && b.ident == "Option" => true,
        [a, b, c]
            if (a.ident == "std" || a.ident == "core")
                && b.ident == "option"
                && c.ident == "Option" =>
        {
            true
        }
        _ => false,
    }
}
