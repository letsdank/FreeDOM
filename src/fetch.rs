use crate::url::Url;
use reqwest::{blocking::Client, header::CONTENT_TYPE, redirect::Policy};
use std::{error::Error, fmt, time::Duration};

const USER_AGENT: &str = "FreeDOM/0.1 (experimental Rust browser)";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContentType {
    pub mime_type: String,
    pub charset: Option<String>,
}

#[derive(Clone, Debug)]
pub struct FetchResponse {
    pub url: Url,
    pub status: u16,
    pub content_type: Option<ContentType>,
    body: Vec<u8>,
}

impl FetchResponse {
    pub fn bytes(&self) -> &[u8] {
        &self.body
    }

    pub fn text_lossy(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

#[derive(Debug)]
pub enum FetchError {
    UnsupportedScheme(String),
    ClientBuild(String),
    Transport(String),
    InvalidFinalUrl(String),
}

impl fmt::Display for FetchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedScheme(scheme) => {
                write!(formatter, "unsupported fetch scheme: {scheme}")
            }
            Self::ClientBuild(message) => {
                write!(formatter, "could not create HTTP client: {message}")
            }
            Self::Transport(message) => {
                write!(formatter, "network request failed: {message}")
            }
            Self::InvalidFinalUrl(message) => {
                write!(formatter, "server returned an invalid final URL: {message}")
            }
        }
    }
}

impl Error for FetchError {}

pub struct Fetcher {
    client: Client,
}

impl Fetcher {
    pub fn new() -> Result<Self, FetchError> {
        let client = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(15))
            .redirect(Policy::limited(10))
            .build()
            .map_err(|error| FetchError::ClientBuild(error.to_string()))?;

        Ok(Self { client })
    }

    pub fn supports(url: &Url) -> bool {
        matches!(url.scheme(), "http" | "https")
    }

    pub fn get(&self, url: &Url) -> Result<FetchResponse, FetchError> {
        if !Self::supports(url) {
            return Err(FetchError::UnsupportedScheme(url.scheme().to_owned()));
        }

        let response = self
            .client
            .get(url.to_string())
            .send()
            .map_err(|error| FetchError::Transport(error.to_string()))?;

        let final_url = Url::parse(response.url().as_str())
            .map_err(|error| FetchError::InvalidFinalUrl(error.to_string()))?;

        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(ContentType::parse);

        let body = response
            .bytes()
            .map_err(|error| FetchError::Transport(error.to_string()))?
            .to_vec();

        Ok(FetchResponse {
            url: final_url,
            status,
            content_type,
            body,
        })
    }
}

impl ContentType {
    fn parse(value: &str) -> Option<Self> {
        let mut parts = value.split(';');
        let mime_type = parts.next()?.trim();

        if mime_type.is_empty() {
            return None;
        }

        let charset = parts.find_map(|part| {
            let (name, value) = part.split_once('=')?;

            if !name.trim().eq_ignore_ascii_case("charset") {
                return None;
            }

            let value = value.trim().trim_matches('"');

            (!value.is_empty()).then(|| value.to_ascii_lowercase())
        });

        Some(Self {
            mime_type: mime_type.to_ascii_lowercase(),
            charset,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_http_and_https_urls() {
        let http = Url::parse("http://example.com/").unwrap();
        let https = Url::parse("https://example.com/").unwrap();
        let data = Url::parse("data:text/plain,FreeDOM").unwrap();

        assert!(Fetcher::supports(&http));
        assert!(Fetcher::supports(&https));
        assert!(!Fetcher::supports(&data));
    }

    #[test]
    fn parses_content_type_and_charset() {
        let content_type = ContentType::parse(" text/css ; Charset = \"UTF-8\" ").unwrap();

        assert_eq!(content_type.mime_type, "text/css");
        assert_eq!(content_type.charset.as_deref(), Some("utf-8"));
    }
}
