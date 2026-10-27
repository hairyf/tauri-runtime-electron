use std::{env, fs, io, path::{Path, PathBuf}};

fn main() -> io::Result<()> {
  println!("cargo:rerun-if-changed=assets");
  println!("cargo:rerun-if-changed=native/appkit.mm");
  let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").ok_or_else(|| io::Error::other("missing CARGO_MANIFEST_DIR"))?);
  let mut files = Vec::new();
  collect(&root.join("assets"), &mut files)?;
  files.sort();
  if !files.iter().any(|file| file == &root.join("assets/main.cjs")) {
    return Err(io::Error::other("missing Electron assets; run pnpm build:electron before Cargo"));
  }
  let mut source = String::from("const ASSETS: &[(&str, &[u8])] = &[\n");
  for file in files {
    let name = file.strip_prefix(root.join("assets")).map_err(io::Error::other)?.to_string_lossy().replace('\\', "/");
    source.push_str(&format!("  ({name:?}, include_bytes!({:?})),\n", file.to_string_lossy()));
  }
  source.push_str(&format!("  (\"appkit.mm\", include_bytes!({:?})),\n];\n", root.join("native/appkit.mm").to_string_lossy()));
  let output = PathBuf::from(env::var_os("OUT_DIR").ok_or_else(|| io::Error::other("missing OUT_DIR"))?).join("assets.rs");
  fs::write(output, source)
}

fn collect(directory: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
  for entry in fs::read_dir(directory)? {
    let entry = entry?;
    let kind = entry.file_type()?;
    if kind.is_dir() { collect(&entry.path(), files)?; }
    else if kind.is_file() && entry.path().extension().is_some_and(|extension| extension == "cjs") { files.push(entry.path()); }
  }
  Ok(())
}
