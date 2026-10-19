#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Document {
    pub children: Vec<Node>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Node {
    Doctype(String),
    Comment(String),
    Text(String),
    Element(Element),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Element {
    pub name: String,
    pub attributes: Vec<Attribute>,
    pub children: Vec<Node>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Attribute {
    pub name: String,
    pub value: String,
}

impl Document {
    pub fn parse(input: &str) -> Self {
        let mut document = Self::default();
        let mut open_elements = Vec::new();

        for token in tokenize(input) {
            match token {
                Token::Doctype(name) => {
                    append_node(&mut document, &mut open_elements, Node::Doctype(name))
                }
                Token::Comment(comment) => {
                    append_node(&mut document, &mut open_elements, Node::Comment(comment))
                }
                Token::Text(text) if !text.is_empty() => {
                    append_node(&mut document, &mut open_elements, Node::Text(text))
                }
                Token::Text(_) => {}
                Token::StartTag {
                    name,
                    attributes,
                    self_closing,
                } => {
                    if self_closing || is_void_element(&name) {
                        append_node(
                            &mut document,
                            &mut open_elements,
                            Node::Element(Element {
                                name,
                                attributes,
                                children: Vec::new(),
                            }),
                        );
                    } else {
                        open_elements.push(OpenElement {
                            name,
                            attributes,
                            children: Vec::new(),
                        });
                    }
                }
                Token::EndTag(name) => close_element(&mut document, &mut open_elements, &name),
            }
        }

        while let Some(element) = open_elements.pop() {
            append_node(
                &mut document,
                &mut open_elements,
                Node::Element(element.into()),
            );
        }

        document
    }

    pub fn text_content(&self) -> String {
        let mut text = String::new();

        for child in &self.children {
            collect_text(child, &mut text);
        }

        text
    }

    pub fn first_element(&self, name: &str) -> Option<&Element> {
        self.children
            .iter()
            .find_map(|child| find_element(child, name))
    }
}

struct OpenElement {
    name: String,
    attributes: Vec<Attribute>,
    children: Vec<Node>,
}

impl From<OpenElement> for Element {
    fn from(value: OpenElement) -> Self {
        Self {
            name: value.name,
            attributes: value.attributes,
            children: value.children,
        }
    }
}

#[derive(Debug)]
enum Token {
    Doctype(String),
    Comment(String),
    Text(String),
    StartTag {
        name: String,
        attributes: Vec<Attribute>,
        self_closing: bool,
    },
    EndTag(String),
}

fn append_node(document: &mut Document, open_elements: &mut [OpenElement], node: Node) {
    if let Some(parent) = open_elements.last_mut() {
        parent.children.push(node);
    } else {
        document.children.push(node);
    }
}

fn close_element(document: &mut Document, open_elements: &mut Vec<OpenElement>, name: &str) {
    let Some(index) = open_elements
        .iter()
        .rposition(|element| element.name == name)
    else {
        return;
    };

    while open_elements.len() > index {
        let element = open_elements.pop().expect("index belongs to open elements");
        append_node(document, open_elements, Node::Element(element.into()));
    }
}

fn collect_text(node: &Node, output: &mut String) {
    match node {
        Node::Text(text) => output.push_str(text),
        Node::Element(element) => {
            for child in &element.children {
                collect_text(child, output);
            }
        }
        Node::Doctype(_) | Node::Comment(_) => {}
    }
}

fn find_element<'a>(node: &'a Node, name: &str) -> Option<&'a Element> {
    let Node::Element(element) = node else {
        return None;
    };

    if element.name == name {
        return Some(element);
    }

    element
        .children
        .iter()
        .find_map(|child| find_element(child, name))
}

