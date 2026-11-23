//! A deliberately small WAT assembler for Wasm tests.
//!
//! Supports function modules with numeric parameters, results and locals,
//! inline exports, standalone function types, and selected instructions.
//! This is not a general WAT parser or a module validator. Tests of malformed
//! binaries should use explicit bytes instead.
//!
//! <https://webassembly.github.io/spec/core/text/modules.html>
//! <https://webassembly.github.io/spec/core/binary/modules.html>

#[derive(Default)]
struct Function {
    params: Vec<u8>,
    results: Vec<u8>,
    locals: Vec<u8>,
    exports: Vec<String>,
    instructions: Vec<u8>,
}

#[derive(Clone)]
struct Tokens<'a> {
    source: &'a str,
    cursor: usize,
}

impl<'a> Tokens<'a> {
    fn new(source: &'a str) -> Self {
        Self { source, cursor: 0 }
    }

    fn next(&mut self) -> Option<&'a str> {
        let source = self.source.as_bytes();
        while self.cursor < source.len() && source[self.cursor].is_ascii_whitespace() {
            self.cursor += 1;
        }
        let start = self.cursor;
        let first = *source.get(start)?;
        if matches!(first, b'(' | b')') {
            self.cursor += 1;
        } else if first == b'"' {
            self.cursor += 1;
            while self.cursor < source.len() && source[self.cursor] != b'"' {
                assert_ne!(
                    source[self.cursor], b'\\',
                    "WAT string escapes are not supported"
                );
                self.cursor += 1;
            }
            assert!(self.cursor < source.len(), "unterminated WAT string");
            self.cursor += 1;
        } else {
            while self.cursor < source.len()
                && !source[self.cursor].is_ascii_whitespace()
                && !matches!(source[self.cursor], b'(' | b')')
            {
                self.cursor += 1;
            }
        }
        Some(&self.source[start..self.cursor])
    }

    fn expect(&mut self, expected: &str) {
        assert_eq!(
            self.next(),
            Some(expected),
            "expected WAT token {expected:?}"
        );
    }
}

fn value_type(token: &str) -> u8 {
    match token {
        "i32" => 0x7f,
        "i64" => 0x7e,
        "f32" => 0x7d,
        "f64" => 0x7c,
        _ => panic!("unsupported WAT value type {token:?}"),
    }
}

fn u32_leb(mut value: u32, out: &mut Vec<u8>) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}

fn i32_leb(mut value: i32, out: &mut Vec<u8>) {
    loop {
        let mut byte = (value as u8) & 0x7f;
        value >>= 7;
        let done = (value == 0 && byte & 0x40 == 0) || (value == -1 && byte & 0x40 != 0);
        if !done {
            byte |= 0x80;
        }
        out.push(byte);
        if done {
            break;
        }
    }
}

fn count(value: usize, out: &mut Vec<u8>) {
    u32_leb(u32::try_from(value).expect("WAT test input too large"), out);
}

fn section(module: &mut Vec<u8>, id: u8, content: &[u8]) {
    module.push(id);
    count(content.len(), module);
    module.extend_from_slice(content);
}

fn instruction(token: &str, tokens: &mut Tokens<'_>, out: &mut Vec<u8>) {
    match token {
        "if" => {
            out.push(0x04);
            if tokens.clone().next() == Some("(") {
                tokens.expect("(");
                tokens.expect("result");
                tokens.expect("i32");
                tokens.expect(")");
                out.push(0x7f);
            } else {
                out.push(0x40);
            }
        }
        "else" => out.push(0x05),
        "end" => out.push(0x0b),
        "i32.const" => {
            out.push(0x41);
            i32_leb(
                tokens
                    .next()
                    .expect("missing i32 constant")
                    .parse()
                    .expect("invalid i32 constant"),
                out,
            );
        }
        "local.get" | "local.set" | "local.tee" => {
            out.push(match token {
                "local.get" => 0x20,
                "local.set" => 0x21,
                _ => 0x22,
            });
            u32_leb(
                tokens
                    .next()
                    .expect("missing local index")
                    .parse()
                    .expect("invalid local index"),
                out,
            );
        }
        "i32.eqz" => out.push(0x45),
        "i32.eq" => out.push(0x46),
        "i32.ne" => out.push(0x47),
        "i32.lt_s" => out.push(0x48),
        "i32.lt_u" => out.push(0x49),
        "i32.gt_s" => out.push(0x4a),
        "i32.gt_u" => out.push(0x4b),
        "i32.le_s" => out.push(0x4c),
        "i32.le_u" => out.push(0x4d),
        "i32.ge_s" => out.push(0x4e),
        "i32.ge_u" => out.push(0x4f),
        "i32.add" => out.push(0x6a),
        "i32.sub" => out.push(0x6b),
        "i32.mul" => out.push(0x6c),
        "i32.div_s" => out.push(0x6d),
        "i32.div_u" => out.push(0x6e),
        "i32.rem_s" => out.push(0x6f),
        "i32.rem_u" => out.push(0x70),
        _ => panic!("unsupported WAT instruction {token:?}"),
    }
}

