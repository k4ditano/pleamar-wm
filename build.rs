//! `--features exp`: the experimental pleamar's `desk` slots (a scene's `windows … desk N`, the
//! programs' widgets on the desktop drawn inside the scene). It needs pleamar 0.8.0-exp.22 or
//! newer beside it (`../pleamar`), which says so in its `Cargo.toml`:
//!
//! ```toml
//! [package.metadata.pleamar-wm]
//! desk = true
//! ```
//!
//! With the feature and another pleamar, the build stops here and says why, instead of failing
//! later on names that pleamar does not have. Without the feature this does nothing.

fn main() {
    println!("cargo:rerun-if-changed=../pleamar/Cargo.toml");
    if std::env::var_os("CARGO_FEATURE_EXP").is_none() {
        return;
    }
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../pleamar/Cargo.toml");
    let text = std::fs::read_to_string(&manifest).unwrap_or_default();
    let mut inside = false;
    let marked = text.lines().map(str::trim).any(|line| {
        if line.starts_with('[') {
            inside = line == "[package.metadata.pleamar-wm]";
            return false;
        }
        inside && line.split_once('=').is_some_and(|(k, v)| k.trim() == "desk" && v.trim() == "true")
    });
    if !marked {
        panic!(
            "\n\n`--features exp` needs the experimental pleamar, 0.8.0-exp.22 or newer, beside pleamar-wm ({}): \
             the one there does not say `desk = true` in `[package.metadata.pleamar-wm]` (pleamar main, or an older exp). \
             Build without `--features exp` for pleamar main.\n\n",
            manifest.display()
        );
    }
}
