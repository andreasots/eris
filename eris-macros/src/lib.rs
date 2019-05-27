#![recursion_limit="128"]

extern crate proc_macro;

use proc_macro::TokenStream;
use quote::quote;
use syn::{Error, LitStr, Ident, FnArg, ItemFn, Pat, parse_macro_input};
use syn::spanned::Spanned;

fn extract_ident(pattern: &Pat) -> Option<&Ident> {
    match pattern {
        Pat::Ident(pat) => Some(&pat.ident),
        Pat::Box(pat) => extract_ident(&pat.pat),
        Pat::Ref(pat) => extract_ident(&pat.pat),
        _ => None,
    }
}

#[proc_macro_attribute]
pub fn rpc_handler(attr: TokenStream, item: TokenStream) -> TokenStream {
    let name = parse_macro_input!(attr as LitStr);
    let function = parse_macro_input!(item as ItemFn);

    let function_name = &function.ident;

    let mut args = vec![];
    for arg in &function.decl.inputs {
        match arg {
            FnArg::SelfRef(..) | FnArg::SelfValue(..) => {
                return Error::new(arg.span(), "#[rpc_handler] on methods not yet implemented").to_compile_error().into();
            },
            FnArg::Captured(arg) => {
                args.push((extract_ident(&arg.pat), &arg.ty));
            },
            FnArg::Inferred(..) => {
                return Error::new(arg.span(), "type information missing on an argument").to_compile_error().into();
            },
            FnArg::Ignored(ref arg) => {
                args.push((None, arg));
            },
        }
    }
    // Remove the context
    args.remove(0);

    let arg_idents = args.iter()
        .enumerate()
        .map(|(i, (ident, ty))| {
            if let Some(ident) = ident {
                Ident::new(&format!("arg_{}", ident), ident.span())
            } else {
                Ident::new(&format!("arg_{}", i), ty.span())
            }
        })
        .collect::<Vec<_>>();

    let deserialized_args = args.into_iter()
        .zip(arg_idents.iter())
        .enumerate()
        .map(|(i, ((ident, ty), var_name))| {
            let value = if let Some(ident) = ident {
                quote! {
                    match (args.next(), kwargs.remove(stringify!(#ident))) {
                        (Some(val), None) => val,
                        (None, Some(val)) => val,
                        (Some(_), Some(_)) => return Err(String::from(concat!("Multiple values for argument `", stringify!(#ident), "`"))),
                        (None, None) => return Err(String::from(concat!("Required argument ", stringify!(#ident), " missing"))),
                    }
                }
            } else {
                quote! {
                    match args.next() {
                        Some(val) => val,
                        None => return Err(String::from(concat!("Required positional argument ", stringify!(#i), " missing"))),
                    }
                }
            };

            let name = if let Some(ident) = ident {
                quote! { concat!("argument `", stringify!(#ident), "`") }
            } else {
                quote! { concat!("positional argument ", stringify!(#i)) }
            };

            quote! {
                let #var_name: #ty = match serde_json::from_value::<#ty>(#value) {
                    Ok(val) => val,
                    Err(err) => return Err(format!(concat!("Failed to deserialize ", #name, ": {}"), err)),
                };
            }
        })
        .collect::<Vec<_>>();

    TokenStream::from(quote! {
        ::inventory::submit! {
            static HANDLER: &'static (dyn crate::aiomas::Handler<crate::context::ErisContext> + Send + Sync + 'static) =
                &async move |
                    ctx: crate::context::ErisContext,
                    args: Vec<serde_json::Value>,
                    mut kwargs: std::collections::HashMap<String, serde_json::Value>
                | -> Result<serde_json::Value, String> {
                let mut args = args.into_iter().fuse();

                #(#deserialized_args);*

                match #function_name(ctx, #(#arg_idents),*).await {
                    Ok(val) => match serde_json::to_value(val) {
                        Ok(val) => Ok(val),
                        Err(err) => Err(format!("Failed to serialize the response: {}", err)),
                    },
                    Err(err) => Err(format!(concat!(stringify!(#function_name), "() returned an error: {}"), err)),
                }
            };

            crate::inventory::AiomasHandler::new(#name, HANDLER)
        }

        #function
    })
}
