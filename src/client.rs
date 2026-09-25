use crate::models::LoadCodeAssistResponse;
use anyhow::{Context, Result, anyhow, bail};
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use serde_json::{Value, json};
use std::env;
use std::time::Duration;

pub const DEFAULT_BASE_URLS: &[&str] = &["https://daily-cloudcode-pa.googleapis.com", "https://cloudcode-pa.googleapis.com"];

/// Builds a standard blocking reqwest [`Client`] with sensible default timeouts.
pub fn create_http_client() -> Result<Client> {
  Client::builder().timeout(Duration::from_secs(20)).connect_timeout(Duration::from_secs(10)).build().context("failed to create HTTPS client")
}

/// Generates the User-Agent header matching Antigravity ACP client conventions.
pub fn user_agent() -> String {
  let os = match env::consts::OS {
    "macos" => "darwin",
    other => other,
  };
  let arch = match env::consts::ARCH {
    "aarch64" => "arm64",
    other => other,
  };
  format!("antigravity/acp/1.2.1 (aidev_client; os_type={os}; arch={arch}; host_path=zed/unknown; proxy_client=antigravity/sdk)")
}

/// Helper that redacts the access token from an API error response and truncates it.
pub fn response_excerpt(value: &str, access_token: &str, max_chars: usize) -> String {
  value.replace(access_token, "[redacted]").chars().take(max_chars).collect()
}

/// Tries each base URL sequentially until one succeeds, collecting errors if all fail.
pub fn try_endpoints<T>(base_urls: &[&str], mut request: impl FnMut(&str) -> Result<T>) -> Result<T> {
  let mut errors = Vec::new();

  for base_url in base_urls {
    match request(base_url) {
      Ok(result) => return Ok(result),
      Err(err) => errors.push(format!("{base_url}: {err:#}")),
    }
  }

  bail!("quota lookup failed on every endpoint:\n  {}", errors.join("\n  "))
}

/// High-level function to fetch quota by trying the provided base URLs.
pub fn fetch_quota(client: &Client, access_token: &str, fallback_project: Option<&str>, base_urls: &[&str]) -> Result<(LoadCodeAssistResponse, Value)> {
  try_endpoints(base_urls, |base_url| fetch_quota_from_endpoint(client, access_token, fallback_project, base_url))
}

/// Fetches quota details from a specific Google Cloud Code PA endpoint.
pub fn fetch_quota_from_endpoint(client: &Client, access_token: &str, fallback_project: Option<&str>, base_url: &str) -> Result<(LoadCodeAssistResponse, Value)> {
  let body = json!({
      "metadata": {
          "ideType": "ANTIGRAVITY"
      }
  });

  let value = post_json(client, &format!("{base_url}/v1internal:loadCodeAssist"), access_token, &body).context("loadCodeAssist failed")?;

  let mut load: LoadCodeAssistResponse = serde_json::from_value(value).context("unexpected loadCodeAssist response")?;

  if load.cloudaicompanion_project.as_deref().filter(|s| !s.trim().is_empty()).is_none() {
    load.cloudaicompanion_project = fallback_project.map(|s| s.to_string());
  }

  let project = load.cloudaicompanion_project.as_deref().filter(|s| !s.trim().is_empty()).ok_or_else(|| {
    anyhow!(
      "Google loadCodeAssist did not return cloudaicompanionProject. \
                 The account may not expose Antigravity quota or the internal API changed."
    )
  })?;

  let quota = post_json(client, &format!("{base_url}/v1internal:retrieveUserQuotaSummary"), access_token, &json!({ "project": project })).context("retrieveUserQuotaSummary failed")?;

  Ok((load, quota))
}

/// Sends an authenticated POST request expecting a JSON response, handling standard Google errors.
pub fn post_json(client: &Client, url: &str, access_token: &str, body: &Value) -> Result<Value> {
  let response = client
    .post(url)
    .header(AUTHORIZATION, format!("Bearer {access_token}"))
    .header(CONTENT_TYPE, "application/json")
    .header(ACCEPT, "application/json")
    .header(USER_AGENT, user_agent())
    .json(body)
    .send()
    .with_context(|| format!("request failed: {url}"))?;

  let status = response.status();
  let text = response.text().with_context(|| format!("failed reading response from {url}"))?;

  if !status.is_success() {
    if status.as_u16() == 401 || status.as_u16() == 403 {
      bail!(
        "Google rejected the ACP OAuth access token (HTTP {}). \
                 Open/use the official Antigravity ACP agent in Zed so it can refresh or \
                 re-authenticate, then run the task again. Response: {}",
        status,
        response_excerpt(&text, access_token, 400)
      );
    }

    bail!("Google returned HTTP {} from {}: {}", status, url, response_excerpt(&text, access_token, 400));
  }

  serde_json::from_str(&text).with_context(|| format!("non-JSON response from {url}: {}", response_excerpt(&text, access_token, 200)))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn falls_back_when_quota_request_fails() {
    let mut attempted = Vec::new();
    let result = try_endpoints(&["first", "second"], |base_url| {
      attempted.push(base_url.to_owned());
      if base_url == "first" {
        bail!("retrieveUserQuotaSummary failed");
      }
      Ok("second quota")
    });
    assert_eq!(result.unwrap(), "second quota");
    assert_eq!(attempted, ["first", "second"]);
  }

  #[test]
  fn removes_access_token_from_error_excerpt() {
    assert_eq!(response_excerpt("error abcdefghijklmnopqrstuvwxyz", "abcdefghijklmnopqrstuvwxyz", 400), "error [redacted]");
  }

  #[test]
  fn formats_antigravity_user_agent() {
    let ua = user_agent();
    assert!(ua.starts_with("antigravity/acp/"));
    assert!(ua.contains("aidev_client"));
    assert!(ua.contains("proxy_client=antigravity/sdk"));
  }
}
