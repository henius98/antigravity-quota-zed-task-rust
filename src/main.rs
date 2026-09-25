use anyhow::{Context, Result, anyhow, bail};
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use serde::Deserialize;
use serde_json::{Value, json};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const DEFAULT_BASE_URLS: &[&str] = &["https://daily-cloudcode-pa.googleapis.com", "https://cloudcode-pa.googleapis.com"];

#[derive(Debug, Deserialize)]
struct LoadCodeAssistResponse {
  #[serde(rename = "cloudaicompanionProject")]
  cloudaicompanion_project: Option<String>,

  #[serde(rename = "currentTier")]
  current_tier: Option<Tier>,

  #[serde(rename = "paidTier")]
  paid_tier: Option<Tier>,
}

#[derive(Debug, Deserialize)]
struct Tier {
  id: Option<String>,
  name: Option<String>,

  #[serde(rename = "displayName")]
  display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct QuotaSummaryResponse {
  groups: Vec<QuotaGroup>,
}

#[derive(Debug, Deserialize)]
struct QuotaGroup {
  #[serde(rename = "displayName")]
  display_name: Option<String>,

  description: Option<String>,

  #[serde(default)]
  buckets: Vec<QuotaBucket>,
}

#[derive(Debug, Deserialize)]
struct QuotaBucket {
  #[serde(rename = "bucketId")]
  bucket_id: Option<String>,

  window: Option<String>,

  #[serde(rename = "displayName")]
  display_name: Option<String>,

  description: Option<String>,

  #[serde(rename = "remainingFraction")]
  remaining_fraction: Option<f64>,

