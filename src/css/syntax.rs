//! Partial CSS declaration parsing.
//!
//! Specification: <https://www.w3.org/TR/css-syntax-3/#consume-declaration>
//!
//! This implementation does not yet tokenize strings, functions, or blocks.

use super::CssDeclaration;

pub(crate) fn parse_declaration_list(source: &str) -> Vec<CssDeclaration> {
    source
        .split(';')
        .filter_map(|declaration| {
            let (name, value) = declaration.split_once(':')?;
            let name = name.trim().to_ascii_lowercase();

            (!name.is_empty()).then_some(CssDeclaration {
                name,
                value: value.trim().to_owned(),
            })
        })
        .collect()
}
