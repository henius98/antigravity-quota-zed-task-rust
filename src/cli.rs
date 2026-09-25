use anyhow::{Result, anyhow, bail};
use std::env;
use std::path::PathBuf;

/// The desired output formatting style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
  #[default]
  Human,
  Json,
  Raw,
}

/// The top-level command to execute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
  /// Check quota (the default behaviour).
  Check(Options),
  /// Show quota history from the SQLite database.
  History { last: u32 },
}

/// Parsed command line options.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
  pub json_output: bool,
  pub raw_output: bool,
  pub token_file: Option<String>,
}

impl Options {
  /// Parses options from an argument slice (typically `std::env::args().skip(1)`).
  pub fn parse(args: &[String]) -> Result<Self> {
    parse_args(args)
  }

  /// Determines the requested output format.
  pub fn output_format(&self) -> OutputFormat {
    if self.json_output {
      OutputFormat::Json
    } else if self.raw_output {
      OutputFormat::Raw
    } else {
      OutputFormat::Human
    }
  }

  /// Resolves the ACP token file path based on flags, environment variables, or default location.
  pub fn resolve_token_path(&self) -> Result<PathBuf> {
    resolve_token_path(self.token_file.as_deref())
  }
}

/// Parses the full argument list into a [`Command`].
pub fn parse_command(args: &[String]) -> Result<Command> {
  if args.first().map(|s| s.as_str()) == Some("history") {
    let mut last: u32 = 20;
    let mut args_iter = args[1..].iter();

    while let Some(arg) = args_iter.next() {
      match arg.as_str() {
        "--last" => {
          let value = args_iter.next().ok_or_else(|| anyhow!("--last requires a number"))?;
          last = value.parse().map_err(|_| anyhow!("--last value must be a positive integer: {value}"))?;
          if last == 0 {
            bail!("--last value must be at least 1");
          }
        }
        _ => bail!("unknown history argument: {arg}"),
      }
    }

    return Ok(Command::History { last });
  }

  Ok(Command::Check(parse_args(args)?))
}

/// Parses command line arguments into [`Options`].
pub fn parse_args(args: &[String]) -> Result<Options> {
  let mut options = Options::default();
  let mut args_iter = args.iter();

  while let Some(arg) = args_iter.next() {
    match arg.as_str() {
      "--json" => options.json_output = true,
      "--raw" => options.raw_output = true,
      "--token-file" => {
        let value = args_iter.next().filter(|val| !val.is_empty() && !val.starts_with('-')).ok_or_else(|| anyhow!("--token-file requires a path"))?;
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

/// Resolves the path to the ACP token JSON file:
/// 1. Explicit CLI argument (`--token-file`)
/// 2. Environment variable (`ANTIGRAVITY_ACP_TOKEN_FILE`)
/// 3. Default `~/.gemini/antigravity-acp/acp_token.json`
pub fn resolve_token_path(token_file: Option<&str>) -> Result<PathBuf> {
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

/// Expands a leading `~/` to the user's HOME directory if present.
pub fn expand_home(value: &str) -> PathBuf {
  if let Some(rest) = value.strip_prefix("~/")
    && let Some(home) = env::var_os("HOME")
  {
    return PathBuf::from(home).join(rest);
  }
  PathBuf::from(value)
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
  fn determines_output_format() {
    assert_eq!(Options::default().output_format(), OutputFormat::Human);
    assert_eq!(Options { json_output: true, ..Default::default() }.output_format(), OutputFormat::Json);
    assert_eq!(Options { raw_output: true, ..Default::default() }.output_format(), OutputFormat::Raw);
  }

  #[test]
  fn expands_home_prefix() {
    if let Some(home) = env::var_os("HOME") {
      let expanded = expand_home("~/test/path.json");
      assert_eq!(expanded, PathBuf::from(home).join("test/path.json"));
    }
    assert_eq!(expand_home("/absolute/path.json"), PathBuf::from("/absolute/path.json"));
  }

  #[test]
  fn parses_history_command() {
    let cmd = parse_command(&["history".into()]).unwrap();
    assert_eq!(cmd, Command::History { last: 20 });
  }

  #[test]
  fn parses_history_with_last() {
    let cmd = parse_command(&["history".into(), "--last".into(), "5".into()]).unwrap();
    assert_eq!(cmd, Command::History { last: 5 });
  }

  #[test]
  fn parses_check_command_as_default() {
    let cmd = parse_command(&["--json".into()]).unwrap();
    assert_eq!(cmd, Command::Check(Options { json_output: true, ..Default::default() }));
  }

  #[test]
  fn rejects_history_last_zero() {
    assert!(parse_command(&["history".into(), "--last".into(), "0".into()]).is_err());
  }
}
