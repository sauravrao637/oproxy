use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let design_sources = [
        "src/design/package.json",
        "src/design/package-lock.json",
        "src/design/build.mjs",
        "src/design/index.html",
        "src/design/app.jsx",
        "src/design/compose.jsx",
        "src/design/detail-panel.jsx",
        "src/design/entry.jsx",
        "src/design/icons.jsx",
        "src/design/redaction.jsx",
        "src/design/sessions-table.jsx",
        "src/design/styles.css",
        "src/design/surfaces-extra.jsx",
        "src/design/surfaces.jsx",
        "src/design/tweaks-panel.jsx",
    ];
    for path in design_sources {
        println!("cargo:rerun-if-changed={path}");
    }
    println!("cargo:rerun-if-changed=src/design/dist/index.html");
    println!("cargo:rerun-if-changed=src/design/dist/assets/app.js");
    println!("cargo:rerun-if-changed=src/design/dist/assets/app.css");

    let index = Path::new("src/design/dist/index.html");
    let app_js = Path::new("src/design/dist/assets/app.js");
    let app_css = Path::new("src/design/dist/assets/app.css");

    if needs_design_build(&design_sources, &[index, app_js, app_css]) {
        let npm = if cfg!(windows) { "npm.cmd" } else { "npm" };
        if !Path::new("src/design/node_modules").exists() {
            let install = Command::new(npm)
                .args(["ci", "--prefix", "src/design"])
                .status()
                .expect("failed to run npm ci --prefix src/design");
            assert!(
                install.success(),
                "npm ci --prefix src/design failed; install Node.js/npm or run the UI build manually"
            );
        }

        let build = Command::new(npm)
            .args(["run", "build", "--prefix", "src/design"])
            .status()
            .expect("failed to run npm run build --prefix src/design");
        assert!(
            build.success(),
            "npm run build --prefix src/design failed; Rust compile requires built UI assets"
        );
    }

    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR is set by cargo"));
    let design_out = out_dir.join("design-dist");
    let assets_out = design_out.join("assets");
    fs::create_dir_all(&assets_out).expect("failed to create generated design asset directory");
    fs::copy(index, design_out.join("index.html")).expect("failed to copy generated index.html");
    fs::copy(app_js, assets_out.join("app.js")).expect("failed to copy generated app.js");
    fs::copy(app_css, assets_out.join("app.css")).expect("failed to copy generated app.css");

    fs::write(
        out_dir.join("design_assets.rs"),
        r#"pub const INDEX_HTML: &str = include_str!(concat!(env!("OUT_DIR"), "/design-dist/index.html"));
pub const APP_CSS: &str = include_str!(concat!(env!("OUT_DIR"), "/design-dist/assets/app.css"));
pub const APP_JS: &str = include_str!(concat!(env!("OUT_DIR"), "/design-dist/assets/app.js"));
"#,
    )
    .expect("failed to write generated design asset module");
}

fn needs_design_build(sources: &[&str], outputs: &[&Path]) -> bool {
    if outputs.iter().any(|path| !path.exists()) {
        return true;
    }

    let oldest_output = outputs
        .iter()
        .filter_map(|path| fs::metadata(path).ok()?.modified().ok())
        .min();
    let newest_source = sources
        .iter()
        .filter_map(|path| fs::metadata(path).ok()?.modified().ok())
        .max();

    match (newest_source, oldest_output) {
        (Some(source), Some(output)) => source > output,
        _ => true,
    }
}
