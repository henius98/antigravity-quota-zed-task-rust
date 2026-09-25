pub mod auth;
pub mod cli;
pub mod client;
pub mod history;
pub mod models;
pub mod output;

use anyhow::{Context, Result};
use std::env;

pub use cli::{Command, Options, OutputFormat};
pub use models::{LoadCodeAssistResponse, QuotaBucket, QuotaGroup, QuotaSummaryResponse, Tier};

/// Runs the Antigravity quota retrieval workflow using command-line arguments.
pub fn run() -> Result<()> {
  let args: Vec<String> = env::args().skip(1).collect();
  let command = cli::parse_command(&args)?;

  match command {
    Command::Check(options) => run_with_options(&options),
    Command::History { last } => run_history(last),
  }
}

/// Executes quota retrieval with the provided [`Options`], then saves to history.
pub fn run_with_options(options: &Options) -> Result<()> {
  let token_path = options.resolve_token_path()?;
  let token_json = auth::read_token_json(&token_path)?;

  let http_client = client::create_http_client()?;
  let access_token = auth::resolve_access_token(&http_client, &token_json, &token_path)?;

  let fallback_project = auth::find_named_string(&token_json, "project_id").or_else(|| auth::find_named_string(&token_json, "projectId"));

  let (load, raw_quota) = client::fetch_quota(&http_client, &access_token, fallback_project, client::DEFAULT_BASE_URLS)?;

  let project = load.cloudaicompanion_project.as_deref().expect("fetch_quota requires a project");

  if options.raw_output {
    output::print_output(OutputFormat::Raw, project, load.tier_name().as_deref(), &[], &raw_quota)?;
    // Save raw output — we don't have parsed groups, so save an empty snapshot.
    save_to_history(project, load.tier_name().as_deref(), &[]);
    return Ok(());
  }

  let quota: QuotaSummaryResponse = serde_json::from_value(raw_quota).context("Google returned an unexpected quota response shape")?;

  output::print_output(options.output_format(), project, load.tier_name().as_deref(), &quota.groups, &serde_json::Value::Null)?;

  // Save quota snapshot to history database.
  save_to_history(project, load.tier_name().as_deref(), &quota.groups);

  Ok(())
}

/// Saves a quota snapshot to the history database, printing a warning on failure
/// rather than aborting the program.
fn save_to_history(project: &str, tier: Option<&str>, groups: &[QuotaGroup]) {
  if let Err(err) = try_save_to_history(project, tier, groups) {
    eprintln!("Warning: failed to save quota history: {err:#}");
  }
}

fn try_save_to_history(project: &str, tier: Option<&str>, groups: &[QuotaGroup]) -> Result<()> {
  let db_path = history::default_db_path()?;
  let conn = history::open_db(&db_path)?;
  history::save_snapshot(&conn, project, tier, groups)?;
  Ok(())
}

/// Shows recent quota history from the SQLite database.
fn run_history(last: u32) -> Result<()> {
  let db_path = history::default_db_path()?;
  let conn = history::open_db(&db_path)?;
  let rows = history::query_recent(&conn, last)?;
  history::print_history(&rows);
  Ok(())
}
