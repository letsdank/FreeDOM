use std::{error::Error, fmt};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Url {
    scheme: String,
    host: String,
    port: Option<u16>,
    path: String,
    query: Option<String>,
    fragment: Option<String>,
    username: String,
    password: Option<String>,
    form: UrlForm,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UrlForm {
    Special,
    Authority,
    Opaque,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UrlError {
    EmptyInput,
    InvalidScheme,
    EmptyHost,
    InvalidHost,
    InvalidPort,
}

impl fmt::Display for UrlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyInput => write!(f, "URL is empty"),
            Self::InvalidScheme => write!(f, "invalid URL scheme"),
            Self::EmptyHost => write!(f, "URL host is empty"),
            Self::InvalidHost => write!(f, "invalid URL host"),
            Self::InvalidPort => write!(f, "invalid URL port"),
        }
    }
}

impl Error for UrlError {}

impl Url {
    pub fn parse(input: &str) -> Result<Self, UrlError> {
        let input = sanitize_url_input(input);

        if input.is_empty() {
            return Err(UrlError::EmptyInput);
        }

        let (without_fragment, fragment) = split_component(&input, '#');
        let (without_query, query) = split_component(without_fragment, '?');

        let Some((scheme, after_scheme)) = without_query.split_once(':') else {
            return Err(UrlError::InvalidScheme);
        };

        if !is_valid_scheme(scheme) {
            return Err(UrlError::InvalidScheme);
        }

        let scheme = scheme.to_ascii_lowercase();

        if scheme == "file" {
            return parse_file_url(after_scheme, query, fragment);
        }

        if is_special_scheme(&scheme) {
            let after_scheme = after_scheme.replace('\\', "/");
            let after_authority_marker = after_scheme.trim_start_matches('/');

            let authority_end = after_authority_marker
                .find('/')
                .unwrap_or(after_authority_marker.len());

            let authority = &after_authority_marker[..authority_end];
            let raw_path = &after_authority_marker[authority_end..];

            let (username, password, host, port) = parse_authority(&scheme, authority, true)?;

            return Ok(Self {
                form: UrlForm::Special,
                scheme,
                username,
                password,
                host,
                port,
                path: normalize_path(raw_path),
                query: query.map(str::to_owned),
                fragment: fragment.map(str::to_owned),
            });
        }

        if let Some(after_authority_marker) = after_scheme.strip_prefix("//") {
            let authority_end = after_authority_marker
                .find('/')
                .unwrap_or(after_authority_marker.len());

            let authority = &after_authority_marker[..authority_end];
            let raw_path = &after_authority_marker[authority_end..];

            let (username, password, host, port) = parse_authority(&scheme, authority, false)?;

            return Ok(Self {
                form: UrlForm::Authority,
                scheme,
                username,
                password,
                host,
                port,
                path: raw_path.to_owned(),
                query: query.map(str::to_owned),
                fragment: fragment.map(str::to_owned),
            });
        }

        Ok(Self {
            form: UrlForm::Opaque,
            scheme,
            username: String::new(),
            password: None,
            host: String::new(),
            port: None,
            path: after_scheme.to_owned(),
            query: query.map(str::to_owned),
            fragment: fragment.map(str::to_owned),
        })
    }

    pub fn join(&self, reference: &str) -> Result<Self, UrlError> {
        let reference = sanitize_url_input(reference);

        if self.scheme == "file" {
            return self.join_file_url(&reference);
        }

        if reference.is_empty() {
            let mut resolved = self.clone();
            resolved.fragment = None;
            return Ok(resolved);
        }

        if has_scheme(&reference) {
            let (scheme, after_scheme) = reference
                .split_once(':')
                .expect("has_scheme guarantees ':'");

            if scheme.eq_ignore_ascii_case(&self.scheme) && !after_scheme.starts_with("//") {
                return self.join(after_scheme);
            }

            return Self::parse(&reference);
        }

        let (without_fragment, fragment) = split_component(&reference, '#');
        let special_reference = without_fragment.replace('\\', "/");

        if special_reference.starts_with("//") {
            let absolute = format!("{}:{}", self.scheme, special_reference);
            let mut resolved = Self::parse(&absolute)?;
            resolved.fragment = fragment.map(str::to_owned);
            return Ok(resolved);
        }

        let (reference_path, query) = split_component(&special_reference, '?');

        let mut resolved = self.clone();
        resolved.fragment = fragment.map(str::to_owned);

        if reference_path.is_empty() {
            if let Some(query) = query {
                resolved.query = Some(query.to_owned());
            }

            return Ok(resolved);
        }

        resolved.query = query.map(str::to_owned);

        if reference_path.starts_with('/') {
            resolved.path = normalize_path(reference_path);
        } else {
            let directory = base_directory(&self.path);
            resolved.path = normalize_path(&format!("{directory}{reference_path}"));
        }

        Ok(resolved)
    }