  #[serde(rename = "resetTime")]
  reset_time: Option<String>,
}

#[derive(Default)]
struct Options {
  json_output: bool,
  raw_output: bool,
  token_file: Option<String>,
}

fn main() {
  if let Err(err) = run() {
    eprintln!("Antigravity quota error: {err:#}");
    std::process::exit(1);
  }
}

fn run() -> Result<()> {
  let args: Vec<String> = env::args().skip(1).collect();
  let options = parse_args(&args)?;

  let token_path = token_path_from_args(options.token_file.as_deref())?;
  let token_json = read_token_json(&token_path)?;

  let client = Client::builder().timeout(Duration::from_secs(20)).connect_timeout(Duration::from_secs(10)).build().context("failed to create HTTPS client")?;

  let access_token = resolve_access_token(&client, &token_json, &token_path)?;
  let fallback_project = find_named_string(&token_json, "project_id").or_else(|| find_named_string(&token_json, "projectId"));

  let (load, raw_quota) = fetch_quota(&client, &access_token, fallback_project, DEFAULT_BASE_URLS)?;
  let project = load.cloudaicompanion_project.as_deref().expect("fetch_quota requires a project");

  if options.raw_output {
    println!("{}", serde_json::to_string_pretty(&raw_quota)?);
    return Ok(());
  }

  let quota: QuotaSummaryResponse = serde_json::from_value(raw_quota).context("Google returned an unexpected quota response shape")?;

  if options.json_output {
    let result = json!({
        "project": project,
        "tier": tier_name(&load),
        "groups": normalize_groups(&quota.groups),
    });
    println!("{}", serde_json::to_string_pretty(&result)?);
  } else {
    print_human(project, tier_name(&load).as_deref(), &quota.groups);
  }

  Ok(())
}

fn parse_args(args: &[String]) -> Result<Options> {
  let mut options = Options::default();
  let mut args = args.iter();

  while let Some(arg) = args.next() {
    match arg.as_str() {
      "--json" => options.json_output = true,
      "--raw" => options.raw_output = true,
      "--token-file" => {
        let value = args.next().filter(|value| !value.is_empty() && !value.starts_with('-')).ok_or_else(|| anyhow!("--token-file requires a path"))?;
        if options.token_file.replace(value.clone()).is_some() {
          bail!("--token-file may only be specified once");
        }
      }
      _ => bail!("unknown argument: {arg}"),
    }
  }

  if options.json_output && options.raw_output {
    bail!("--json and --raw cannot be used together");
  }

  Ok(options)
}

fn token_path_from_args(token_file: Option<&str>) -> Result<PathBuf> {
  if let Some(value) = token_file {
    return Ok(expand_home(value));
  }

  if let Ok(value) = env::var("ANTIGRAVITY_ACP_TOKEN_FILE")
    && !value.trim().is_empty()
  {
    return Ok(expand_home(&value));
  }

  let home = env::var_os("HOME").ok_or_else(|| anyhow!("HOME is not set"))?;
  Ok(PathBuf::from(home).join(".gemini").join("antigravity-acp").join("acp_token.json"))
}

fn expand_home(value: &str) -> PathBuf {
  if let Some(rest) = value.strip_prefix("~/")
    && let Some(home) = env::var_os("HOME")
  {
    return PathBuf::from(home).join(rest);
  }
  PathBuf::from(value)
}

fn read_token_json(path: &Path) -> Result<Value> {
  let metadata = fs::metadata(path).with_context(|| {
    format!(
      "ACP OAuth token file not found: {}. \
             Authenticate the Antigravity ACP agent in Zed first.",
      path.display()
    )
  })?;

  if metadata.len() > 2 * 1024 * 1024 {
    bail!("Refusing to read unexpectedly large token file: {}", path.display());
  }

  let contents = fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;

  serde_json::from_str(&contents).with_context(|| format!("failed to parse {}", path.display()))
}

fn user_agent() -> String {
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

fn resolve_access_token(client: &Client, token_json: &Value, token_path: &Path) -> Result<String> {
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

fn find_access_token(value: &Value) -> Option<&str> {
  const PREFERRED_KEYS: &[&str] = &["access_token", "accessToken", "access", "token"];

  PREFERRED_KEYS.iter().find_map(|key| find_named_token(value, key))
}

fn find_refresh_token(value: &Value) -> Option<&str> {
  const REFRESH_KEYS: &[&str] = &["refresh_token", "refreshToken"];

  REFRESH_KEYS.iter().find_map(|key| find_named_token(value, key))
}

fn find_named_string<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
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

fn find_named_token<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
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

fn refresh_access_token(client: &Client, token_uri: &str, refresh_token: &str, client_id: Option<&str>, client_secret: Option<&str>) -> Result<String> {
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

fn redact_secrets(text: &str, secrets: &[&str]) -> String {
  let mut result = text.to_string();
  for secret in secrets {
    if !secret.is_empty() {
      result = result.replace(secret, "[redacted]");
    }
  }
  result
}

fn fetch_quota(client: &Client, access_token: &str, fallback_project: Option<&str>, base_urls: &[&str]) -> Result<(LoadCodeAssistResponse, Value)> {
  try_endpoints(base_urls, |base_url| fetch_quota_from_endpoint(client, access_token, fallback_project, base_url))
}

fn try_endpoints<T>(base_urls: &[&str], mut request: impl FnMut(&str) -> Result<T>) -> Result<T> {
  let mut errors = Vec::new();

  for base_url in base_urls {
    match request(base_url) {
      Ok(result) => return Ok(result),
      Err(err) => errors.push(format!("{base_url}: {err:#}")),
    }
  }

  bail!("quota lookup failed on every endpoint:\n  {}", errors.join("\n  "))
}

fn fetch_quota_from_endpoint(client: &Client, access_token: &str, fallback_project: Option<&str>, base_url: &str) -> Result<(LoadCodeAssistResponse, Value)> {
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

  let project = load
    .cloudaicompanion_project
    .as_deref()
    .filter(|s| !s.trim().is_empty())
    .ok_or_else(|| anyhow!("Google loadCodeAssist did not return cloudaicompanionProject. The account may not expose Antigravity quota or the internal API changed."))?;
  let quota = post_json(client, &format!("{base_url}/v1internal:retrieveUserQuotaSummary"), access_token, &json!({ "project": project })).context("retrieveUserQuotaSummary failed")?;

  Ok((load, quota))
}

fn post_json(client: &Client, url: &str, access_token: &str, body: &Value) -> Result<Value> {
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

fn response_excerpt(value: &str, access_token: &str, max_chars: usize) -> String {
  value.replace(access_token, "[redacted]").chars().take(max_chars).collect()
}

fn tier_name(load: &LoadCodeAssistResponse) -> Option<String> {
  load.paid_tier.as_ref().or(load.current_tier.as_ref()).and_then(|tier| tier.display_name.as_ref().or(tier.name.as_ref()).or(tier.id.as_ref()).cloned())
}

fn group_name(group: &QuotaGroup) -> String {
  group.display_name.as_ref().or(group.description.as_ref()).cloned().unwrap_or_else(|| "Quota group".to_string())
}

fn bucket_name(bucket: &QuotaBucket) -> String {
  bucket.display_name.as_ref().or(bucket.description.as_ref()).or(bucket.window.as_ref()).or(bucket.bucket_id.as_ref()).cloned().unwrap_or_else(|| "Limit".to_string())
}

fn normalize_groups(groups: &[QuotaGroup]) -> Value {
  Value::Array(
    groups
      .iter()
      .map(|group| {
        json!({
            "name": group_name(group),
            "buckets": group.buckets.iter().map(|bucket| {
                json!({
                    "name": bucket_name(bucket),
                    "remaining_fraction": bucket.remaining_fraction,
                    "remaining_percent": bucket.remaining_fraction.map(|v| (v * 100.0).clamp(0.0, 100.0)),
                    "reset_time": bucket.reset_time,
                    "window": bucket.window,
                    "bucket_id": bucket.bucket_id,
                })
            }).collect::<Vec<_>>()
        })
      })
      .collect(),
  )
}

fn print_human(project: &str, tier: Option<&str>, groups: &[QuotaGroup]) {
  println!("Antigravity quota");
  println!("=================");

  if let Some(tier) = tier {
    println!("Plan: {tier}");
  }

  // Useful for diagnostics, but not a credential.
  println!("Project: {project}");

  if groups.is_empty() {
    println!("\nNo grouped quota buckets were returned.");
    return;
  }

  for group in groups {
    let name = group_name(group);
    println!("\n{name}");
    println!("{}", "-".repeat(name.chars().count()));

    if group.buckets.is_empty() {
      println!("No buckets returned.");
      continue;
    }

    for bucket in &group.buckets {
      let label = bucket_name(bucket);

      match bucket.remaining_fraction {
        Some(fraction) => {
          let percent = (fraction * 100.0).clamp(0.0, 100.0);
          println!("{label}: {percent:.1}% remaining");
        }
        None => println!("{label}: remaining quota unknown"),
      }

      if let Some(reset) = bucket.reset_time.as_deref()
        && !reset.trim().is_empty()
      {
        println!("  reset: {reset}");
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn rejects_invalid_cli_options() {
    assert!(parse_args(&["--unknown".into()]).is_err());
    assert!(parse_args(&["--token-file".into(), "--json".into()]).is_err());
    assert!(parse_args(&["--json".into(), "--raw".into()]).is_err());
    assert_eq!(parse_args(&["--token-file".into(), "~/token.json".into()]).unwrap().token_file.as_deref(), Some("~/token.json"));
  }

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

  #[test]
  fn formats_antigravity_user_agent() {
    let ua = user_agent();
    assert!(ua.starts_with("antigravity/acp/"));
    assert!(ua.contains("aidev_client"));
    assert!(ua.contains("proxy_client=antigravity/sdk"));
  }

  #[test]
  fn missing_quota_groups_is_an_error() {
    assert!(serde_json::from_value::<QuotaSummaryResponse>(json!({"message": "changed API"})).is_err());
    assert!(serde_json::from_value::<QuotaSummaryResponse>(json!({"groups": []})).is_ok());
  }

  #[test]
  fn normalizes_quota() {
    let groups = vec![QuotaGroup {
      display_name: Some("Gemini Models".into()),
      description: None,
      buckets: vec![QuotaBucket {
        bucket_id: Some("weekly".into()),
        window: Some("WEEKLY".into()),
        display_name: Some("Weekly Limit".into()),
        description: None,
        remaining_fraction: Some(0.83),
        reset_time: Some("2026-10-01T00:00:00Z".into()),
      }],
    }];

    let value = normalize_groups(&groups);
    assert_eq!(value[0]["name"], "Gemini Models");
    assert_eq!(value[0]["buckets"][0]["remaining_percent"], 83.0);
  }
}
