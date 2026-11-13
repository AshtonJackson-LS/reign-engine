//! `#[derive(Reflect)]` and `#[derive(Component)]` for `reign_scene`.
//!
//! Structs with named fields reflect field by field; fieldless enums
//! reflect as a list of variants. Field attributes:
//!
//! - `#[reflect(skip)]`: not reflected (the editor never sees it).
//! - `#[reflect(read_only)]`: shown but not editable.
//! - `#[reflect(range(min, max))]`: editors clamp to this range.
//! - `#[reflect(tooltip = "...")]`: hover text; a doc comment works too.
//!
//! `Component` also takes `#[component(name = "...")]` (the stable name in
//! scene files; the type's name by default) and
//! `#[component(bounds(x, y, w, h))]` (the editor's click box, relative to
//! the object's position) or `#[component(bounds = "path::to_fn")]` for a
//! `fn(&Self) -> Option<Bounds>` when the box depends on the data. `#[serde(...)]` attributes are left to serde.

use proc_macro::TokenStream;
use proc_macro_crate::{FoundCrate, crate_name};
use proc_macro2::{Span, TokenStream as Tokens};
use quote::{format_ident, quote};
use syn::punctuated::Punctuated;
use syn::{
    Attribute, Data, DeriveInput, Error, Expr, Fields, LitStr, Meta, Token, parse_macro_input,
};

#[proc_macro_derive(Reflect, attributes(reflect))]
pub fn derive_reflect(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    reflect(&input).unwrap_or_else(Error::into_compile_error).into()
}

#[proc_macro_derive(Component, attributes(reflect, component))]
pub fn derive_component(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let component = || -> syn::Result<Tokens> {
        let mut out = reflect(&input)?;
        out.extend(component(&input)?);
        Ok(out)
    };
    component().unwrap_or_else(Error::into_compile_error).into()
}

/// The path to `reign_scene` from the crate being compiled: directly, or
/// through the `reign` facade.
fn krate() -> Tokens {
    match crate_name("reign_scene") {
        // reign_scene names itself with `extern crate self as reign_scene`,
        // which also works from its integration tests.
        Ok(FoundCrate::Itself) => quote!(::reign_scene),
        Ok(FoundCrate::Name(name)) => {
            let name = format_ident!("{name}");
            quote!(::#name)
        }
        Err(_) => match crate_name("reign") {
            Ok(FoundCrate::Name(name)) => {
                let name = format_ident!("{name}");
                quote!(::#name::scene)
            }
            Ok(FoundCrate::Itself) => quote!(crate::scene),
            Err(_) => quote!(::reign_scene),
        },
    }
}

/// What `#[reflect(...)]` says about a field.
#[derive(Default)]
struct FieldAttrs {
    skip: bool,
    read_only: bool,
    range: Option<(Expr, Expr)>,
    tooltip: Option<String>,
}

fn field_attrs(attrs: &[Attribute]) -> syn::Result<FieldAttrs> {
    let mut out = FieldAttrs::default();
    let mut docs = Vec::new();
    for attr in attrs {
        if attr.path().is_ident("doc") {
            if let Meta::NameValue(nv) = &attr.meta
                && let Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) = &nv.value
            {
                docs.push(s.value().trim().to_owned());
            }
            continue;
        }
        if !attr.path().is_ident("reflect") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("skip") {
                out.skip = true;
            } else if meta.path.is_ident("read_only") {
                out.read_only = true;
            } else if meta.path.is_ident("tooltip") {
                out.tooltip = Some(meta.value()?.parse::<LitStr>()?.value());
            } else if meta.path.is_ident("range") {
                let content;
                syn::parenthesized!(content in meta.input);
                let bounds = Punctuated::<Expr, Token![,]>::parse_terminated(&content)?;
                let [min, max] = <[Expr; 2]>::try_from(bounds.into_iter().collect::<Vec<_>>())
                    .map_err(|_| meta.error("range takes two bounds: range(min, max)"))?;
                out.range = Some((min, max));
            } else {
                return Err(meta.error("expected skip, read_only, range(min, max) or tooltip"));
            }
            Ok(())
        })?;
    }
    if out.tooltip.is_none() && !docs.is_empty() {
        out.tooltip = Some(docs.join(" "));
    }
    Ok(out)
}

