use anyhow::{Context, Result};
use std::env;
use std::fs;
use std::path::PathBuf;

/// Default database path: `~/.local/share/antigravity-quota/quota_history.db`
pub fn default_db_path() -> Result<PathBuf> {
  let home = env::var_os("HOME").ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
  let dir = PathBuf::from(home).join(".local").join("share").join("antigravity-quota");
  fs::create_dir_all(&dir).with_context(|| format!("failed to create history directory: {}", dir.display()))?;
  Ok(dir.join("quota_history.db"))
}

/// Opens (or creates) the SQLite database and ensures the schema exists.
pub fn open_db(path: &std::path::Path) -> Result<rusqlite::Connection> {
  let conn = rusqlite::Connection::open(path).with_context(|| format!("failed to open history database: {}", path.display()))?;

  conn
    .execute_batch(
      "PRAGMA journal_mode = WAL;
     PRAGMA foreign_keys = ON;
     CREATE TABLE IF NOT EXISTS quota_snapshots (
       id         INTEGER PRIMARY KEY AUTOINCREMENT,
       timestamp  TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
       project    TEXT    NOT NULL,
       tier       TEXT
     );
     CREATE TABLE IF NOT EXISTS quota_buckets (
       id                  INTEGER PRIMARY KEY AUTOINCREMENT,
       snapshot_id         INTEGER NOT NULL REFERENCES quota_snapshots(id) ON DELETE CASCADE,
       group_name          TEXT    NOT NULL,
       bucket_name         TEXT    NOT NULL,
       bucket_id           TEXT,
       window              TEXT,
       remaining_fraction  REAL,
       remaining_percent   REAL,
       reset_time          TEXT
     );
     CREATE INDEX IF NOT EXISTS idx_snapshots_timestamp ON quota_snapshots(timestamp);
     CREATE INDEX IF NOT EXISTS idx_buckets_snapshot    ON quota_buckets(snapshot_id);",
    )
    .context("failed to initialize history database schema")?;

  Ok(conn)
}

/// Inserts a quota snapshot with all its bucket data into the database.
pub fn save_snapshot(conn: &rusqlite::Connection, project: &str, tier: Option<&str>, groups: &[crate::models::QuotaGroup]) -> Result<i64> {
  conn.execute("INSERT INTO quota_snapshots (project, tier) VALUES (?1, ?2)", rusqlite::params![project, tier]).context("failed to insert quota snapshot")?;

  let snapshot_id = conn.last_insert_rowid();

  let mut stmt = conn
    .prepare_cached(
      "INSERT INTO quota_buckets
         (snapshot_id, group_name, bucket_name, bucket_id, window, remaining_fraction, remaining_percent, reset_time)
       VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )
    .context("failed to prepare bucket insert")?;

  for group in groups {
    let group_name = group.name();
    for bucket in &group.buckets {
      stmt
        .execute(rusqlite::params![snapshot_id, group_name, bucket.name(), bucket.bucket_id, bucket.window, bucket.remaining_fraction, bucket.remaining_percent(), bucket.reset_time,])
        .context("failed to insert quota bucket")?;
    }
  }

  Ok(snapshot_id)
}

/// A row from the history query for display.
#[derive(Debug, Clone)]
pub struct HistoryRow {
  pub timestamp: String,
  pub project: String,
  pub tier: Option<String>,
  pub group_name: String,
  pub bucket_name: String,
  pub remaining_percent: Option<f64>,
  pub reset_time: Option<String>,
}

