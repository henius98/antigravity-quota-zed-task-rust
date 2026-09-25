fn main() {
  if let Err(err) = antigravity_quota::run() {
    eprintln!("Antigravity quota error: {err:#}");
    std::process::exit(1);
  }
}
