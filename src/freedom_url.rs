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
    MissingAuthority,
    EmptyHost,
    InvalidHost,
    InvalidPort,
}

impl fmt::Display for UrlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyInput => write!(f, "URL is empty"),
            Self::InvalidScheme => write!(f, "invalid URL scheme"),
            Self::MissingAuthority => write!(f, "URL is missing an authority"),
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

        if is_special_scheme(&scheme) {
            let after_scheme = after_scheme.replace('\\', "/");

            let Some(after_authority_marker) = after_scheme.strip_prefix("//") else {
                return Err(UrlError::MissingAuthority);
            };

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
                    write!(f, "{}", self.username)?;

                    if let Some(password) = &self.password {
                        write!(f, ":{password}")?;
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
                write!(f, "{}:{}", self.scheme, self.path)?;
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

fn sanitize_url_input(input: &str) -> String {
    trim_ascii_whitespace(input)
        .chars()
        .filter(|character| !matches!(character, '\t' | '\n' | '\r'))
        .collect()
}

fn serialize_component(component: &str) -> String {
    component.replace(' ', "%20")
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
    matches!(scheme, "http" | "https" | "ws" | "wss")
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
                ("http" | "ws", 80) | ("https" | "wss", 443) => None,
                _ => Some(port),
            }
        }
    };

    Ok((username, password, host.to_ascii_lowercase(), port))
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
}
