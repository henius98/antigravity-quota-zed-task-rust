use serde::{Deserialize, Serialize};

/// Response payload from Google's `v1internal:loadCodeAssist` endpoint.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LoadCodeAssistResponse {
  #[serde(rename = "cloudaicompanionProject")]
  pub cloudaicompanion_project: Option<String>,

  #[serde(rename = "currentTier")]
  pub current_tier: Option<Tier>,

  #[serde(rename = "paidTier")]
  pub paid_tier: Option<Tier>,
}

impl LoadCodeAssistResponse {
  /// Resolves the user's tier name, checking paid tier first then current tier.
  pub fn tier_name(&self) -> Option<String> {
    self.paid_tier.as_ref().or(self.current_tier.as_ref()).and_then(|tier| tier.display_name())
  }
}

/// Tier information returned by the Code Assist API.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Tier {
  pub id: Option<String>,
  pub name: Option<String>,

  #[serde(rename = "displayName")]
  pub display_name: Option<String>,
}

impl Tier {
  /// Returns the best available display identifier for the tier.
  pub fn display_name(&self) -> Option<String> {
    self.display_name.as_ref().or(self.name.as_ref()).or(self.id.as_ref()).cloned()
  }
}

/// Response payload from Google's `v1internal:retrieveUserQuotaSummary` endpoint.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct QuotaSummaryResponse {
  pub groups: Vec<QuotaGroup>,
}

/// A logical group of quota buckets (e.g. Gemini Models).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct QuotaGroup {
  #[serde(rename = "displayName")]
  pub display_name: Option<String>,

  pub description: Option<String>,

  #[serde(default)]
  pub buckets: Vec<QuotaBucket>,
}

impl QuotaGroup {
  /// Returns the best available display name for the group.
  pub fn name(&self) -> String {
    self.display_name.as_ref().or(self.description.as_ref()).cloned().unwrap_or_else(|| "Quota group".to_string())
  }
}

/// Individual quota bucket details.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct QuotaBucket {
  #[serde(rename = "bucketId")]
  pub bucket_id: Option<String>,

  pub window: Option<String>,

  #[serde(rename = "displayName")]
  pub display_name: Option<String>,

  pub description: Option<String>,

  #[serde(rename = "remainingFraction")]
  pub remaining_fraction: Option<f64>,

  #[serde(rename = "resetTime")]
  pub reset_time: Option<String>,
}

impl QuotaBucket {
  /// Returns the best available display label for the bucket.
  pub fn name(&self) -> String {
    self.display_name.as_ref().or(self.description.as_ref()).or(self.window.as_ref()).or(self.bucket_id.as_ref()).cloned().unwrap_or_else(|| "Limit".to_string())
  }

  /// Computes the percentage (0.0 to 100.0) of quota remaining.
  pub fn remaining_percent(&self) -> Option<f64> {
    self.remaining_fraction.map(|v| (v * 100.0).clamp(0.0, 100.0))
  }
}

/// Normalized structure for JSON output or programmatic consumption.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NormalizedQuota {
  pub project: String,
  pub tier: Option<String>,
  pub groups: Vec<NormalizedQuotaGroup>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NormalizedQuotaGroup {
  pub name: String,
  pub buckets: Vec<NormalizedQuotaBucket>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NormalizedQuotaBucket {
  pub name: String,
  pub remaining_fraction: Option<f64>,
  pub remaining_percent: Option<f64>,
  pub reset_time: Option<String>,
  pub window: Option<String>,
  pub bucket_id: Option<String>,
}

impl NormalizedQuota {
  pub fn new(project: impl Into<String>, tier: Option<String>, groups: &[QuotaGroup]) -> Self {
    Self { project: project.into(), tier, groups: groups.iter().map(NormalizedQuotaGroup::from_group).collect() }
  }
}

impl NormalizedQuotaGroup {
  pub fn from_group(group: &QuotaGroup) -> Self {
    Self { name: group.name(), buckets: group.buckets.iter().map(NormalizedQuotaBucket::from_bucket).collect() }
  }
}

impl NormalizedQuotaBucket {
  pub fn from_bucket(bucket: &QuotaBucket) -> Self {
    Self {
      name: bucket.name(),
      remaining_fraction: bucket.remaining_fraction,
      remaining_percent: bucket.remaining_percent(),
      reset_time: bucket.reset_time.clone(),
      window: bucket.window.clone(),
      bucket_id: bucket.bucket_id.clone(),
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use serde_json::json;

  #[test]
  fn missing_quota_groups_is_an_error() {
    assert!(serde_json::from_value::<QuotaSummaryResponse>(json!({"message": "changed API"})).is_err());
    assert!(serde_json::from_value::<QuotaSummaryResponse>(json!({"groups": []})).is_ok());
  }

  #[test]
  fn tier_name_prefers_paid_tier() {
    let load = LoadCodeAssistResponse {
      cloudaicompanion_project: None,
      current_tier: Some(Tier { id: Some("free".into()), name: Some("Free Tier".into()), display_name: Some("Free".into()) }),
      paid_tier: Some(Tier { id: Some("pro".into()), name: Some("Pro Tier".into()), display_name: Some("Pro".into()) }),
    };
    assert_eq!(load.tier_name(), Some("Pro".into()));
  }
}
