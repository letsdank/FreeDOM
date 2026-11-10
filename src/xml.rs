//! Partial XML lexer. This is not an XML parser or a well-formedness validator.
//!
//! XML 1.0: <https://www.w3.org/TR/xml/#sec-cdata-sect>
//! External identifiers are only tokenized; no DTD or entity is loaded.

const MAX_INPUT_BYTES: usize = 1024 * 1024;
const MAX_TOKENS: usize = 100_000;

#[derive(Debug, PartialEq, Eq)]
enum Token<'a> {
    Text(&'a str),
    StartTag(&'a str),
    EndTag(&'a str),
    Comment(&'a str),
    Cdata(&'a str),
    ProcessingInstruction(&'a str),
    Doctype(&'a str),
}

#[derive(Debug, PartialEq, Eq)]
struct XmlError {
    offset: usize,
    message: &'static str,
}

fn tokenize(source: &str) -> Result<Vec<Token<'_>>, XmlError> {
    if source.len() > MAX_INPUT_BYTES {
        return Err(XmlError {
            offset: 0,
            message: "XML input limit exceeded",
        });
    }

    let mut output = Vec::new();
    let mut cursor = 0;

    while cursor < source.len() {
        if output.len() == MAX_TOKENS {
            return Err(XmlError {
                offset: cursor,
                message: "XML token limit exceeded",
            });
        }

        let remaining = &source[cursor..];

        if !remaining.starts_with('<') {
            let end = remaining
                .find('<')
                .map_or(source.len(), |index| cursor + index);

            let text = &source[cursor..end];

            if let Some(index) = text.find("]]>") {
                return Err(XmlError {
                    offset: cursor + index,
                    message: "CDATA terminator in XML text",
                });
            }

            output.push(Token::Text(text));
            cursor = end;
            continue;
        }

        let (token, next) = if remaining.starts_with("<!--") {
            let start = cursor + 4;
            let end = find_end(source, start, "-->")?;
            let comment = &source[start..end];

            if comment.contains("--") || comment.ends_with('-') {
                return Err(XmlError {
                    offset: start,
                    message: "invalid XML comment",
                });
            }

            (Token::Comment(comment), end + 3)
        } else if remaining.starts_with("<![CDATA[") {
            let start = cursor + 9;
            let end = find_end(source, start, "]]>")?;

            (Token::Cdata(&source[start..end]), end + 3)
        } else if remaining.starts_with("<?") {
            let start = cursor + 2;
            let end = find_end(source, start, "?>")?;

            (Token::ProcessingInstruction(&source[start..end]), end + 2)
        } else if remaining.starts_with("<!DOCTYPE")
            && remaining
                .as_bytes()
                .get(9)
                .is_some_and(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
        {
            let start = cursor + 9;
            let end = find_markup_end(source, start, true)?;

            (Token::Doctype(&source[start..end]), end + 1)
        } else if remaining.starts_with("<!") {
            return Err(XmlError {
                offset: cursor,
                message: "unsupported XML declaration",
            });
        } else if remaining.starts_with("</") {
            let start = cursor + 2;
            let end = find_markup_end(source, start, false)?;

            if source[start..end].trim().is_empty() {
                return Err(XmlError {
                    offset: cursor,
                    message: "empty XML end tag",
                });
            }

            (Token::EndTag(&source[start..end]), end + 1)
        } else {
            let start = cursor + 1;
            let end = find_markup_end(source, start, false)?;

            if source[start..end].trim().is_empty() {
                return Err(XmlError {
                    offset: cursor,
                    message: "empty XML start tag",
                });
            }

            (Token::StartTag(&source[start..end]), end + 1)
        };

        output.push(token);
        cursor = next;
    }

    Ok(output)
}

fn find_end(source: &str, start: usize, marker: &str) -> Result<usize, XmlError> {
    source[start..]
        .find(marker)
        .map(|index| start + index)
        .ok_or(XmlError {
            offset: start,
            message: "unterminated XML construct",
        })
}

fn find_markup_end(source: &str, start: usize, doctype: bool) -> Result<usize, XmlError> {
    let mut quote = None;

    for (offset, &byte) in source.as_bytes()[start..].iter().enumerate() {
        let position = start + offset;

        if let Some(delimiter) = quote {
            if byte == delimiter {
                quote = None;
            }

            continue;
        }

        match byte {
            b'\'' | b'"' => {
                quote = Some(byte);
            }

            b'[' if doctype => {
                return Err(XmlError {
                    offset: position,
                    message: "XML internal DTD subsets are not supported",
                });
            }

            b'<' => {
                return Err(XmlError {
                    offset: position,
                    message: "unexpected '<' inside XML markup",
                });
            }

            b'>' => return Ok(position),

            _ => {}
        }
    }

    Err(XmlError {
        offset: start,
        message: "unterminated XML markup",
    })
}

/// A restricted XML syntax tree, not yet the browser DOM.
#[derive(Debug, PartialEq, Eq)]
struct Document {
    doctype: Option<String>,
    root: Element,
}

#[derive(Debug, PartialEq, Eq)]
struct Element {
    name: String,
    attributes: Vec<(String, String)>,
    children: Vec<Node>,
}

#[derive(Debug, PartialEq, Eq)]
enum Node {
    Element(Element),
    Text(String),
    Comment(String),
}

const MAX_DEPTH: usize = 256;

// This parser accepts an ASCII-name subset. It does not resolve namespaces,
// external DTDs or custom entities, and is not a full XML validator.
// https://www.w3.org/TR/xml/#sec-starttags
fn parse(source: &str) -> Result<Document, XmlError> {
    if source.len() > MAX_INPUT_BYTES {
        return Err(XmlError {
            offset: 0,
            message: "XML input limit exceeded",
        });
    }

    for (offset, ch) in source.char_indices() {
        if !valid_xml_char(ch) {
            return Err(XmlError {
                offset,
                message: "invalid XML character",
            });
        }
    }

    let normalized = source.replace("\r\n", "\n").replace('\r', "\n");

    let mut root = None;
    let mut doctype = None;
    let mut stack: Vec<Element> = Vec::new();

    for token in tokenize(&normalized)? {
        let raw = match &token {
            Token::Text(raw)
            | Token::StartTag(raw)
            | Token::EndTag(raw)
            | Token::Comment(raw)
            | Token::Cdata(raw)
            | Token::ProcessingInstruction(raw)
            | Token::Doctype(raw) => *raw,
        };

        let offset = (raw.as_ptr() as usize).saturating_sub(normalized.as_ptr() as usize);

        let error = |message| XmlError { offset, message };

        match token {
            Token::StartTag(raw) => {
                if stack.len() >= MAX_DEPTH {
                    return Err(error("XML depth limit exceeded"));
                }

                let (element, self_closing) = parse_start_tag(raw).map_err(error)?;

                if stack.is_empty() && root.is_some() {
                    return Err(error("multiple XML root elements"));
                }

                if self_closing {
                    append_element(element, &mut stack, &mut root);
                } else {
                    stack.push(element);
                }
            }

            Token::EndTag(raw) => {
                let name = raw.trim_end_matches(xml_whitespace);

                let (parsed, tail) = read_name(name).ok_or_else(|| error("invalid XML end tag"))?;

                if !tail.is_empty() {
                    return Err(error("invalid XML end tag"));
                }

                let element = stack.pop().ok_or_else(|| error("unexpected XML end tag"))?;

                if element.name != parsed {
                    return Err(error("mismatched XMl end tag"));
                }

                append_element(element, &mut stack, &mut root);
            }

            Token::Text(raw) => {
                let text = decode_references(raw, false).map_err(error)?;

                if let Some(parent) = stack.last_mut() {
                    parent.children.push(Node::Text(text));
                } else if !text.chars().all(xml_whitespace) {
                    return Err(error("text outside XML root"));
                }
            }

            Token::Cdata(raw) => {
                let parent = stack
                    .last_mut()
                    .ok_or_else(|| error("CDATA outside XML root"))?;

                parent.children.push(Node::Text(raw.to_owned()));
            }

            Token::Comment(raw) => {
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(Node::Comment(raw.to_owned()));
                }
            }

            Token::ProcessingInstruction(_) => {}

            Token::Doctype(raw) => {
                if doctype.is_some() || root.is_some() || !stack.is_empty() {
                    return Err(error("unexpected XML doctype"));
                }

                doctype = Some(raw.trim().to_owned());
            }
        }
    }

    if !stack.is_empty() {
        return Err(XmlError {
            offset: normalized.len(),
            message: "unclosed XML element",
        });
    }

    let root = root.ok_or(XmlError {
        offset: normalized.len(),
        message: "missing XML root element",
    })?;

    Ok(Document { doctype, root })
}

fn append_element(element: Element, stack: &mut [Element], root: &mut Option<Element>) {
    if let Some(parent) = stack.last_mut() {
        parent.children.push(Node::Element(element));
    } else {
        *root = Some(element);
    }
}

fn xml_whitespace(ch: char) -> bool {
    matches!(ch, ' ' | '\t' | '\n' | '\r')
}

// ASCII-only subset of XML Name. Colon and non-ASCII names are rejected until
// namespace handling and full XML name tables are implemented.
fn read_name(input: &str) -> Option<(&str, &str)> {
    let mut chars = input.char_indices();
    let (_, first) = chars.next()?;

    if !first.is_ascii_alphabetic() && first != '_' {
        return None;
    }

    let end = chars
        .find_map(|(index, ch)| {
            (!(ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))).then_some(index)
        })
        .unwrap_or(input.len());

    Some((&input[..end], &input[end..]))
}

fn parse_start_tag(raw: &str) -> Result<(Element, bool), &'static str> {
    let self_closing = raw.ends_with('/');

    let content = if self_closing {
        &raw[..raw.len() - 1]
    } else {
        raw
    };

    let (name, mut rest) = read_name(content).ok_or("invalid or unsupported XML name")?;

    let mut attributes = Vec::new();

    while !rest.is_empty() {
        let before = rest.len();

        rest = rest.trim_start_matches(xml_whitespace);

        if rest.is_empty() {
            break;
        }

        if rest.len() == before {
            return Err("missing XML attribute separator");
        }

        let (name, tail) = read_name(rest).ok_or("invalid or unsupported XML attribute name")?;

        if attributes.iter().any(|(key, _)| key == name) {
            return Err("duplicate XML attribute");
        }

        rest = tail.trim_start_matches(xml_whitespace);

        rest = rest
            .strip_prefix('=')
            .ok_or("missing XML attribute equals")?
            .trim_start_matches(xml_whitespace);

        let quote = rest
            .chars()
            .next()
            .filter(|ch| matches!(ch, '\'' | '"'))
            .ok_or("unquoted XML attribute")?;

        rest = &rest[1..];

        let end = rest.find(quote).ok_or("unterminated XML attribute")?;
        let raw_value = &rest[..end];

        if raw_value.contains('<') {
            return Err("invalid '<' in XML attribute");
        }

        attributes.push((name.to_owned(), decode_references(raw_value, true)?));

        rest = &rest[end + 1..];
    }

    Ok((
        Element {
            name: name.to_owned(),
            attributes,
            children: Vec::new(),
        },
        self_closing,
    ))
}

fn valid_xml_char(ch: char) -> bool {
    matches!(
        ch as u32,
        0x9
            | 0xA
            | 0xD
            | 0x20..=0xD7FF
            | 0xE000..=0xFFFD
            | 0x10000..=0x10FFFF
    )
}

// Only predefined and numeric XML references; custom entities are rejected.
// https://www.w3.org/TR/xml/#sec-predefined-ent
fn decode_references(input: &str, attribute: bool) -> Result<String, &'static str> {
    let mut result = String::new();
    let mut rest = input;

    while !rest.is_empty() {
        if let Some(tail) = rest.strip_prefix('&') {
            let end = tail.find(';').ok_or("unterminated XML reference")?;
            let name = &tail[..end];

            let ch = match name {
                "amp" => '&',
                "lt" => '<',
                "gt" => '>',
                "quot" => '"',
                "apos" => '\'',

                _ => {
                    let (digits, radix) = if let Some(value) = name.strip_prefix("#x") {
                        (value, 16)
                    } else if let Some(value) = name.strip_prefix('#') {
                        (value, 10)
                    } else {
                        return Err("unsupported XML entity");
                    };

                    if digits.is_empty() || !digits.chars().all(|ch| ch.to_digit(radix).is_some()) {
                        return Err("invalid XML character reference");
                    }

                    let number = u32::from_str_radix(digits, radix)
                        .map_err(|_| "invalid XML character reference")?;

                    char::from_u32(number)
                        .filter(|&ch| valid_xml_char(ch))
                        .ok_or("invalid XML character reference")?
                }
            };

            result.push(ch);
            rest = &tail[end + 1..];
        } else {
            let ch = rest.chars().next().expect("nonempty string");

            result.push(if attribute && matches!(ch, '\t' | '\n' | '\r') {
                ' '
            } else {
                ch
            });

            rest = &rest[ch.len_utf8()..];
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizes_cdata_without_treating_its_markup_as_xml() {
        let source = "<style><![CDATA[a > b && c < d]]></style>";

        assert_eq!(
            tokenize(source).unwrap(),
            vec![
                Token::StartTag("style"),
                Token::Cdata("a > b && c < d"),
                Token::EndTag("style")
            ]
        );
    }

    #[test]
    fn recognizes_doctype_processing_instructions_and_quoted_delimiters() {
        let source = concat!(
            "<?xml version='1.0'?>",
            "<!DOCTYPE html PUBLIC 'a>b' 'system'>",
            "<div title='>'/>",
        );

        assert_eq!(
            tokenize(source).unwrap(),
            vec![
                Token::ProcessingInstruction("xml version='1.0'"),
                Token::Doctype(" html PUBLIC 'a>b' 'system'"),
                Token::StartTag("div title='>'/")
            ]
        );
    }

    #[test]
    fn rejects_unterminated_and_unsupported_constructs() {
        for input in [
            "<![CDATA[no",
            "<!--no",
            "<?xml",
            "<div a='x>",
            "<!DOCTYPE html [<!ENTITY x 'y'>]>",
            "<div <bad>",
            "a]]>b",
            "<!--a--b-->",
        ] {
            assert!(tokenize(input).is_err(), "{input}");
        }
    }

    #[test]
    fn preserves_text_and_entity_references_for_a_later_parser() {
        assert_eq!(
            tokenize("<p>é &amp; &#x41;</p>").unwrap(),
            vec![
                Token::StartTag("p"),
                Token::Text("é &amp; &#x41;"),
                Token::EndTag("p")
            ]
        );
    }

    #[test]
    fn enforces_input_and_token_limits() {
        let oversized = "x".repeat(MAX_INPUT_BYTES + 1);

        assert_eq!(
            tokenize(&oversized).unwrap_err().message,
            "XML input limit exceeded",
        );

        let too_many_tokens = "<x/>".repeat(MAX_TOKENS + 1);

        assert_eq!(
            tokenize(&too_many_tokens).unwrap_err().message,
            "XML token limit exceeded"
        );
    }

    #[test]
    fn builds_xml_tree_with_attributes_references_and_cdata() {
        let document = parse(concat!(
            "<?xml version='1.0'?>",
            "<root a='A&amp;B' b='x&#10;y\nz'>",
            "<item/>",
            "text&#x1F4A1;",
            "<![CDATA[<x>&amp;]]>",
            "</root>"
        ))
        .unwrap();

        assert_eq!(document.root.name, "root");

        assert_eq!(
            document.root.attributes,
            vec![
                ("a".to_owned(), "A&B".to_owned()),
                ("b".to_owned(), "x\ny z".to_owned())
            ]
        );

        assert_eq!(
            document.root.children,
            vec![
                Node::Element(Element {
                    name: "item".to_owned(),
                    attributes: vec![],
                    children: vec![]
                }),
                Node::Text("text💡".to_owned()),
                Node::Text("<x>&amp;".to_owned())
            ]
        );
    }

    #[test]
    fn rejects_invalid_or_unsupported_xml_documents() {
        for input in [
            "<a><b></a></b>",
            "<a/><b/>",
            "<a x='1' x='2' />",
            "<a x=1/>",
            "<a>&unknown;</a>",
            "<a>&#0;</a>",
            "<a>&#xD800;</a>",
            "<a x='<bad'/>",
            "<a:b/>",
            "<a>",
            "text<a/>",
        ] {
            assert!(parse(input).is_err(), "{input:?}");
        }

        let deep = format!(
            "{}{}",
            "<a>".repeat(MAX_DEPTH + 1),
            "</a>".repeat(MAX_DEPTH + 1)
        );

        assert_eq!(
            parse(&deep).unwrap_err().message,
            "XML depth limit exceeded"
        );
    }

    #[test]
    fn parses_the_saved_xhtml_pair_as_xml_trees() {
        fn element<'a>(parent: &'a Element, name: &str) -> &'a Element {
            parent
                .children
                .iter()
                .find_map(|node| match node {
                    Node::Element(child) if child.name == name => Some(child),

                    _ => None,
                })
                .expect("missing element")
        }

        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata/wpt/css/CSS2/margin-padding-clear");

        for filename in ["margin-collapse-003.xht", "margin-collapse-003-ref.xht"] {
            let source =
                std::fs::read_to_string(base.join(filename)).expect("WPT fixture must exist");

            let document = parse(&source).unwrap_or_else(|error| panic!("{filename}: {error:?}"));

            assert_eq!(document.root.name, "html");

            assert!(
                document
                    .doctype
                    .as_deref()
                    .is_some_and(|value| value.starts_with("html PUBLIC"))
            );

            assert!(document.root.attributes.iter().any(|(name, value)| {
                name == "xmlns" && value == "http://www.w3.org/1999/xhtml"
            }));

            let head = element(&document.root, "head");
            let style = element(head, "style");

            let style_text: String = style
                .children
                .iter()
                .filter_map(|node| match node {
                    Node::Text(text) => Some(text.as_str()),
                    _ => None,
                })
                .collect();

            assert!(style_text.contains("div"));
            assert!(!style_text.contains("<![CDATA["));
            assert!(!style_text.contains("]]>"));

            let body = element(&document.root, "body");

            assert!(
                body.children
                    .iter()
                    .any(|node| matches!(node, Node::Element(_)))
            );

            if filename == "margin-collapse-003.xht" {
                assert!(head.children.iter().any(|node| match node {
                    Node::Element(element) if element.name == "link" => {
                        element
                            .attributes
                            .iter()
                            .any(|(name, value)| name == "rel" && value == "match")
                            && element.attributes.iter().any(|(name, value)| {
                                name == "href" && value == "margin-collapse-003-ref.xht"
                            })
                    }

                    _ => false,
                }));
            } else {
                assert!(style_text.contains("border-top: blue solid 20px"));
            }
        }
    }
}