    fn join_file_url(&self, reference: &str) -> Result<Self, UrlError> {
        let reference = sanitize_url_input(reference);

        if reference.is_empty() {
            let mut resolved = self.clone();
            resolved.fragment = None;
            return Ok(resolved);
        }

        let (without_fragment, fragment) = split_component(&reference, '#');
        let (without_query, query) = split_component(&without_fragment, '?');

        if has_scheme(without_query) {
            let (scheme, after_scheme) = without_query
                .split_once(':')
                .expect("has_scheme guarantees ':'");

            if scheme.eq_ignore_ascii_case("file") {
                let absolute_file_reference = after_scheme.starts_with('/')
                    || after_scheme.starts_with('\\')
                    || is_windows_drive_letter(after_scheme);

                if absolute_file_reference {
                    return parse_file_url(after_scheme, query, fragment);
                }

                let mut resolved = self.join_file_url(after_scheme)?;
                resolved.query = query.map(str::to_owned);
                resolved.fragment = fragment.map(str::to_owned);
                return Ok(resolved);
            }

            return Self::parse(&reference);
        }

        let reference_path = without_query.replace('\\', "/");
        let mut resolved = self.clone();
        resolved.fragment = fragment.map(str::to_owned);

        if reference_path.is_empty() {
            if let Some(query) = query {
                resolved.query = Some(query.to_owned());
            }

            return Ok(resolved);
        }

        resolved.query = query.map(str::to_owned);

        if reference_path.starts_with("//") {
            return parse_file_url(&reference_path, query, fragment);
        }

        if is_windows_drive_letter(reference_path.trim_start_matches('/')) {
            resolved.host.clear();
            resolved.path = normalize_file_path(&reference_path);
        } else if reference_path.starts_with('/') {
            resolved.path = normalize_file_path(&reference_path);
        } else {
            let directory = base_directory(&self.path);
            resolved.path = normalize_path(&format!("{directory}{reference_path}"));
        }

        Ok(resolved)
    }

    pub fn scheme(&self) -> &str {
        &self.scheme
    }
}

impl fmt::Display for Url {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.form {
            UrlForm::Special | UrlForm::Authority => {
                write!(f, "{}://", self.scheme)?;

                if !self.username.is_empty() || self.password.is_some() {
                    write!(f, "{}", serialize_userinfo(&self.username))?;

                    if let Some(password) = &self.password {
                        write!(f, ":{}", serialize_userinfo(password))?;
                    }

                    write!(f, "@")?;
                }

                write!(f, "{}", self.host)?;

                if let Some(port) = self.port {
                    write!(f, ":{port}")?;
                }

                if self.form == UrlForm::Special {
                    write!(f, "{}", serialize_component(&self.path))?;
                } else {
                    write!(f, "{}", self.path)?;
                }
            }

            UrlForm::Opaque => {
                let has_query_or_fragment = self.query.is_some() || self.fragment.is_some();

                write!(
                    f,
                    "{}:{}",
                    self.scheme,
                    serialize_opaque_path(&self.path, has_query_or_fragment)
                )?;
            }
        }

        if let Some(query) = &self.query {
            write!(f, "?{}", serialize_component(query))?;
        }

        if let Some(fragment) = &self.fragment {
            write!(f, "#{}", serialize_component(fragment))?;
        }

        Ok(())
    }
}

fn trim_ascii_whitespace(input: &str) -> &str {
    input.trim_matches(|character: char| character.is_ascii_whitespace())
}

fn trim_c0_control_and_space(input: &str) -> &str {
    input.trim_matches(|character| character <= '\u{20}')
}

fn sanitize_url_input(input: &str) -> String {
    trim_c0_control_and_space(input)
        .chars()
        .filter(|character| !matches!(character, '\t' | '\n' | '\r'))
        .collect()
}

fn serialize_component(component: &str) -> String {
    percent_encode(component, |byte| byte == b' ' || byte >= 0x7F)
}

