//! # cap-http
//!
//! HTTP client type contracts for Ordo.
//! Inspired by Zed's http_client crate.

/// HTTP request.
#[derive(Clone, Debug)]
pub struct HttpRequest {
    pub method: HttpMethod,
    pub url: String,
    pub headers: HttpHeaders,
    pub body: Option<Vec<u8>>,
}

/// HTTP method.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Patch,
    Head,
    Options,
}

/// HTTP response.
#[derive(Clone, Debug)]
pub struct HttpResponse {
    pub status: StatusCode,
    pub headers: HttpHeaders,
    pub body: Vec<u8>,
}

/// HTTP status code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatusCode(pub u16);

impl StatusCode {
    pub fn is_success(&self) -> bool {
        self.0 >= 200 && self.0 < 300
    }
    pub fn is_redirect(&self) -> bool {
        self.0 >= 300 && self.0 < 400
    }
    pub fn is_client_error(&self) -> bool {
        self.0 >= 400 && self.0 < 500
    }
    pub fn is_server_error(&self) -> bool {
        self.0 >= 500
    }
}

/// Typed header map.
#[derive(Clone, Debug)]
pub struct HttpHeaders {
    headers: Vec<(String, String)>,
}

impl HttpHeaders {
    pub fn new() -> Self {
        HttpHeaders {
            headers: Vec::new(),
        }
    }
    pub fn insert(&mut self, key: &str, value: &str) {
        self.headers.push((key.to_string(), value.to_string()));
    }
    pub fn get(&self, key: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }
}

impl Default for HttpHeaders {
    fn default() -> Self {
        Self::new()
    }
}

/// HTTP errors.
#[derive(Clone, Debug, PartialEq)]
pub enum HttpError {
    Timeout,
    Connection(String),
    Status(u16),
    Tls(String),
}

/// Request builder (fluent API).
#[derive(Clone, Debug)]
pub struct RequestBuilder(HttpRequest);

impl RequestBuilder {
    pub fn new(method: HttpMethod, url: &str) -> Self {
        RequestBuilder(HttpRequest {
            method,
            url: url.to_string(),
            headers: HttpHeaders::new(),
            body: None,
        })
    }
    pub fn header(mut self, key: &str, value: &str) -> Self {
        self.0.headers.insert(key, value);
        self
    }
    pub fn body(mut self, data: Vec<u8>) -> Self {
        self.0.body = Some(data);
        self
    }
    pub fn build(self) -> HttpRequest {
        self.0
    }
}

/// HTTP client trait.
pub trait HttpClient {
    fn send(&mut self, request: HttpRequest) -> Result<HttpResponse, HttpError>;
    fn request_json(
        &mut self,
        method: HttpMethod,
        url: &str,
        body: &str,
    ) -> Result<HttpResponse, HttpError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_http_method() {
        assert_ne!(HttpMethod::Get, HttpMethod::Post);
    }
    #[test]
    fn test_status_code_success() {
        assert!(StatusCode(200).is_success());
        assert!(StatusCode(201).is_success());
        assert!(!StatusCode(404).is_success());
    }
    #[test]
    fn test_status_code_redirect() {
        assert!(StatusCode(301).is_redirect());
    }
    #[test]
    fn test_status_code_client_error() {
        assert!(StatusCode(404).is_client_error());
    }
    #[test]
    fn test_status_code_server_error() {
        assert!(StatusCode(500).is_server_error());
    }
    #[test]
    fn test_headers() {
        let mut h = HttpHeaders::new();
        h.insert("Content-Type", "application/json");
        assert_eq!(h.get("content-type"), Some("application/json"));
    }
    #[test]
    fn test_request_builder() {
        let req = RequestBuilder::new(HttpMethod::Get, "https://example.com")
            .header("Accept", "text/html")
            .build();
        assert_eq!(req.method, HttpMethod::Get);
        assert_eq!(req.url, "https://example.com");
    }
    #[test]
    fn test_request_builder_with_body() {
        let req = RequestBuilder::new(HttpMethod::Post, "https://example.com/api")
            .body(vec![1, 2, 3])
            .build();
        assert!(req.body.is_some());
    }
    #[test]
    fn test_http_error() {
        let e = HttpError::Timeout;
        assert!(format!("{:?}", e).contains("Timeout"));
    }
    #[test]
    fn test_http_request() {
        let req = HttpRequest {
            method: HttpMethod::Get,
            url: "https://test.com".into(),
            headers: HttpHeaders::new(),
            body: None,
        };
        assert_eq!(req.method, HttpMethod::Get);
    }
}