fn is_void_element(name: &str) -> bool {
    matches!(
        name,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

fn tokenize(input: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut cursor = 0;

    while cursor < input.len() {
        let remaining = &input[cursor..];

        if remaining.starts_with("<!--") {
            let content_start = cursor + 4;

            if let Some(end_offset) = input[content_start..].find("-->") {
                let end = content_start + end_offset;
                tokens.push(Token::Comment(input[content_start..end].to_owned()));
                cursor = end + 3;
            } else {
                tokens.push(Token::Comment(input[content_start..].to_owned()));
                break;
            }

            continue;
        }

        if remaining.starts_with("<!") {
            let content_start = cursor + 2;

            if let Some(end_offset) = input[content_start..].find('>') {
                let end = content_start + end_offset;
                let content = input[content_start..end].trim();

                if content.len() >= 7 && content[..7].eq_ignore_ascii_case("doctype") {
                    tokens.push(Token::Doctype(content[7..].trim().to_ascii_lowercase()));
                    cursor = end + 1;
                    continue;
                }
            }
        }

        if remaining.starts_with("</") {
            if let Some((name, next_cursor)) = parse_end_tag(input, cursor) {
                tokens.push(Token::EndTag(name));
                cursor = next_cursor;
                continue;
            }
        }

        if input.as_bytes()[cursor] == b'<' {
            if let Some((token, next_cursor)) = parse_start_tag(input, cursor) {
                tokens.push(token);
                cursor = next_cursor;
                continue;
            }
        }

        let next = input[cursor + 1..]
            .find('<')
            .map(|offset| cursor + offset + 1)
            .unwrap_or(input.len());

        tokens.push(Token::Text(decode_character_references(
            &input[cursor..next],
        )));
        cursor = next;
    }

    tokens
}

fn parse_end_tag(input: &str, start: usize) -> Option<(String, usize)> {
    let mut cursor = start + 2;
    skip_ascii_whitespace(input, &mut cursor);

    let name = take_name(input, &mut cursor)?;
    skip_ascii_whitespace(input, &mut cursor);

    (input.as_bytes().get(cursor) == Some(&b'>')).then(|| (name, cursor + 1))
}

fn parse_start_tag(input: &str, start: usize) -> Option<(Token, usize)> {
    let mut cursor = start + 1;

    if !input.as_bytes().get(cursor)?.is_ascii_alphabetic() {
        return None;
    }

    let name = take_name(input, &mut cursor)?;
    let mut attributes = Vec::new();

    loop {
        skip_ascii_whitespace(input, &mut cursor);

        match input.as_bytes().get(cursor) {
            Some(b'>') => {
                return Some((
                    Token::StartTag {
                        name,
                        attributes,
                        self_closing: false,
                    },
                    cursor + 1,
                ));
            }
            Some(b'/') if input.as_bytes().get(cursor + 1) == Some(&b'>') => {
                return Some((
                    Token::StartTag {
                        name,
                        attributes,
                        self_closing: true,
                    },
                    cursor + 2,
                ));
            }
            Some(_) => {}
            None => return None,
        }

        let attribute_name = take_attribute_name(input, &mut cursor)?;
        skip_ascii_whitespace(input, &mut cursor);

        let value = if input.as_bytes().get(cursor) == Some(&b'=') {
            cursor += 1;
            skip_ascii_whitespace(input, &mut cursor);
            parse_attribute_value(input, &mut cursor)?
        } else {
            String::new()
        };

        if !attributes
            .iter()
            .any(|attribute: &Attribute| attribute.name == attribute_name)
        {
            attributes.push(Attribute {
                name: attribute_name,
                value,
            });
        }
    }
}

fn take_name(input: &str, cursor: &mut usize) -> Option<String> {
    let start = *cursor;

    while let Some(byte) = input.as_bytes().get(*cursor) {
        if byte.is_ascii_whitespace() || matches!(byte, b'/' | b'>') {
            break;
        }

        *cursor += 1;
    }

    (*cursor > start).then(|| input[start..*cursor].to_ascii_lowercase())
}

fn take_attribute_name(input: &str, cursor: &mut usize) -> Option<String> {
    let start = *cursor;

    while let Some(byte) = input.as_bytes().get(*cursor) {
        if byte.is_ascii_whitespace() || matches!(byte, b'/' | b'>' | b'=') {
            break;
        }

        *cursor += 1;
    }

    (*cursor > start).then(|| input[start..*cursor].to_ascii_lowercase())
}

fn parse_attribute_value(input: &str, cursor: &mut usize) -> Option<String> {
    let quote = input.as_bytes().get(*cursor).copied();

    if matches!(quote, Some(b'"' | b'\'')) {
        *cursor += 1;
        let start = *cursor;

        while input.as_bytes().get(*cursor) != quote.as_ref() {
            input.as_bytes().get(*cursor)?;
            *cursor += 1;
        }

        let value = decode_character_references(&input[start..*cursor]);
        *cursor += 1;
        return Some(value);
    }

    let start = *cursor;

    while let Some(byte) = input.as_bytes().get(*cursor) {
        if byte.is_ascii_whitespace() || *byte == b'>' {
            break;
        }

        *cursor += 1;
    }

    Some(decode_character_references(&input[start..*cursor]))
}

fn skip_ascii_whitespace(input: &str, cursor: &mut usize) {
    while input
        .as_bytes()
        .get(*cursor)
        .is_some_and(|byte| byte.is_ascii_whitespace())
    {
        *cursor += 1;
    }
}

fn decode_character_references(input: &str) -> String {
    let mut output = String::new();
    let mut cursor = 0;

    while let Some(offset) = input[cursor..].find('&') {
        let ampersand = cursor + offset;
        output.push_str(&input[cursor..ampersand]);

        if let Some((decoded, length)) = decode_reference(&input[ampersand..]) {
            output.push(decoded);
            cursor = ampersand + length;
        } else {
            output.push('&');
            cursor = ampersand + 1;
        }
    }

    output.push_str(&input[cursor..]);
    output
}

fn decode_reference(input: &str) -> Option<(char, usize)> {
    for (name, character) in [
        ("&amp;", '&'),
        ("&lt;", '<'),
        ("&gt;", '>'),
        ("&quot;", '"'),
        ("&apos;", '\''),
        ("&nbsp;", '\u{00A0}'),
    ] {
        if input.starts_with(name) {
            return Some((character, name.len()));
        }
    }

    let digits = input.strip_prefix("&#")?;
    let (radix, digits) = if let Some(hexadecimal) = digits.strip_prefix(['x', 'X']) {
        (16, hexadecimal)
    } else {
        (10, digits)
    };

    let end = digits.find(';')?;
    let value = u32::from_str_radix(&digits[..end], radix).ok()?;

    let character = match value {
        0 | 0xD800..=0xDFFF | 0x11_0000.. => '\u{FFFD}',
        _ => char::from_u32(value).unwrap_or('\u{FFFD}'),
    };

    let prefix_length = input.len() - digits.len();

    Some((character, prefix_length + end + 1))
}

#[cfg(test)]
mod tests {
    use super::{Document, Node};

    #[test]
    fn builds_a_nested_dom_and_decodes_text() {
        let document =
            Document::parse(r#"<!doctype HTML><p class=lead>Hello &amp; <b>world</b><br>!</p>"#);

        assert_eq!(document.text_content(), "Hello & world!");

        let paragraph = document.first_element("p").unwrap();
        assert_eq!(paragraph.attributes[0].name, "class");
        assert_eq!(paragraph.attributes[0].value, "lead");
        assert!(matches!(paragraph.children[1], Node::Element(_)));
    }

    #[test]
    fn handles_mismatched_end_tags_like_an_html_tree_builder() {
        let document = Document::parse("<p>one <b>two</p>three");

        assert_eq!(document.text_content(), "one twothree");

        let paragraph = document.first_element("p").unwrap();
        assert_eq!(paragraph.children.len(), 2);
    }

    #[test]
    fn parses_quoted_unquoted_and_boolean_attributes() {
        let document =
            Document::parse(r#"<input disabled data-a="one &amp; two" data-b='three' checked>"#);

        let input = document.first_element("input").unwrap();

        assert_eq!(input.attributes[0].value, "");
        assert_eq!(input.attributes[1].value, "one & two");
        assert_eq!(input.attributes[2].value, "three");
        assert_eq!(input.attributes[3].name, "checked");
    }
}