fn serialize_opaque_path(path: &str, has_query_or_fragment: bool) -> String {
    let mut serialized = percent_encode(path, |byte| byte >= 0x7F);

    if has_query_or_fragment && serialized.ends_with(' ') {
        serialized.truncate(serialized.len() - 1);
        serialized.push_str("%20");
    }

    serialized
}

fn serialize_opaque_host(host: &str) -> String {
    percent_encode(host, |byte| {
        byte <= 0x1F
            || byte >= 0x7F
            || matches!(
                byte,
                b' ' | b'"'
                    | b'#'
                    | b'/'
                    | b'<'
                    | b'>'
                    | b'?'
                    | b'@'
                    | b'['
                    | b'\\'
                    | b']'
                    | b'^'
                    | b'|'
            )
    })
}

fn serialize_userinfo(component: &str) -> String {
    percent_encode(component, |byte| {
        byte <= 0x1F
            || byte >= 0x7F
            || matches!(
                byte,
                b' ' | b'"'
                    | b'#'
                    | b'<'
                    | b'>'
                    | b'?'
                    | b'`'
                    | b'/'
                    | b':'
                    | b';'
                    | b'='
                    | b'@'
                    | b'['
                    | b'\\'
                    | b']'
                    | b'^'
                    | b'|'
            )
    })
}

fn percent_encode(input: &str, should_encode: impl Fn(u8) -> bool) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";

    let mut encoded = String::new();

    for byte in input.bytes() {
        if should_encode(byte) {
            encoded.push('%');
            encoded.push(HEX[(byte >> 4) as usize] as char);
            encoded.push(HEX[(byte & 0x0F) as usize] as char);
        } else {
            encoded.push(byte as char);
        }
    }

    encoded
}

fn split_component(input: &str, marker: char) -> (&str, Option<&str>) {
    match input.split_once(marker) {
        Some((before, after)) => (before, Some(after)),
        None => (input, None),
    }
}

fn has_scheme(input: &str) -> bool {
    input
        .find(':')
        .is_some_and(|index| is_valid_scheme(&input[..index]))
}

fn is_valid_scheme(scheme: &str) -> bool {
    let mut characters = scheme.chars();

    match characters.next() {
        Some(character) if character.is_ascii_alphabetic() => {}
        _ => return false,
    }

    characters
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.'))
}

fn is_special_scheme(scheme: &str) -> bool {
    matches!(scheme, "file" | "ftp" | "http" | "https" | "ws" | "wss")
}

fn parse_authority(
    scheme: &str,
    authority: &str,
    host_required: bool,
) -> Result<(String, Option<String>, String, Option<u16>), UrlError> {
    if authority.is_empty() && host_required {
        return Err(UrlError::EmptyHost);
    }

    let (username, password, host_and_port) = match authority.rsplit_once('@') {
        Some((userinfo, host_and_port)) => {
            let (username, password) = match userinfo.split_once(':') {
                Some((username, password)) if !password.is_empty() => {
                    (username.to_owned(), Some(password.to_owned()))
                }

                Some((username, _)) => (username.to_owned(), None),

                None => (userinfo.to_owned(), None),
            };

            (username, password, host_and_port)
        }

        None => (String::new(), None, authority),
    };

    let (host, port_text) = if host_and_port.starts_with('[') {
        let Some(closing_bracket) = host_and_port.find(']') else {
            return Err(UrlError::InvalidHost);
        };

        let host = &host_and_port[..=closing_bracket];
        let suffix = &host_and_port[closing_bracket + 1..];

        if suffix.is_empty() {
            (host, None)
        } else if let Some(port) = suffix.strip_prefix(':') {
            (host, Some(port))
        } else {
            return Err(UrlError::InvalidHost);
        }
    } else {
        match host_and_port.rsplit_once(':') {
            Some((host, port)) if !host.contains(':') => (host, Some(port)),
            Some(_) => return Err(UrlError::InvalidHost),
            None => (host_and_port, None),
        }
    };

    if host_required && host.is_empty() {
        return Err(UrlError::EmptyHost);
    }

    if host.chars().any(|character| {
        character.is_ascii_whitespace() || matches!(character, '/' | '\\' | '?' | '#')
    }) {
        return Err(UrlError::InvalidHost);
    }

    let port = match port_text {
        None | Some("") => None,

        Some(port) => {
            if !port.chars().all(|character| character.is_ascii_digit()) {
                return Err(UrlError::InvalidPort);
            }

            let canonical_port = port.trim_start_matches('0');

            let port = if canonical_port.is_empty() {
                0
            } else {
                canonical_port
                    .parse::<u16>()
                    .map_err(|_| UrlError::InvalidPort)?
            };

            match (scheme, port) {
                ("ftp", 21) | ("http" | "ws", 80) | ("https" | "wss", 443) => None,
                _ => Some(port),
            }
        }
    };

    let host = if is_special_scheme(scheme) {
        canonicalize_special_host(host)?
    } else {
        serialize_opaque_host(host)
    };

    Ok((username, password, host, port))
}

