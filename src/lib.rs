// Based on RustEmbed crate

extern crate proc_macro;
#[macro_use]
extern crate quote;

use proc_macro::TokenStream;
use std::{env, fs, path::Path, time::SystemTime};

use chrono::{DateTime, Utc};
use miniz_oxide::deflate::compress_to_vec;
use syn::export::Span;
use syn::{export::TokenStream2, Data, DeriveInput, Fields, Ident, Lit, LitByteStr, Meta};

fn generate_assets(ident: &Ident, folder_path: String) -> TokenStream2 {
    let mut match_values = Vec::new();
    let mut modified_values = Vec::new();
    let mut list_values = Vec::new();

    for entry in fs::read_dir(folder_path.clone()).unwrap() {
        if let Ok(entry) = entry {
            let rel_path = entry.file_name().to_str().unwrap().to_owned();
            let full_path = std::fs::canonicalize(entry.path())
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned();

            let content = std::fs::read(full_path).unwrap();
            let compressed = compress_to_vec(&content, 10);
            let bytes = LitByteStr::new(&compressed[..], Span::call_site());

            match_values.push(quote! {
                #rel_path => {
                    Some(std::borrow::Cow::Borrowed(#bytes))
                }
            });

            let modif = if let Ok(metadata) = entry.metadata() {
                metadata.modified().unwrap()
            } else {
                SystemTime::now()
            };
            let modif: DateTime<Utc> = DateTime::from(modif);
            let modif = modif.format("%a, %d %b %Y %T GMT").to_string();
            modified_values.push(quote! {
                #rel_path => {Some(std::borrow::Cow::from(#modif))}
            });

            list_values.push(rel_path);
        }
    }

    quote! {
        #[cfg(not(debug_assertions))]
        impl #ident {
            pub fn get(file_path: &str) -> Option<std::borrow::Cow<'static, [u8]>> {
                match file_path {
                    #(#match_values)*
                    _ => None
                }
            }

            pub fn modif(file_path: &str) -> Option<std::borrow::Cow<'static, str>> {
                match file_path {
                    #(#modified_values)*
                    _ => None
                }
            }
        }

        #[cfg(debug_assertions)]
        impl #ident {
            pub fn get(file_path: &str) -> Option<std::borrow::Cow<'static, [u8]>> {
                let file_path = std::path::Path::new(#folder_path).join(file_path);
                match std::fs::read(file_path) {
                    Ok(contents) => {
                        let compressed = miniz_oxide::deflate::compress_to_vec(&contents, 6);

                        Some(std::borrow::Cow::Owned(compressed))
                    },
                    Err(_e) => None,
                }
            }

            pub fn modif(file_path: &str) -> Option<std::borrow::Cow<'static, str>> {
                use chrono::{DateTime, Utc};

                let file_path = std::path::Path::new(#folder_path).join(file_path);
                match std::fs::metadata(file_path) {
                    Ok(metadata) => {
                        match metadata.modified() {
                            Ok(modif) => {
                                let modif: DateTime<Utc> = DateTime::from(modif);
                                // Tue, 01 Dec 2020 00:00:00 GMT
                                let modif = modif.format("%a, %d %b %Y %T GMT").to_string();

                                Some(std::borrow::Cow::from(modif))
                            }
                            Err(_e) => None,
                        }
                    }
                    Err(_e) => None,
                }
            }
        }
    }
}

fn impl_asset_embed(ast: &DeriveInput) -> TokenStream2 {
    match ast.data {
        Data::Struct(ref data) => match data.fields {
            Fields::Unit => {}
            _ => panic!("Only on unit structs"),
        },
        _ => panic!("Only on unit structs"),
    };

    let attribute = ast
        .attrs
        .iter()
        .find(|value| value.path.is_ident("folder"))
        .expect(r#"#[derive(AssetEmbed)] should contain attribute like #[folder = "asset/"]"#);
    let meta = attribute
        .parse_meta()
        .expect(r#"#[derive(AssetEmbed)] should contain attribute like #[folder = "asset/"]"#);
    let literal_value = match meta {
        Meta::NameValue(ref data) => &data.lit,
        _ => panic!(r#"#[derive(AssetEmbed)] should contain attribute like #[folder = "asset/"]"#),
    };
    let folder_path = match literal_value {
        Lit::Str(ref val) => val.clone().value(),
        _ => panic!(r#"#[derive(AssetEmbed)] attribute value must be a string literal"#),
    };

    let folder_path = if Path::new(&folder_path).is_relative() {
        Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap())
            .join(folder_path)
            .to_str()
            .unwrap()
            .to_owned()
    } else {
        folder_path
    };

    if !Path::new(&folder_path).exists() {
        panic!(
            "#[derive(AssetEmbed)] folder '{}' does not exist. cwd: '{}'",
            folder_path,
            std::env::current_dir().unwrap().to_str().unwrap()
        );
    }

    generate_assets(&ast.ident, folder_path)
}

#[proc_macro_derive(AssetEmbed, attributes(folder))]
pub fn derive_input_object(input: TokenStream) -> TokenStream {
    let ast: DeriveInput = syn::parse(input).unwrap();
    let gen = impl_asset_embed(&ast);
    gen.into()
}