/// Assembles an instruction sequence, including the final end.
pub(super) fn expression(source: &str) -> Vec<u8> {
    let mut tokens = Tokens::new(source);
    let mut out = Vec::new();
    while let Some(token) = tokens.next() {
        instruction(token, &mut tokens, &mut out);
    }
    out.push(0x0b);
    out
}

/// Assembles the supported WAT subset without validating function semantics.
pub(super) fn module(source: &str) -> Vec<u8> {
    let mut tokens = Tokens::new(source);
    tokens.expect("(");
    tokens.expect("module");
    let mut functions = Vec::new();
    let mut signatures: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    loop {
        match tokens.next().expect("unterminated module") {
            ")" => break,
            "(" => {
                let kind = tokens.next().expect("missing module declaration");
                if kind == "type" {
                    assert!(
                        functions.is_empty(),
                        "mixed explicit types and functions are unsupported"
                    );
                    tokens.expect("(");
                    tokens.expect("func");
                    let (mut params, mut results) = (Vec::new(), Vec::new());
                    loop {
                        match tokens.next().expect("unterminated function type") {
                            ")" => break,
                            "(" => {
                                let values = match tokens.next() {
                                    Some("param") => &mut params,
                                    Some("result") => &mut results,
                                    other => panic!("unsupported type declaration {other:?}"),
                                };
                                loop {
                                    let token =
                                        tokens.next().expect("unterminated type declaration");
                                    if token == ")" {
                                        break;
                                    }
                                    values.push(value_type(token));
                                }
                            }
                            token => panic!("unexpected function type token {token:?}"),
                        }
                    }
                    tokens.expect(")");
                    signatures.push((params, results));
                    continue;
                }
                assert_eq!(kind, "func", "unsupported module declaration");
                assert!(
                    signatures.is_empty(),
                    "mixed explicit types and functions are unsupported"
                );
                let mut function = Function::default();
                loop {
                    match tokens.next().expect("unterminated function") {
                        ")" => break,
                        "(" => match tokens.next().expect("missing function declaration") {
                            "export" => {
                                let name = tokens.next().expect("missing export name");
                                assert!(
                                    name.starts_with('"') && name.ends_with('"'),
                                    "invalid export name"
                                );
                                function.exports.push(name[1..name.len() - 1].to_owned());
                                tokens.expect(")");
                            }
                            kind @ ("param" | "result" | "local") => {
                                let values = match kind {
                                    "param" => &mut function.params,
                                    "result" => &mut function.results,
                                    _ => &mut function.locals,
                                };
                                loop {
                                    let token = tokens.next().expect("unterminated declaration");
                                    if token == ")" {
                                        break;
                                    }
                                    values.push(value_type(token));
                                }
                            }
                            kind => panic!("unsupported WAT declaration {kind:?}"),
                        },
                        token => instruction(token, &mut tokens, &mut function.instructions),
                    }
                }
                function.instructions.push(0x0b);
                functions.push(function);
            }
            token => panic!("unexpected module token {token:?}"),
        }
    }
    assert!(tokens.next().is_none(), "trailing tokens after WAT module");

    let mut output = b"\0asm\x01\0\0\0".to_vec();
    if functions.is_empty() && signatures.is_empty() {
        return output;
    }
    let mut indices = Vec::new();
    for f in &functions {
        let signature = (f.params.clone(), f.results.clone());
        let index = signatures
            .iter()
            .position(|s| *s == signature)
            .unwrap_or_else(|| {
                signatures.push(signature);
                signatures.len() - 1
            });
        indices.push(index);
    }
    let mut types = Vec::new();
    count(signatures.len(), &mut types);
    for (params, results) in signatures {
        types.push(0x60);
        count(params.len(), &mut types);
        types.extend(params);
        count(results.len(), &mut types);
        types.extend(results);
    }
    section(&mut output, 1, &types);
    if functions.is_empty() {
        return output;
    }
    let mut declarations = Vec::new();
    count(indices.len(), &mut declarations);
    for index in indices {
        count(index, &mut declarations);
    }
    section(&mut output, 3, &declarations);
    let exports_count: usize = functions.iter().map(|f| f.exports.len()).sum();
    if exports_count != 0 {
        let mut exports = Vec::new();
        count(exports_count, &mut exports);
        for (index, f) in functions.iter().enumerate() {
            for name in &f.exports {
                count(name.len(), &mut exports);
                exports.extend_from_slice(name.as_bytes());
                exports.push(0);
                count(index, &mut exports);
            }
        }
        section(&mut output, 7, &exports);
    }
    let mut code = Vec::new();
    count(functions.len(), &mut code);
    for f in functions {
        let mut body = Vec::new();
        let mut groups: Vec<(u32, u8)> = Vec::new();
        for local in f.locals {
            match groups.last_mut() {
                Some((n, ty)) if *ty == local => *n += 1,
                _ => groups.push((1, local)),
            }
        }
        count(groups.len(), &mut body);
        for (n, ty) in groups {
            u32_leb(n, &mut body);
            body.push(ty);
        }
        body.extend(f.instructions);
        count(body.len(), &mut code);
        code.extend(body);
    }
    section(&mut output, 10, &code);
    output
}

