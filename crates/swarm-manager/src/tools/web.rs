//! Web tools: a Brave-backed search tool and an SSRF-guarded fetch tool that
//! strips HTML to readable text.

use super::output::{prepare_tool_output, DEFAULT_MAX_OUTPUT_CHARS};
use super::Tool;
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::net::IpAddr;

const BRAVE_SEARCH_ENDPOINT: &str = "https://api.search.brave.com/res/v1/web/search";

/// Web search tool using the Brave search API.
pub struct WebSearchTool {
    api_key: String,
    endpoint: String,
    client: reqwest::Client,
}

impl WebSearchTool {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            endpoint: BRAVE_SEARCH_ENDPOINT.to_string(),
            client: reqwest::Client::new(),
        }
    }

    /// Build a search tool that talks to a custom endpoint (used in tests).
    pub fn with_endpoint(api_key: String, endpoint: String) -> Self {
        Self {
            api_key,
            endpoint,
            client: reqwest::Client::new(),
        }
    }
}

#[derive(Deserialize)]
struct BraveSearchResponse {
    web: Option<BraveWebResults>,
}

#[derive(Deserialize)]
struct BraveWebResults {
    results: Vec<BraveResult>,
}

#[derive(Deserialize)]
struct BraveResult {
    title: String,
    url: String,
    description: Option<String>,
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search the web and return results with titles, URLs, and snippets"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query"
                },
                "count": {
                    "type": "integer",
                    "description": "Number of results (1-10)",
                    "default": 5
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let query = args["query"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing query parameter"))?;
        if query.trim().is_empty() {
            anyhow::bail!("Missing search query text. Retry with a specific, narrow query.");
        }
        let count = args["count"].as_u64().unwrap_or(5).clamp(1, 10);

        let response = self
            .client
            .get(&self.endpoint)
            .header("X-Subscription-Token", &self.api_key)
            .query(&[("q", query), ("count", &count.to_string())])
            .send()
            .await?;

        if !response.status().is_success() {
            anyhow::bail!("Search error: {}", response.status());
        }

        let data: BraveSearchResponse = response.json().await?;

        let results: Vec<String> = data
            .web
            .map(|w| w.results)
            .unwrap_or_default()
            .into_iter()
            .map(|r| {
                format!(
                    "**{}**\n{}\n{}",
                    r.title,
                    r.url,
                    r.description.unwrap_or_default()
                )
            })
            .collect();

        Ok(prepare_tool_output(
            &results.join("\n\n"),
            DEFAULT_MAX_OUTPUT_CHARS,
        ))
    }
}

/// Web fetch tool to extract readable content from URLs.
pub struct WebFetchTool {
    client: reqwest::Client,
}

impl WebFetchTool {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .user_agent("Mozilla/5.0 (compatible; swarm-manager/1.0)")
                .build()
                .unwrap_or_default(),
        }
    }
}

impl Default for WebFetchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "web_fetch"
    }

    fn description(&self) -> &str {
        "Fetch a URL and extract readable text content"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "URL to fetch"
                },
                "max_chars": {
                    "type": "integer",
                    "description": "Maximum characters to return",
                    "default": 10000
                }
            },
            "required": ["url"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let url = args["url"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing url parameter"))?;
        let url = validate_public_http_url(url)?;
        let max_chars = (args["max_chars"].as_u64().unwrap_or(10000) as usize)
            .clamp(512, DEFAULT_MAX_OUTPUT_CHARS);

        let response = self.client.get(url.clone()).send().await?;

        if !response.status().is_success() {
            anyhow::bail!(
                "Fetch error for {}: {}. Retry with a narrower public HTTP(S) URL.",
                url,
                response.status()
            );
        }

        let html = response.text().await?;
        Ok(prepare_tool_output(&strip_html(&html), max_chars))
    }
}

