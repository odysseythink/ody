use std::fs;
use std::path::Path;

fn main() {
    // `include_dir!("src/assets/embedded")` embeds at compile time but does not
    // register the directory with cargo's change tracking; without this,
    // editing an embedded skill would not rebuild the crate.
    let embedded_dir = Path::new("src/assets/embedded");
    if embedded_dir.exists() {
        println!("cargo:rerun-if-changed={}", embedded_dir.display());
        visit_dir(embedded_dir);
    }

    let samples_dir = Path::new("src/assets/samples");
    if !samples_dir.exists() {
        return;
    }

    println!("cargo:rerun-if-changed={}", samples_dir.display());
    visit_dir(samples_dir);
}

fn visit_dir(dir: &Path) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        println!("cargo:rerun-if-changed={}", path.display());
        if path.is_dir() {
            visit_dir(&path);
        }
    }
}
