use std::{error::Error, fmt};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Url {
    scheme: String,
    host: String,
    port: Option<u16>,
    path: String,
    query: Option<String>,
    fragment: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UrlError {
    EmptyInput,
    InvalidScheme,
    UnsupportedScheme(String),
    MissingAuthority,
    EmptyHost,
    UserInfoUnsupported,
    InvalidHost,
    InvalidPort,
}

impl fmt::Display for UrlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyInput => write!(f, "URL is empty"),
            Self::InvalidScheme => write!(f, "invalid URL scheme"),
            Self::UnsupportedScheme(scheme) => write!(f, "unsupported URL scheme: {scheme}"),
            Self::MissingAuthority => write!(f, "URL is missing an authority"),
            Self::EmptyHost => write!(f, "URL host is empty"),
            Self::UserInfoUnsupported => write!(f, "URL user info is not supported yet"),
            Self::InvalidHost => write!(f, "invalid URL host"),
            Self::InvalidPort => write!(f, "invalid URL port"),
        }
    }
}

impl Error for UrlError {}

impl Url {
    pub fn parse(input: &str) -> Result<Self, UrlError> {
        let input = trim_ascii_whitespace(input);
        if input.is_empty() {
            return Err(UrlError::EmptyInput);
        }

        let (without_fragment, fragment) = split_component(input, '#');
        let (without_query, query) = split_component(without_fragment, '?');

        let Some((scheme, after_scheme)) = without_query.split_once(':') else {
            return Err(UrlError::InvalidScheme);
        };

        if !is_valid_scheme(scheme) {
            return Err(UrlError::InvalidScheme);
        }

        let scheme = scheme.to_ascii_lowercase();
        if !matches!(scheme.as_str(), "http" | "https") {
            return Err(UrlError::UnsupportedScheme(scheme));
        }

        let Some(after_authority_marker) = after_scheme.strip_prefix("//") else {
            return Err(UrlError::MissingAuthority);
        };

        let authority_end = after_authority_marker
            .find('/')
            .unwrap_or(after_authority_marker.len());

        let authority = &after_authority_marker[..authority_end];
        let raw_path = &after_authority_marker[authority_end..];
        let (host, port) = parse_authority(&scheme, authority)?;

        Ok(Self {
            scheme,
            host,
            port,
            path: normalize_path(raw_path),
            query: query.map(str::to_owned),
            fragment: fragment.map(str::to_owned),
        })
    }

    pub fn join(&self, reference: &str) -> Result<Self, UrlError> {
        let reference = trim_ascii_whitespace(reference);

        if reference.is_empty() {
            let mut resolved = self.clone();
            resolved.fragment = None;
            return Ok(resolved);
        }

        if has_scheme(reference) {
            return Self::parse(reference);
        }

        let (without_fragment, fragment) = split_component(reference, '#');

        if without_fragment.starts_with("//") {
            let absolute = format!("{}:{}", self.scheme, without_fragment);
            let mut resolved = Self::parse(&absolute)?;
            resolved.fragment = fragment.map(str::to_owned);
            return Ok(resolved);
        }

        let (reference_path, query) = split_component(without_fragment, '?');

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

    pub fn scheme(&self) -> &str {
        &self.scheme
    }
}

impl fmt::Display for Url {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}://{}", self.scheme, self.host)?;

        if let Some(port) = self.port {
            write!(f, ":{port}")?;
        }

        write!(f, "{}", self.path)?;

        if let Some(query) = &self.query {
            write!(f, "?{query}")?;
        }

        if let Some(fragment) = &self.fragment {
            write!(f, "#{fragment}")?;
        }

        Ok(())
    }
}

fn trim_ascii_whitespace(input: &str) -> &str {
    input.trim_matches(|character: char| character.is_ascii_whitespace())
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
        Some(character) if character.is_alphanumeric() => {}
        _ => return false,
    }

    characters
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.'))
}

fn parse_authority(scheme: &str, authority: &str) -> Result<(String, Option<u16>), UrlError> {
    if authority.is_empty() {
        return Err(UrlError::EmptyHost);
    }

    if authority.contains('@') {
        return Err(UrlError::UserInfoUnsupported);
    }

    let (host, port_text) = if authority.starts_with('[') {
        let Some(closing_bracket) = authority.find(']') else {
            return Err(UrlError::InvalidHost);
        };

        let host = &authority[..=closing_bracket];
        let suffix = &authority[closing_bracket + 1..];

        if suffix.is_empty() {
            (host, None)
        } else if let Some(port) = suffix.strip_prefix(':') {
            (host, Some(port))
        } else {
            return Err(UrlError::InvalidHost);
        }
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) if !host.contains(':') => (host, Some(port)),
            Some(_) => return Err(UrlError::InvalidHost),
            None => (authority, None),
        }
    };

    if host.is_empty() {
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
            let port = port.parse::<u16>().map_err(|_| UrlError::InvalidPort)?;

            match (scheme, port) {
                ("http", 80) | ("https", 443) => None,
                _ => Some(port),
            }
        }
    };

    Ok((host.to_ascii_lowercase(), port))
}

fn normalize_path(raw_path: &str) -> String {
    let path = raw_path.replace('\\', "/");

    if path.is_empty() {
        return "/".to_owned();
    }

    let trailing_slash = path.ends_with('/') || path.ends_with("/.") || path.ends_with("/..");
    let mut segments = Vec::new();

    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            _ => segments.push(segment),
        }
    }

    let mut normalized = String::from("/");
    normalized.push_str(&segments.join("/"));

    if trailing_slash && normalized != "/" {
        normalized.push('/');
    }

    normalized
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
}