/// Strip `<script>` blocks and remaining HTML tags, collapsing whitespace.
fn strip_html(html: &str) -> String {
    let text = html
        .replace("<script", "\n<script")
        .replace("</script>", "</script>\n")
        .lines()
        .filter(|l| !l.trim().starts_with("<script"))
        .collect::<Vec<_>>()
        .join("\n");

    let mut result = String::new();
    let mut in_tag = false;
    for ch in text.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(ch),
            _ => {}
        }
    }

    result
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn validate_public_http_url(raw: &str) -> Result<reqwest::Url> {
    let url = reqwest::Url::parse(raw).map_err(|err| {
        anyhow::anyhow!(
            "Invalid URL `{}`: {}. Retry with a full public http:// or https:// URL.",
            raw,
            err
        )
    })?;
    match url.scheme() {
        "http" | "https" => {}
        _ => {
            anyhow::bail!(
                "Unsupported URL scheme `{}`. Retry with a public http:// or https:// URL.",
                url.scheme()
            );
        }
    }
    if !url.username().is_empty() || url.password().is_some() {
        anyhow::bail!(
            "URL credentials are not allowed. Retry without embedding usernames, passwords, or tokens in the URL."
        );
    }
    let Some(host) = url.host_str() else {
        anyhow::bail!("URL is missing a host. Retry with a full public HTTP(S) URL.");
    };
    let host_lower = host.to_ascii_lowercase();
    if matches!(host_lower.as_str(), "localhost" | "0.0.0.0")
        || host_lower.ends_with(".localhost")
        || host_lower == "169.254.169.254"
        || host_lower.starts_with("127.")
        || host_lower == "::1"
    {
        anyhow::bail!("Local or metadata URLs are not allowed. Retry with a public HTTP(S) URL.");
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        let blocked = match ip {
            IpAddr::V4(ip) => {
                ip.is_private() || ip.is_loopback() || ip.is_link_local() || ip.is_unspecified()
            }
            IpAddr::V6(ip) => {
                ip.is_loopback()
                    || ip.is_unspecified()
                    || ip.is_unique_local()
                    || ip.is_unicast_link_local()
            }
        };
        if blocked {
            anyhow::bail!(
                "Private, local, or link-local IP URLs are not allowed. Retry with a public HTTP(S) URL."
            );
        }
    }

    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn strip_html_extracts_text() {
        let html = "<html><body><script>ignore()</script><p>Hello</p><p>World</p></body></html>";
        let out = strip_html(html);
        assert!(out.contains("Hello"));
        assert!(out.contains("World"));
        assert!(!out.contains("ignore"));
    }

    #[test]
    fn web_fetch_rejects_local_urls() {
        let err = validate_public_http_url("http://127.0.0.1:7331/rpc").unwrap_err();
        assert!(err.to_string().contains("Local or metadata URLs"));
    }

    #[test]
    fn web_fetch_rejects_private_ip_urls() {
        let err = validate_public_http_url("http://192.168.1.10/status").unwrap_err();
        assert!(err.to_string().contains("Private, local, or link-local"));
    }

    #[tokio::test]
    async fn web_search_rejects_blank_queries() {
        let tool = WebSearchTool::new("fake".to_string());
        let err = tool.execute(json!({"query": "   "})).await.unwrap_err();
        assert!(err.to_string().contains("Missing search query"));
    }

    /// Network-free: a one-shot std TcpListener mock that drains the request and
    /// replies with a canned Brave JSON body. Validates request-path reach +
    /// response parse without any real network call.
    #[tokio::test]
    async fn web_search_parses_mock_response() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let body = r#"{"web":{"results":[{"title":"Example","url":"https://example.com","description":"A site"}]}}"#;
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            // Drain the request headers so the client's write completes.
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
            stream.flush().unwrap();
        });

        let endpoint = format!("http://127.0.0.1:{}/search", addr.port());
        let tool = WebSearchTool::with_endpoint("token".to_string(), endpoint);
        let out = tool.execute(json!({"query": "example"})).await.unwrap();

        assert!(out.contains("Example"));
        assert!(out.contains("https://example.com"));
        assert!(out.contains("A site"));

        handle.join().unwrap();
    }
}