fn preprocess_special_host(host: &str) -> Result<String, UrlError> {
    let decoded = percent_decode_host(host)?;
    let mut mapped = String::new();

    for character in decoded.chars() {
        match character {
            '\u{200B}' | '\u{2060}' | '\u{FEFF}' => {}

            '\u{3002}' | '\u{FF0E}' | '\u{FF61}' => mapped.push('.'),

            '\u{FF01}'..='\u{FF5E}' => {
                let ascii = char::from_u32(character as u32 - 0xFEE0)
                    .expect("fullwidth ASCII always maps to ASCII");
                mapped.push(ascii);
            }

            _ => mapped.push(character),
        }
    }

    if mapped.chars().any(is_forbidden_special_host_character) {
        return Err(UrlError::InvalidHost);
    }

    Ok(mapped)
}

fn percent_decode_host(host: &str) -> Result<String, UrlError> {
    let bytes = host.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] != b'%' {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }

        if index + 2 >= bytes.len() {
            return Err(UrlError::InvalidHost);
        }

        let high = hex_value(bytes[index + 1]).ok_or(UrlError::InvalidHost)?;
        let low = hex_value(bytes[index + 2]).ok_or(UrlError::InvalidHost)?;

        decoded.push((high << 4) | low);
        index += 3;
    }

    String::from_utf8(decoded).map_err(|_| UrlError::InvalidHost)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn is_forbidden_special_host_character(character: char) -> bool {
    let value = character as u32;

    character.is_control()
        || character.is_whitespace()
        || matches!(
            character,
            '%' | '#' | '/' | ':' | '?' | '@' | '[' | '\\' | ']'
        )
        || character == '\u{FFFD}'
        || (0xFDD0..=0xFDEF).contains(&value)
        || value & 0xFFFF == 0xFFFE
        || value & 0xFFFF == 0xFFFF
}

fn canonicalize_special_host(host: &str) -> Result<String, UrlError> {
    if host.is_empty() {
        return Ok(String::new());
    }

    if host.starts_with('[') {
        let Some(address) = host
            .strip_prefix('[')
            .and_then(|address| address.strip_suffix(']'))
        else {
            return Err(UrlError::InvalidHost);
        };

        let address = address
            .parse::<std::net::Ipv6Addr>()
            .map_err(|_| UrlError::InvalidHost)?;

        return Ok(format!("[{address}]"));
    }

    let host = preprocess_special_host(host)?;

    if host.contains('[') || host.contains(']') {
        return Err(UrlError::InvalidHost);
    }

    if let Some(address) = parse_ipv4_address(&host)? {
        return Ok(address);
    }

    host.split('.')
        .map(domain_label_to_ascii)
        .collect::<Result<Vec<_>, _>>()
        .map(|labels| labels.join("."))
}

fn parse_ipv4_address(input: &str) -> Result<Option<String>, UrlError> {
    let mut parts: Vec<&str> = input.split('.').collect();

    if parts.len() > 1 && parts.last() == Some(&"") {
        parts.pop();
    }

    let Some(last_part) = parts.last().copied() else {
        return Ok(None);
    };

    if parse_ipv4_number(last_part)?.is_none() {
        return Ok(None);
    }

    if parts.len() > 4 {
        return Err(UrlError::InvalidHost);
    }

    let mut numbers = Vec::with_capacity(parts.len());

    for part in parts {
        let number = parse_ipv4_number(part)?.ok_or(UrlError::InvalidHost)?;
        numbers.push(number);
    }

    let last_index = numbers.len() - 1;

    if numbers[..last_index].iter().any(|number| *number > 255) {
        return Err(UrlError::InvalidHost);
    }

    let maximum_last_part = 256_u64.pow((5 - numbers.len()) as u32);

    if numbers[last_index] >= maximum_last_part {
        return Err(UrlError::InvalidHost);
    }

    let mut address = numbers[last_index];

    for (index, number) in numbers[..last_index].iter().enumerate() {
        address += number * 256_u64.pow((3 - index) as u32);
    }

    Ok(Some(format!(
        "{}.{}.{}.{}",
        address >> 24,
        (address >> 16) & 0xFF,
        (address >> 8) & 0xFF,
        address & 0xFF
    )))
}

