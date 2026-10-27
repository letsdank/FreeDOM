//! Font parsing, container decoding, layout, and rendering.
//!
//! WOFF containers are normalized into SFNT data before the font tables are
//! parsed. WOFF 1.0 uses zlib compression; WOFF 2.0 uses Brotli compression
//! and can additionally transform individual font tables.
//!
//! Specifications:
//! - WOFF 1.0: <https://www.w3.org/TR/WOFF/>
//! - WOFF 2.0: <https://www.w3.org/TR/WOFF2/>
//! - OpenType: <https://learn.microsoft.com/en-us/typography/opentype/spec/>
pub mod brotli;
pub mod cmap;
pub mod glyf;
pub mod gpos;
pub mod hmtx;
pub mod kern;
pub mod layout;
pub mod loca;
pub(crate) mod reader;
pub mod render;
pub mod sfnt;
pub mod truetype;
pub mod woff;
pub mod woff2;

pub(crate) use self::{
    layout::TextOutline, render::rasterize_text_outline, truetype::TrueTypeFont,
    woff::normalize_font_container,
};
