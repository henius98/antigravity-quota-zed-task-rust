use crate::client::user_agent;
use anyhow::{Context, Result, anyhow, bail};
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, CONTENT_TYPE, USER_AGENT};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

/// Maximum allowed token file size (2 MiB) to guard against reading huge unintended files.
const MAX_TOKEN_FILE_SIZE_BYTES: u64 = 2 * 1024 * 1024;

/// Reads and parses the OAuth token JSON file from disk.
pub fn read_token_json(path: &Path) -> Result<Value> {
  let metadata = fs::metadata(path).with_context(|| {
    format!(
      "ACP OAuth token file not found: {}. \
             Authenticate the Antigravity ACP agent in Zed first.",
      path.display()
    )
  })?;

  if metadata.len() > MAX_TOKEN_FILE_SIZE_BYTES {
    bail!("Refusing to read unexpectedly large token file: {}", path.display());
  }

  let contents = fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;

  serde_json::from_str(&contents).with_context(|| format!("failed to parse {}", path.display()))
}

/// Resolves a valid access token:
/// - If a direct access token is present in `token_json`, returns it.
/// - If a refresh token is found, requests a new access token from Google's OAuth endpoint.
pub fn resolve_access_token(client: &Client, token_json: &Value, token_path: &Path) -> Result<String> {
  if let Some(token) = find_access_token(token_json) {
    return Ok(token.to_string());
  }

  if let Some(refresh_token) = find_refresh_token(token_json) {
    let client_id = find_named_string(token_json, "client_id").or_else(|| find_named_string(token_json, "clientId"));
    let client_secret = find_named_string(token_json, "client_secret").or_else(|| find_named_string(token_json, "clientSecret"));
    let token_uri = find_named_string(token_json, "token_uri").or_else(|| find_named_string(token_json, "tokenUri")).unwrap_or("https://oauth2.googleapis.com/token");

    return refresh_access_token(client, token_uri, refresh_token, client_id, client_secret).with_context(|| format!("failed to refresh access token using credentials from {}", token_path.display()));
  }

  bail!("No OAuth access token or refresh token found in {}. The ACP token schema may have changed.", token_path.display())
}

/// Recursively searches for an access token in the JSON structure.
pub fn find_access_token(value: &Value) -> Option<&str> {
  const PREFERRED_KEYS: &[&str] = &["access_token", "accessToken", "access", "token"];

  PREFERRED_KEYS.iter().find_map(|key| find_named_token(value, key))
}

/// Recursively searches for a refresh token in the JSON structure.
pub fn find_refresh_token(value: &Value) -> Option<&str> {
  const REFRESH_KEYS: &[&str] = &["refresh_token", "refreshToken"];

  REFRESH_KEYS.iter().find_map(|key| find_named_token(value, key))
}

/// Recursively finds a non-empty string under the given key in a JSON object/array.
pub fn find_named_string<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
  match value {
    Value::Object(map) => {
      if let Some(Value::String(val)) = map.get(key)
        && !val.trim().is_empty()
      {
        return Some(val.trim());
      }

      for child in map.values() {
        if let Some(val) = find_named_string(child, key) {
          return Some(val);
        }
      }
    }
    Value::Array(items) => {
      for child in items {
        if let Some(val) = find_named_string(child, key) {
          return Some(val);
        }
      }
    }
    _ => {}
  }

  None
}

/// Recursively finds a token string (> 20 chars) under the given key in a JSON structure.
pub fn find_named_token<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
  match value {
    Value::Object(map) => {
      if let Some(Value::String(token)) = map.get(key)
        && token.trim().len() > 20
      {
        return Some(token.trim());
      }

      for child in map.values() {
        if let Some(token) = find_named_token(child, key) {
          return Some(token);
        }
      }
    }
    Value::Array(items) => {
      for child in items {
        if let Some(token) = find_named_token(child, key) {
          return Some(token);
        }
      }
    }
    _ => {}
  }

  None
}

/// Exchanges an OAuth refresh token for a short-lived access token.
pub fn refresh_access_token(client: &Client, token_uri: &str, refresh_token: &str, client_id: Option<&str>, client_secret: Option<&str>) -> Result<String> {
  let mut body = json!({
      "grant_type": "refresh_token",
      "refresh_token": refresh_token,
  });

  if let Some(id) = client_id {
    body["client_id"] = json!(id);
  }
  if let Some(secret) = client_secret {
    body["client_secret"] = json!(secret);
  }

  let response = client
    .post(token_uri)
    .header(CONTENT_TYPE, "application/json")
    .header(ACCEPT, "application/json")
    .header(USER_AGENT, user_agent())
    .json(&body)
    .send()
    .with_context(|| format!("request failed to token endpoint: {token_uri}"))?;

  let status = response.status();
  let text = response.text().with_context(|| format!("failed reading response from {token_uri}"))?;

  if !status.is_success() {
    let mut secrets = vec![refresh_token];
    if let Some(s) = client_secret {
      secrets.push(s);
    }
    let redacted = redact_secrets(&text, &secrets);
    bail!("OAuth refresh failed (HTTP {status}): {redacted}");
  }

  let res_json: Value = serde_json::from_str(&text).with_context(|| "non-JSON response from OAuth token endpoint")?;

  find_access_token(&res_json).map(|s| s.to_string()).ok_or_else(|| anyhow!("OAuth token endpoint response did not contain an access_token"))
}

/// Redacts occurrences of sensitive secrets from a string.
pub fn redact_secrets(text: &str, secrets: &[&str]) -> String {
  let mut result = text.to_string();
  for secret in secrets {
    if !secret.is_empty() {
      result = result.replace(secret, "[redacted]");
    }
  }
  result
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn extracts_nested_access_token() {
    let value = json!({
        "token": "unrelated-token-abcdefghijklmnopqrstuvwxyz",
        "oauth": {
            "credentials": {
                "access_token": "abcdefghijklmnopqrstuvwxyz0123456789"
            }
        }
    });

    assert_eq!(find_access_token(&value), Some("abcdefghijklmnopqrstuvwxyz0123456789"));
  }

  #[test]
  fn extracts_refresh_token_and_named_strings() {
    let value = json!({
        "client_id": "test-client-id-1234567890",
        "client_secret": "test-secret",
        "refresh_token": "1//test-refresh-token-abcdefghijklmnopqrstuvwxyz",
        "token_uri": "https://oauth2.googleapis.com/token",
        "project_id": "aicode-consumers"
    });

    assert_eq!(find_refresh_token(&value), Some("1//test-refresh-token-abcdefghijklmnopqrstuvwxyz"));
    assert_eq!(find_named_string(&value, "client_id"), Some("test-client-id-1234567890"));
    assert_eq!(find_named_string(&value, "project_id"), Some("aicode-consumers"));
  }

  #[test]
  fn resolves_direct_access_token_without_refresh() {
    let client = Client::builder().build().unwrap();
    let value = json!({
        "access_token": "direct-access-token-0123456789abcdef"
    });
    let result = resolve_access_token(&client, &value, Path::new("/dummy/token.json")).unwrap();
    assert_eq!(result, "direct-access-token-0123456789abcdef");
  }

  #[test]
  fn errors_when_neither_access_nor_refresh_token_found() {
    let client = Client::builder().build().unwrap();
    let value = json!({
        "unrelated": "data"
    });
    let err = resolve_access_token(&client, &value, Path::new("/dummy/token.json")).unwrap_err();
    assert!(err.to_string().contains("No OAuth access token or refresh token found"));
  }
}