fn parse_ipv4_number(part: &str) -> Result<Option<u64>, UrlError> {
    let Some(first) = part.as_bytes().first().copied() else {
        return Ok(None);
    };

    if !first.is_ascii_digit() {
        return Ok(None);
    }

    let (radix, digits) = if part.starts_with("0x") || part.starts_with("0X") {
        (16, &part[2..])
    } else if part.len() > 1 && part.starts_with('0') {
        (8, &part[1..])
    } else {
        (10, part)
    };

    if digits.is_empty() {
        return Err(UrlError::InvalidHost);
    }

    let valid_digits = match radix {
        8 => digits.bytes().all(|byte| matches!(byte, b'0'..=b'7')),
        10 => digits.bytes().all(|byte| byte.is_ascii_digit()),
        16 => digits.bytes().all(|byte| byte.is_ascii_hexdigit()),
        _ => unreachable!("only supported IPv4 radices are used"),
    };

    if !valid_digits {
        return Err(UrlError::InvalidHost);
    }

    u64::from_str_radix(digits, radix)
        .map(Some)
        .map_err(|_| UrlError::InvalidHost)
}

fn domain_label_to_ascii(label: &str) -> Result<String, UrlError> {
    if label.is_ascii() {
        return Ok(label.to_ascii_lowercase());
    }

    const BASE: u64 = 36;
    const TMIN: u64 = 1;
    const TMAX: u64 = 26;
    const INITIAL_BIAS: u64 = 72;
    const INITIAL_N: u64 = 128;

    let code_points: Vec<u64> = label
        .chars()
        .flat_map(|character| character.to_lowercase())
        .map(|character| character as u32 as u64)
        .collect();

    let mut output = String::new();

    for &point in &code_points {
        if point < 0x80 {
            output.push((point as u8 as char).to_ascii_lowercase());
        }
    }

    let basic_count = output.len() as u64;
    let mut handled = basic_count;

    if basic_count > 0 {
        output.push('-');
    }

    let mut n = INITIAL_N;
    let mut delta = 0_u64;
    let mut bias = INITIAL_BIAS;

    while handled < code_points.len() as u64 {
        let next = code_points
            .iter()
            .copied()
            .filter(|point| *point >= n)
            .min()
            .ok_or(UrlError::InvalidHost)?;

        let weight = handled.checked_add(1).ok_or(UrlError::InvalidHost)?;
        let increment = (next - n)
            .checked_mul(weight)
            .ok_or(UrlError::InvalidHost)?;

        delta = delta.checked_add(increment).ok_or(UrlError::InvalidHost)?;

        n = next;

        for &point in &code_points {
            if point < n {
                delta = delta.checked_add(1).ok_or(UrlError::InvalidHost)?;
            }

            if point != n {
                continue;
            }

            let mut quotient = delta;
            let mut k = BASE;

            loop {
                let threshold = if k <= bias {
                    TMIN
                } else if k >= bias.saturating_add(TMAX) {
                    TMAX
                } else {
                    k - bias
                };

                if quotient < threshold {
                    output.push(punycode_digit(quotient));
                    break;
                }

                let digit = threshold + (quotient - threshold) % (BASE - threshold);
                output.push(punycode_digit(digit));
                quotient = (quotient - threshold) / (BASE - threshold);
                k = k.checked_add(BASE).ok_or(UrlError::InvalidHost)?;
            }

            bias = adapt_punycode_bias(delta, handled + 1, handled == basic_count);
            delta = 0;
            handled += 1;
        }

        delta = delta.checked_add(1).ok_or(UrlError::InvalidHost)?;
        n = n.checked_add(1).ok_or(UrlError::InvalidHost)?;
    }

    Ok(format!("xn--{output}"))
}

fn punycode_digit(value: u64) -> char {
    match value {
        0..=25 => (b'a' + value as u8) as char,
        26..=35 => (b'0' + (value as u8 - 26)) as char,
        _ => unreachable!("Punycode digit must be less than 36"),
    }
}

