use std::{env, fs, io, path::Path};

fn copy_directory(source: &Path, target: &Path) -> io::Result<()> {
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_directory(&entry.path(), &target.join(entry.file_name()))?;
        } else if kind.is_file() {
            fs::copy(entry.path(), target.join(entry.file_name()))?;
        }
    }
    Ok(())
}

fn main() {
    println!("cargo:rerun-if-changed=ui/dist");
    println!("cargo:rerun-if-changed=build.rs");
    let output = std::path::PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let target = output.join("ui");
    if target.exists() {
        fs::remove_dir_all(&target).expect("remove stale bundled UI");
    }
    fs::create_dir_all(&target).expect("create bundled UI directory");
    if Path::new("ui/dist/index.html").is_file() {
        copy_directory(Path::new("ui/dist"), &target).expect("copy dashboard build");
    } else {
        fs::write(target.join("index.html"), r#"<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Observer — dashboard build needed</title><body><main><h1>Observer is running</h1><p>The dashboard has not been built into this executable yet.</p><p>From the project directory, run <code>npm --prefix ui install</code>, then <code>npm --prefix ui run build</code>, then rebuild with <code>cargo build --release</code>.</p><p>The local API is available at <a href="/api/dashboard">/api/dashboard</a>.</p></main></body></html>"#).expect("write dashboard build instructions");
    }
}
