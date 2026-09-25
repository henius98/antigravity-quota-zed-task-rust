use crate::cli::OutputFormat;
use crate::models::{NormalizedQuota, QuotaGroup};
use anyhow::Result;
use serde_json::{Value, json};
use std::io::{self, Write};

/// Normalizes quota groups into a `serde_json::Value` structure.
pub fn normalize_groups(groups: &[QuotaGroup]) -> Value {
  Value::Array(
    groups
      .iter()
      .map(|group| {
        json!({
            "name": group.name(),
            "buckets": group.buckets.iter().map(|bucket| {
                json!({
                    "name": bucket.name(),
                    "remaining_fraction": bucket.remaining_fraction,
                    "remaining_percent": bucket.remaining_percent(),
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

/// Dispatches printing according to the selected [`OutputFormat`].
pub fn print_output(format: OutputFormat, project: &str, tier: Option<&str>, groups: &[QuotaGroup], raw_quota: &Value) -> Result<()> {
  match format {
    OutputFormat::Raw => {
      println!("{}", serde_json::to_string_pretty(raw_quota)?);
    }
    OutputFormat::Json => {
      let normalized = NormalizedQuota::new(project, tier.map(str::to_string), groups);
      println!("{}", serde_json::to_string_pretty(&normalized)?);
    }
    OutputFormat::Human => {
      print_human(project, tier, groups);
    }
  }
  Ok(())
}

/// Prints a friendly human-readable summary of the quota to stdout.
pub fn print_human(project: &str, tier: Option<&str>, groups: &[QuotaGroup]) {
  let mut stdout = io::stdout().lock();
  let _ = write_human(&mut stdout, project, tier, groups);
}

/// Formats the human-readable summary to an arbitrary [`Write`] implementation.
pub fn write_human<W: Write>(writer: &mut W, project: &str, tier: Option<&str>, groups: &[QuotaGroup]) -> io::Result<()> {
  writeln!(writer, "Antigravity quota")?;
  writeln!(writer, "=================")?;

  if let Some(tier) = tier {
    writeln!(writer, "Plan: {tier}")?;
  }

  // Useful for diagnostics, but not a credential.
  writeln!(writer, "Project: {project}")?;

  if groups.is_empty() {
    writeln!(writer, "\nNo grouped quota buckets were returned.")?;
    return Ok(());
  }

  for group in groups {
    let name = group.name();
    writeln!(writer, "\n{name}")?;
    writeln!(writer, "{}", "-".repeat(name.chars().count()))?;

    if group.buckets.is_empty() {
      writeln!(writer, "No buckets returned.")?;
      continue;
    }

    for bucket in &group.buckets {
      let label = bucket.name();

      match bucket.remaining_percent() {
        Some(percent) => {
          writeln!(writer, "{label}: {percent:.1}% remaining")?;
        }
        None => writeln!(writer, "{label}: remaining quota unknown")?,
      }

      if let Some(reset) = bucket.reset_time.as_deref()
        && !reset.trim().is_empty()
      {
        writeln!(writer, "  reset: {reset}")?;
      }
    }
  }

  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::models::QuotaBucket;

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

  #[test]
  fn formats_human_output() {
    let groups = vec![QuotaGroup {
      display_name: Some("Gemini 1.5 Pro".into()),
      description: None,
      buckets: vec![QuotaBucket {
        bucket_id: Some("daily".into()),
        window: Some("DAY".into()),
        display_name: Some("Daily Requests".into()),
        description: None,
        remaining_fraction: Some(0.5),
        reset_time: Some("2026-09-26T00:00:00Z".into()),
      }],
    }];

    let mut output = Vec::new();
    write_human(&mut output, "my-project", Some("Pro"), &groups).unwrap();
    let rendered = String::from_utf8(output).unwrap();

    assert!(rendered.contains("Plan: Pro"));
    assert!(rendered.contains("Project: my-project"));
    assert!(rendered.contains("Gemini 1.5 Pro"));
    assert!(rendered.contains("Daily Requests: 50.0% remaining"));
    assert!(rendered.contains("reset: 2026-09-26T00:00:00Z"));
  }
}