fn adapt_punycode_bias(mut delta: u64, number_of_points: u64, first_time: bool) -> u64 {
    const BASE: u64 = 36;
    const TMIN: u64 = 1;
    const TMAX: u64 = 26;
    const SKEW: u64 = 38;
    const DAMP: u64 = 700;

    delta = if first_time { delta / DAMP } else { delta / 2 };
    delta += delta / number_of_points;

    let mut k = 0;

    while delta > ((BASE - TMIN) * TMAX) / 2 {
        delta /= BASE - TMIN;
        k += BASE;
    }

    k + ((BASE - TMIN + 1) * delta) / (delta + SKEW)
}

fn parse_file_url(
    after_scheme: &str,
    query: Option<&str>,
    fragment: Option<&str>,
) -> Result<Url, UrlError> {
    let after_scheme = after_scheme.replace('\\', "/");

    let (host, raw_path) = if let Some(after_authority_marker) = after_scheme.strip_prefix("//") {
        let authority_end = after_authority_marker
            .find('/')
            .unwrap_or(after_authority_marker.len());

        let authority = &after_authority_marker[..authority_end];
        let raw_path = &after_authority_marker[authority_end..];

        if authority.contains('%') {
            return Err(UrlError::InvalidHost);
        }

        if is_windows_drive_letter(authority) {
            (String::new(), format!("/{authority}{raw_path}"))
        } else {
            let (username, password, mut host, port) = parse_authority("file", authority, false)?;

            if !username.is_empty() || password.is_some() {
                return Err(UrlError::InvalidHost);
            }

            if port.is_some() {
                return Err(UrlError::InvalidHost);
            }

            if host.eq_ignore_ascii_case("localhost") {
                host.clear();
            }

            (host, raw_path.to_owned())
        }
    } else {
        (String::new(), after_scheme)
    };

    Ok(Url {
        form: UrlForm::Special,
        scheme: "file".to_owned(),
        username: String::new(),
        password: None,
        host,
        port: None,
        path: normalize_file_path(&raw_path),
        query: query.map(str::to_owned),
        fragment: fragment.map(str::to_owned),
    })
}

fn is_windows_drive_letter(input: &str) -> bool {
    let bytes = input.as_bytes();

    bytes.len() >= 2
        && bytes[0].is_ascii_alphabetic()
        && matches!(bytes[1], b':' | b'|')
        && (bytes.len() == 2 || matches!(bytes[2], b'/' | b'\\' | b'?' | b'#'))
}

fn normalize_file_path(raw_path: &str) -> String {
    let mut path = raw_path.replace('\\', "/");

    if !path.starts_with('/') {
        path.insert(0, '/');
    }

    if is_windows_drive_letter(&path[1..]) {
        path.replace_range(2..3, ":");
    }

    normalize_path(&path)
}

fn is_single_dot_segment(segment: &str) -> bool {
    segment == "." || segment.eq_ignore_ascii_case("%2e")
}

fn is_double_dot_segment(segment: &str) -> bool {
    matches!(segment, "..")
        || segment.eq_ignore_ascii_case(".%2e")
        || segment.eq_ignore_ascii_case("%2e.")
        || segment.eq_ignore_ascii_case("%2e%2e")
}

fn normalize_path(raw_path: &str) -> String {
    let path = raw_path.replace('\\', "/");

    if path.is_empty() {
        return "/".to_owned();
    }

    let absolute = path.starts_with('/');
    let raw_segments: Vec<&str> = if absolute {
        path[1..].split('/').collect()
    } else {
        path.split('/').collect()
    };

    let mut segments = Vec::new();

    for (index, segment) in raw_segments.iter().enumerate() {
        let segment = *segment;

        if is_single_dot_segment(segment) {
            if index + 1 == raw_segments.len() {
                segments.push("");
            }
        } else if is_double_dot_segment(segment) {
            segments.pop();

            if index + 1 == raw_segments.len() {
                segments.push("");
            }
        } else {
            segments.push(segment);
        }
    }

    let mut normalized = if absolute {
        "/".to_owned()
    } else {
        String::new()
    };

    normalized.push_str(&segments.join("/"));

    if normalized.is_empty() {
        "/".to_owned()
    } else {
        normalized
    }
}

fn base_directory(path: &str) -> &str {
    match path.rfind('/') {
        Some(index) => &path[..=index],
        None => "/",
    }
}

#[cfg(test)]
mod tests {
    use super::Url;

    const BASE: &str = "http://example.org/foo/bar";