#[cfg(test)]
mod tests {
    use super::{expression, module};
    use crate::wasm::{binary::scan_module, code::decode_bodies, decode::decode_declarations};

    #[test]
    fn assembles_known_module_and_leb_boundaries() {
        let wat = r#"(module (func (export "f") (result i32)
            i32.const 20 i32.const 22 i32.add))"#;
        assert_eq!(module(wat),
        b"\0asm\x01\0\0\0\x01\x05\x01\x60\x00\x01\x7f\x03\x02\x01\x00\x07\x05\x01\x01f\x00\x00\x0a\x09\x01\x07\x00\x41\x14\x41\x16\x6a\x0b");
        assert_eq!(
            expression("i32.const -2147483648 local.get 4294967295"),
            [
                0x41, 0x80, 0x80, 0x80, 0x80, 0x78, 0x20, 0xff, 0xff, 0xff, 0xff, 0x0f, 0x0b
            ]
        );
        assert_eq!(
            expression("i32.const 1 if (result i32) i32.const 2 else i32.const 3 end"),
            [0x41, 1, 0x04, 0x7f, 0x41, 2, 0x05, 0x41, 3, 0x0b, 0x0b]
        );
    }

    #[test]
    #[should_panic(expected = "unsupported WAT instruction")]
    fn rejects_unknown_instructions() {
        expression("i32.foobar");
    }

    #[test]
    fn encodes_multi_byte_lengths_and_reuses_signatures() {
        let mut wat = String::from("(module ");
        for _ in 0..130 {
            wat.push_str("(func (param i32) (result i32) local.get 0) ");
        }
        wat.push(')');
        let bytes = module(&wat);
        let framed = scan_module(&bytes).unwrap();
        let declarations = decode_declarations(&framed).unwrap();
        assert_eq!(declarations.types.len(), 1);
        assert_eq!(declarations.function_types.len(), 130);
        assert!(declarations.function_types.iter().all(|index| *index == 0));
        assert_eq!(decode_bodies(&framed).unwrap().len(), 130);
    }
}