fn reflect(input: &DeriveInput) -> syn::Result<Tokens> {
    let krate = krate();
    let name = &input.ident;
    let label = name.to_string();
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    let (body, kind) = match &input.data {
        Data::Struct(data) => {
            let Fields::Named(fields) = &data.fields else {
                return Err(Error::new_spanned(
                    &input.ident,
                    "Reflect needs a struct with named fields",
                ));
            };
            let mut infos = Vec::new();
            let mut arms = Vec::new();
            for field in &fields.named {
                let attrs = field_attrs(&field.attrs)?;
                if attrs.skip {
                    continue;
                }
                let ident = field.ident.as_ref().expect("named");
                let index = infos.len();
                let field_name = ident.to_string();
                let ty = &field.ty;
                let type_name = quote!(#ty).to_string().replace(' ', "");
                let read_only = attrs.read_only;
                let range = match attrs.range {
                    Some((min, max)) => quote!(Some(((#min) as f64, (#max) as f64))),
                    None => quote!(None),
                };
                let tooltip = match attrs.tooltip {
                    Some(t) => quote!(Some(#t)),
                    None => quote!(None),
                };
                infos.push(quote! {
                    #krate::FieldInfo {
                        name: #field_name,
                        type_name: #type_name,
                        read_only: #read_only,
                        range: #range,
                        tooltip: #tooltip,
                    }
                });
                arms.push(quote!(#index => Some(&mut self.#ident),));
            }
            let body = quote! {
                impl #impl_generics #krate::reflect::ReflectStruct for #name #ty_generics #where_clause {
                    fn fields(&self) -> &'static [#krate::FieldInfo] {
                        const FIELDS: &[#krate::FieldInfo] = &[#(#infos),*];
                        FIELDS
                    }
                    fn field_mut(&mut self, index: usize) -> Option<&mut dyn #krate::Reflect> {
                        match index {
                            #(#arms)*
                            _ => None,
                        }
                    }
                }
            };
            (body, quote!(Struct))
        }
        Data::Enum(data) => {
            let mut variants = Vec::new();
            for variant in &data.variants {
                if !matches!(variant.fields, Fields::Unit) {
                    return Err(Error::new_spanned(
                        variant,
                        "Reflect supports fieldless enums only",
                    ));
                }
                variants.push(&variant.ident);
            }
            let names: Vec<String> = variants.iter().map(|v| v.to_string()).collect();
            let indices: Vec<usize> = (0..variants.len()).collect();
            let body = quote! {
                impl #impl_generics #krate::reflect::ReflectEnum for #name #ty_generics #where_clause {
                    fn variants(&self) -> &'static [&'static str] {
                        &[#(#names),*]
                    }
                    fn variant_index(&self) -> usize {
                        match self {
                            #(#name::#variants => #indices,)*
                        }
                    }
                    fn set_variant_index(&mut self, index: usize) {
                        *self = match index {
                            #(#indices => #name::#variants,)*
                            _ => return,
                        };
                    }
                }
            };
            (body, quote!(Enum))
        }
        Data::Union(_) => {
            return Err(Error::new(Span::call_site(), "Reflect can't be derived for unions"));
        }
    };
    Ok(quote! {
        #body
        impl #impl_generics #krate::Reflect for #name #ty_generics #where_clause {
            fn type_name(&self) -> &'static str {
                #label
            }
            fn reflect_mut(&mut self) -> #krate::Mut<'_> {
                #krate::Mut::#kind(self)
            }
            fn as_any(&self) -> &dyn ::std::any::Any {
                self
            }
            fn as_any_mut(&mut self) -> &mut dyn ::std::any::Any {
                self
            }
        }
    })
}

fn component(input: &DeriveInput) -> syn::Result<Tokens> {
    let krate = krate();
    let name = &input.ident;
    let mut stable = name.to_string();
    let mut bounds = None;
    for attr in input.attrs.iter().filter(|a| a.path().is_ident("component")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("name") {
                stable = meta.value()?.parse::<LitStr>()?.value();
            } else if meta.path.is_ident("bounds") && meta.input.peek(Token![=]) {
                let path: syn::Path = meta.value()?.parse::<LitStr>()?.parse()?;
                bounds = Some(quote!(#path(self)));
            } else if meta.path.is_ident("bounds") {
                let content;
                syn::parenthesized!(content in meta.input);
                let parts = Punctuated::<Expr, Token![,]>::parse_terminated(&content)?;
                if parts.len() != 4 {
                    return Err(meta.error("bounds takes four values: bounds(x, y, w, h)"));
                }
                let parts: Vec<_> = parts.into_iter().collect();
                bounds = Some(quote!(Some(#krate::Bounds::new(#(#parts),*))));
            } else {
                return Err(meta.error(
                    "expected name = \"...\", bounds(x, y, w, h) or bounds = \"function\"",
                ));
            }
            Ok(())
        })?;
    }
    let bounds = bounds.map(|body| {
        quote! {
            fn bounds(&self) -> Option<#krate::Bounds> {
                #body
            }
        }
    });
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    Ok(quote! {
        impl #impl_generics #krate::Component for #name #ty_generics #where_clause {
            const NAME: &'static str = #stable;
            #bounds
        }
    })
}