    #[test]
    fn wpt_urltestdata_query_and_fragment_cases() {
        // Cases from:
        // https://github.com/web-platform-tests/wpt/blob/master/url/resources/urltestdata.json
        let base = Url::parse(BASE).unwrap();

        let cases = [
            (":\\", "http://example.org/foo/:/"),
            ("#", "http://example.org/foo/bar#"),
            ("#/", "http://example.org/foo/bar#/"),
            ("#\\", "http://example.org/foo/bar#\\"),
            ("#;?", "http://example.org/foo/bar#;?"),
            ("?", "http://example.org/foo/bar?"),
        ];

        for (input, expected) in cases {
            assert_eq!(base.join(input).unwrap().to_string(), expected, "{input}");
        }
    }

    #[test]
    fn wpt_urltestdata_absolute_delimiter_cases() {
        let base = Url::parse(BASE).unwrap();

        for expected in [
            "http://foo/path;a??e#f#g",
            "http://foo/abcd?efgh?ijkl",
            "http://foo/abcd#foo?bar",
        ] {
            assert_eq!(base.join(expected).unwrap().to_string(), expected);
        }
    }

    #[test]
    fn resolves_relative_paths() {
        let base = Url::parse(BASE).unwrap();

        assert_eq!(
            base.join("./next").unwrap().to_string(),
            "http://example.org/foo/next"
        );
        assert_eq!(
            base.join("../images/logo.svg").unwrap().to_string(),
            "http://example.org/images/logo.svg"
        );
        assert_eq!(
            base.join("/assets/site.css").unwrap().to_string(),
            "http://example.org/assets/site.css"
        );
        assert_eq!(
            base.join("//cdn.example.test/app.css").unwrap().to_string(),
            "http://cdn.example.test/app.css"
        );
    }

    #[test]
    fn normalizes_host_and_default_port() {
        assert_eq!(
            Url::parse("HTTPS://Example.Org:443/docs")
                .unwrap()
                .to_string(),
            "https://example.org/docs"
        );
    }

    #[test]
    fn wpt_special_url_normalization_cases() {
        assert_eq!(
            Url::parse("http://example\t.\norg").unwrap().to_string(),
            "http://example.org/"
        );

        assert_eq!(
            Url::parse("http://user:pass@foo:21/bar;par?b#c")
                .unwrap()
                .to_string(),
            "http://user:pass@foo:21/bar;par?b#c"
        );

        assert_eq!(
            Url::parse("https://test:@test").unwrap().to_string(),
            "https://test@test/"
        );

        assert_eq!(
            Url::parse("http://f:00000000000000000000080/c")
                .unwrap()
                .to_string(),
            "http://f/c"
        );

        assert_eq!(
            Url::parse("http://f:21/ b ? d # e").unwrap().to_string(),
            "http://f:21/%20b%20?%20d%20#%20e"
        );
    }

    #[test]
    fn wpt_special_url_reference_cases() {
        let base = Url::parse("http://example.org/foo/bar").unwrap();

        assert_eq!(
            base.join("http:foo.com").unwrap().to_string(),
            "http://example.org/foo/foo.com"
        );

        assert_eq!(
            base.join(r"\x").unwrap().to_string(),
            "http://example.org/x"
        );

        assert_eq!(
            base.join(r"\\x\hello").unwrap().to_string(),
            "http://x/hello"
        );
    }

    #[test]
    fn wpt_non_special_url_cases() {
        assert_eq!(
            Url::parse("non-special://test:@test/x")
                .unwrap()
                .to_string(),
            "non-special://test@test/x"
        );

        assert_eq!(Url::parse("a: foo.com").unwrap().to_string(), "a: foo.com");

        assert_eq!(
            Url::parse("lolscheme:x x#x x").unwrap().to_string(),
            "lolscheme:x x#x%20x"
        );

        assert_eq!(Url::parse("foo://").unwrap().to_string(), "foo://");

        assert_eq!(
            Url::parse("foo:/bar.com/").unwrap().to_string(),
            "foo:/bar.com/"
        );

        assert_eq!(Url::parse("ws://a@b/c").unwrap().to_string(), "ws://a@b/c");
    }

    #[test]
    fn wpt_host_and_userinfo_serialization_cases() {
        assert_eq!(
            Url::parse("http://é@é").unwrap().to_string(),
            "http://%C3%A9@xn--9ca/"
        );
        assert_eq!(
            Url::parse("http://&a:foo(b]c@d:2/").unwrap().to_string(),
            "http://&a:foo(b%5Dc@d:2/"
        );
        assert_eq!(
            Url::parse("http://::@c@d:2").unwrap().to_string(),
            "http://:%3A%40c@d:2/"
        );
        assert_eq!(
            Url::parse(r"http://foo.com/\@").unwrap().to_string(),
            "http://foo.com//@"
        );
        assert_eq!(
            Url::parse("http://[::127.0.0.1]").unwrap().to_string(),
            "http://[::7f00:1]/"
        );
        assert!(Url::parse("http://[::127.0.0.1.]").is_err());
    }

