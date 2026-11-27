use super::*;
use crate::graphics::{frame::XrgbFrame, raster::Color};
use crate::render::PaintOptions;
use freedom_css::{FontStyle, FontWeight};
use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
};

mod canvas;
mod document;
mod fonts;
mod inspector;
mod preview_blocks;
mod preview_text;
mod resources;

/// Reads an HTTP request through the end of its header block.
fn read_request_head(stream: &mut std::net::TcpStream) -> String {
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();

    let mut request = Vec::new();
    let mut chunk = [0_u8; 1024];

    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        let count = stream.read(&mut chunk).unwrap();

        assert!(count > 0, "unexpected end of request");

        request.extend_from_slice(&chunk[..count]);

        assert!(request.len() < 16_384, "request headers are too long");
    }

    String::from_utf8(request).unwrap()
}
