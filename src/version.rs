pub fn print_version() {
  const ARCH: &str = "amd64";
  const VERSION: &str = "0.1.1";
  const COMMIT_ID: &str = "5be2fc8f";

  println!("Arch: {}", ARCH);
  println!("Version: {}", VERSION);
  println!("Commit Id: {}", COMMIT_ID);
}