    #[test]
    fn wpt_special_scheme_short_forms() {
        let cases = [
            ("ftp://example.com/", "ftp://example.com/"),
            ("ftp:example.com/", "ftp://example.com/"),
            ("https:/example.com", "https://example.com/"),
            ("https:example.com/", "https://example.com/"),
            ("ws:/example.com/", "ws://example.com/"),
            ("wss:example.com/", "wss://example.com/"),
        ];

        for (input, expected) in cases {
            assert_eq!(Url::parse(input).unwrap().to_string(), expected);
        }

        let base = Url::parse("http://example.org/foo/bar").unwrap();

        assert_eq!(
            base.join("#β").unwrap().to_string(),
            "http://example.org/foo/bar#%CE%B2"
        );
    }

    #[test]
    fn wpt_file_url_cases() {
        assert_eq!(
            Url::parse("file:/example.com/").unwrap().to_string(),
            "file:///example.com/"
        );
        assert!(Url::parse("file://example:1/").is_err());
        assert!(Url::parse("file://example%/").is_err());
        assert!(Url::parse("file://[example]/").is_err());

        let base = Url::parse("file:///tmp/mock/path").unwrap();

        let cases = [
            (r"file:c:\foo\bar.html", "file:///c:/foo/bar.html"),
            (r"C|/foo/bar", "file:///C:/foo/bar"),
            (r"/C|\foo\bar", "file:///C:/foo/bar"),
            (r"//C|/foo/bar", "file:///C:/foo/bar"),
            ("//", "file:///"),
            ("file://test", "file://test/"),
            ("file://localhost/test", "file:///test"),
        ];

        for (input, expected) in cases {
            assert_eq!(base.join(input).unwrap().to_string(), expected);
        }
    }

    #[test]
    fn wpt_percent_encoded_dot_segments() {
        assert_eq!(
            Url::parse("http://example.com/foo/%2e")
                .unwrap()
                .to_string(),
            "http://example.com/foo/"
        );

        assert_eq!(
            Url::parse("http://example.com/foo/%2e./%2e%2e/.%2e/%2e.bar")
                .unwrap()
                .to_string(),
            "http://example.com/%2e.bar"
        );
    }

    #[test]
    fn wpt_legacy_ipv4_cases() {
        assert_eq!(
            Url::parse("http://192.0x00A80001").unwrap().to_string(),
            "http://192.168.0.1/"
        );
        assert_eq!(
            Url::parse("http://0x7f.1").unwrap().to_string(),
            "http://127.0.0.1/"
        );
        assert_eq!(
            Url::parse("http://127.1").unwrap().to_string(),
            "http://127.0.0.1/"
        );
        assert!(Url::parse("http://256.256.256.256").is_err());
    }

    #[test]
    fn wpt_host_preprocessing_and_opaque_urls() {
        assert_eq!(
            Url::parse("http://GOO\u{200B}\u{2060}\u{FEFF}goo.com")
                .unwrap()
                .to_string(),
            "http://googoo.com/"
        );
        assert_eq!(
            Url::parse("http://www.foo\u{3002}bar.com")
                .unwrap()
                .to_string(),
            "http://www.foo.bar.com/"
        );
        assert_eq!(
            Url::parse("http://\u{FF27}\u{FF4F}.com")
                .unwrap()
                .to_string(),
            "http://go.com/"
        );

        assert!(Url::parse("http://GOO\u{00A0}\u{3000}goo.com").is_err());
        assert!(Url::parse("http://\u{FDD0}zyx.com").is_err());
        assert!(Url::parse("http://%EF%B7%90zyx.com").is_err());
        assert!(Url::parse("http://%zz%66%a.com").is_err());

        assert_eq!(
            Url::parse("sc://fa\u{00DF}.ExAmPlE/").unwrap().to_string(),
            "sc://fa%C3%9F.ExAmPlE/"
        );
        assert_eq!(
            Url::parse("non-special:opaque  ?hi").unwrap().to_string(),
            "non-special:opaque %20?hi"
        );
    }
}