/// Queries recent history records, returning the last `limit` snapshots
/// with all their buckets.
pub fn query_recent(conn: &rusqlite::Connection, limit: u32) -> Result<Vec<HistoryRow>> {
  let mut stmt = conn
    .prepare(
      "SELECT s.timestamp, s.project, s.tier,
              b.group_name, b.bucket_name, b.remaining_percent, b.reset_time
       FROM quota_snapshots s
       JOIN quota_buckets b ON b.snapshot_id = s.id
       WHERE s.id IN (
         SELECT id FROM quota_snapshots ORDER BY id DESC LIMIT ?1
       )
       ORDER BY s.timestamp DESC, b.group_name, b.bucket_name",
    )
    .context("failed to prepare history query")?;

  let rows = stmt
    .query_map(rusqlite::params![limit], |row| {
      Ok(HistoryRow { timestamp: row.get(0)?, project: row.get(1)?, tier: row.get(2)?, group_name: row.get(3)?, bucket_name: row.get(4)?, remaining_percent: row.get(5)?, reset_time: row.get(6)? })
    })
    .context("history query failed")?;

  rows.collect::<std::result::Result<Vec<_>, _>>().context("failed to read history rows")
}

/// Formats and prints history rows as a human-readable table.
pub fn print_history(rows: &[HistoryRow]) {
  if rows.is_empty() {
    println!("No quota history records found.");
    return;
  }

  let ts_w = 20;
  let grp_w = rows.iter().map(|r| r.group_name.len()).max().unwrap_or(5).max(5);
  let bkt_w = rows.iter().map(|r| r.bucket_name.len()).max().unwrap_or(6).max(6);
  let pct_w = 10;

  println!("{:<ts_w$}  {:<grp_w$}  {:<bkt_w$}  {:>pct_w$}", "Timestamp", "Group", "Bucket", "Remaining");
  println!("{:<ts_w$}  {:<grp_w$}  {:<bkt_w$}  {:>pct_w$}", "-".repeat(ts_w), "-".repeat(grp_w), "-".repeat(bkt_w), "-".repeat(pct_w));

  for row in rows {
    let pct = row.remaining_percent.map(|p| format!("{p:.1}%")).unwrap_or_else(|| "N/A".to_string());

    println!("{:<ts_w$}  {:<grp_w$}  {:<bkt_w$}  {:>pct_w$}", row.timestamp, row.group_name, row.bucket_name, pct);
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::models::{QuotaBucket, QuotaGroup};

  fn test_db() -> rusqlite::Connection {
    open_db(std::path::Path::new(":memory:")).unwrap()
  }

  #[test]
  fn creates_schema_and_saves_snapshot() {
    let conn = test_db();
    let groups = vec![QuotaGroup {
      display_name: Some("Gemini Models".into()),
      description: None,
      buckets: vec![QuotaBucket {
        bucket_id: Some("daily".into()),
        window: Some("DAY".into()),
        display_name: Some("Daily Requests".into()),
        description: None,
        remaining_fraction: Some(0.75),
        reset_time: Some("2026-09-26T00:00:00Z".into()),
      }],
    }];

    let id = save_snapshot(&conn, "my-project", Some("Pro"), &groups).unwrap();
    assert_eq!(id, 1);

    let rows = query_recent(&conn, 10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].project, "my-project");
    assert_eq!(rows[0].tier.as_deref(), Some("Pro"));
    assert_eq!(rows[0].group_name, "Gemini Models");
    assert_eq!(rows[0].bucket_name, "Daily Requests");
    assert!((rows[0].remaining_percent.unwrap() - 75.0).abs() < 0.01);
  }

  #[test]
  fn query_recent_respects_limit() {
    let conn = test_db();
    let groups = vec![QuotaGroup {
      display_name: Some("Test".into()),
      description: None,
      buckets: vec![QuotaBucket { bucket_id: Some("b1".into()), window: None, display_name: Some("Bucket".into()), description: None, remaining_fraction: Some(0.5), reset_time: None }],
    }];

    for _ in 0..5 {
      save_snapshot(&conn, "proj", None, &groups).unwrap();
    }

    let rows = query_recent(&conn, 2).unwrap();
    // 2 snapshots × 1 bucket each = 2 rows
    assert_eq!(rows.len(), 2);
  }

  #[test]
  fn empty_history_returns_empty() {
    let conn = test_db();
    let rows = query_recent(&conn, 10).unwrap();
    assert!(rows.is_empty());
  }
}
