//! Generates `ErrorCode::ALL` and `ErrorCode::description()` from the enum's own doc comments.

use std::fmt::Write;

const SOURCE: &str = "src/phases/validation/error.rs";

fn doc_of(attrs: &[syn::Attribute]) -> String {
    attrs
        .iter()
        .filter(|a| a.path().is_ident("doc"))
        .filter_map(|a| match &a.meta {
            syn::Meta::NameValue(nv) => match &nv.value {
                syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(s),
                    ..
                }) => Some(s.value().trim().to_string()),
                _ => None,
            },
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

fn main() {
    println!("cargo:rerun-if-changed={SOURCE}");
    let source = std::fs::read_to_string(SOURCE).expect("read error.rs");
    let file = syn::parse_file(&source).expect("parse error.rs");
    let codes = file
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Enum(e) if e.ident == "ErrorCode" => Some(e),
            _ => None,
        })
        .expect("enum ErrorCode");

    let mut all = String::new();
    let mut arms = String::new();
    for v in &codes.variants {
        let doc = doc_of(&v.attrs);
        writeln!(all, "        ErrorCode::{},", v.ident).unwrap();
        writeln!(arms, "            ErrorCode::{} => {:?},", v.ident, doc).unwrap();
    }
    let out = format!(
        "impl ErrorCode {{\n    /// Every code, in declaration order.\n    pub const ALL: &'static [ErrorCode] = &[\n{all}    ];\n\n    /// The code's doc comment.\n    pub fn description(&self) -> &'static str {{\n        match self {{\n{arms}        }}\n    }}\n}}\n"
    );
    let dest = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("error_code_docs.rs");
    std::fs::write(dest, out).expect("write error_code_docs.rs");
}
