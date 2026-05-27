use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string());
    println!("cargo:rustc-env=MARG_BUILD_TIMESTAMP={}", timestamp);

    // Expose the resolved Kavach versions to runtime via env vars so /version
    // and the x-kavach-version response header report what Cargo actually
    // linked in (not Marg's own version). Cargo writes
    // `DEP_<package>_<key>` style env vars only for crates that publish
    // metadata via [package.metadata.links]; kavach does not, so the simplest
    // shape is to read CARGO_PKG_VERSION at the dep level via the resolved
    // Cargo.lock entries. We do that by reading the lockfile here at build
    // time and grepping for the kavach-core entry.
    emit_kavach_versions();

    embed_console();
}

fn emit_kavach_versions() {
    let manifest = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    // The workspace lockfile lives one level up at marg/Cargo.lock.
    let lock_path = Path::new(&manifest).join("..").join("Cargo.lock");
    println!("cargo:rerun-if-changed={}", lock_path.display());

    let text = fs::read_to_string(&lock_path).unwrap_or_default();
    for crate_name in ["kavach-core", "kavach-pq", "kavach-redis"] {
        let version = find_lock_version(&text, crate_name).unwrap_or_else(|| "unknown".to_string());
        let env_key = format!(
            "MARG_{}_VERSION",
            crate_name.to_uppercase().replace('-', "_")
        );
        println!("cargo:rustc-env={}={}", env_key, version);
    }
}

/// Parse Cargo.lock manually for `name = "<crate>"` followed by `version = "<v>"`.
/// Cargo.lock is TOML, but pulling in toml just for this would add a build-only
/// dep. The grep is tight and well-shaped enough that the manual scan is fine.
fn find_lock_version(text: &str, name: &str) -> Option<String> {
    let needle = format!("name = \"{}\"", name);
    let pos = text.find(&needle)?;
    let after = &text[pos + needle.len()..];
    let version_line = after.lines().take(5).find(|l| l.trim_start().starts_with("version ="))?;
    let stripped = version_line.trim_start().trim_start_matches("version = ").trim();
    let value = stripped.trim_matches('"');
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn embed_console() {
    let manifest = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let console_dist = Path::new(&manifest).join("..").join("console").join("dist");
    let out_dir = env::var("OUT_DIR").expect("OUT_DIR");
    let out_path = Path::new(&out_dir).join("console_embed.rs");

    let mut entries: Vec<(String, PathBuf)> = Vec::new();
    if console_dist.exists() {
        collect(&console_dist, &console_dist, &mut entries);
    }

    let mut src = String::new();
    src.push_str("pub static CONSOLE_FILES: &[(&str, &[u8], &str)] = &[\n");
    for (rel, abs) in &entries {
        let mime = guess_mime(rel);
        let abs_str = abs.to_string_lossy().into_owned();
        src.push_str(&format!(
            "    ({:?}, include_bytes!({:?}), {:?}),\n",
            rel, abs_str, mime
        ));
        println!("cargo:rerun-if-changed={}", abs_str);
    }
    src.push_str("];\n");
    src.push_str(&format!(
        "pub const CONSOLE_FILE_COUNT: usize = {};\n",
        entries.len()
    ));

    fs::write(&out_path, src).expect("writing console_embed.rs");

    // Re-run when the dist directory itself appears or disappears.
    println!(
        "cargo:rerun-if-changed={}",
        console_dist.display()
    );
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let read = match fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => return,
    };
    for entry in read.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, out);
        } else if path.is_file() {
            if let Ok(rel) = path.strip_prefix(root) {
                let rel_str = rel.to_string_lossy().replace('\\', "/");
                out.push((rel_str, path));
            }
        }
    }
}

fn guess_mime(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("");
    match ext {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "application/javascript; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "webp" => "image/webp",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "txt" => "text/plain; charset=utf-8",
        "map" => "application/json; charset=utf-8",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}
