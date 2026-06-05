use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    // Only the dist outputs are direct inputs to this script: when they change
    // (i.e. after a `yarn build`), Cargo re-runs build.rs to re-copy them into
    // OUT_DIR. Source files (jsx, css) are not listed here — they are not direct
    // inputs to build.rs; they affect the dist only after an explicit yarn build,
    // at which point the dist timestamps change and trigger the rerun correctly.
    println!("cargo:rerun-if-changed=src/design/dist/index.html");
    println!("cargo:rerun-if-changed=src/design/dist/assets/app.js");
    println!("cargo:rerun-if-changed=src/design/dist/assets/app.css");

    let index = Path::new("src/design/dist/index.html");
    let app_js = Path::new("src/design/dist/assets/app.js");
    let app_css = Path::new("src/design/dist/assets/app.css");
    let outputs = [index, app_js, app_css];

    let dist_missing = outputs.iter().any(|p| !p.exists());

    if dist_missing {
        let npm = if cfg!(windows) { "npm.cmd" } else { "npm" };
        if !Path::new("src/design/node_modules").exists() {
            let status = Command::new(npm)
                .args(["ci", "--prefix", "src/design"])
                .status()
                .expect("failed to run npm ci — install Node.js/npm or run `yarn --cwd src/design install` manually");
            assert!(status.success(), "npm ci failed");
        }
        let status = Command::new(npm)
            .args(["run", "build", "--prefix", "src/design"])
            .status()
            .expect("failed to run npm run build — install Node.js/npm or run `yarn --cwd src/design build` manually");
        assert!(status.success(), "npm run build failed");
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
