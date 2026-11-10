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
}
