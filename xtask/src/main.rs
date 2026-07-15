//! Repo automation entry point.
//!
//! `cargo xtask <command>` (aliased in `.cargo/config.toml`) runs the Rust
//! implementations of tasks that used to be inline bash in the Makefile and
//! GitHub Actions workflows: building the UI, verifying a release tag
//! matches Cargo.toml, and checking the built UI dist for dev-only/CDN
//! leakage. Local dev and CI call the exact same code path, so there's one
//! implementation instead of several hand-copied shell snippets.
//!
//! Commands:
//!   setup                          Check required tooling, install the pre-commit hook
//!   build-ui                       Build src/design (corepack + yarn install + yarn build)
//!   verify-release-version <tag>   Confirm Cargo.toml's version matches a release tag
//!   check-dist                     Confirm src/design/dist exists and has no dev-only/CDN leakage
//!
//! `xtask` is a workspace member but deliberately NOT a default one (see
//! root Cargo.toml's `[workspace]` comment) — plain `cargo build` / `cargo
//! test` at the repo root are unaffected by this crate existing.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let command = match args.next() {
        Some(c) => c,
        None => return usage_error("missing command"),
    };

    let result = match command.as_str() {
        "setup" => setup(),
        "build-ui" => build_ui(),
        "verify-release-version" => match args.next() {
            Some(tag) => verify_release_version(&tag),
            None => return usage_error("verify-release-version requires a <tag> argument"),
        },
        "check-dist" => check_dist(),
        "help" | "-h" | "--help" => {
            print_usage();
            return ExitCode::SUCCESS;
        }
        other => return usage_error(&format!("unknown command '{other}'")),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("error: {msg}");
            ExitCode::FAILURE
        }
    }
}

fn usage_error(msg: &str) -> ExitCode {
    eprintln!("error: {msg}\n");
    print_usage();
    ExitCode::FAILURE
}

fn print_usage() {
    eprintln!(
        "cargo xtask <command>\n\n\
         Commands:\n\
         \x20 setup                         Check required tooling, install the pre-commit hook\n\
         \x20 build-ui                      Build the React UI (corepack + yarn install + yarn build)\n\
         \x20 verify-release-version <tag>  Check Cargo.toml version matches a release tag (e.g. v0.1.9 or 0.1.9)\n\
         \x20 check-dist                    Verify src/design/dist exists and has no dev-only/CDN leakage\n"
    );
}

/// Repo root, resolved from this crate's own manifest dir rather than the
/// process cwd, so `cargo xtask ...` behaves the same no matter where in
/// the repo it's invoked from.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask/Cargo.toml has a parent directory")
        .to_path_buf()
}

/// npm/yarn/corepack ship as `.cmd` shims on Windows; everywhere else the
/// bare name resolves via PATH. Mirrors the same check in build.rs.
fn platform_cmd(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.cmd")
    } else {
        name.to_string()
    }
}

fn run(program: &str, args: &[&str], cwd: &Path) -> Result<(), String> {
    println!("$ {program} {}", args.join(" "));
    let status = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .status()
        .map_err(|e| format!("failed to spawn `{program}`: {e} (is it installed and on PATH?)"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "`{program} {}` exited with {status}",
            args.join(" ")
        ))
    }
}

/// Checks required tooling and installs a pre-commit hook (`cargo fmt --all
/// -- --check`). Missing tools are warnings, not hard failures — e.g. only
/// `soak-test` needs bash, so its absence shouldn't block the hook install.
fn setup() -> Result<(), String> {
    let root = repo_root();

    warn_if_missing(
        "node",
        &["--version"],
        "node not found - `cargo xtask build-ui` / browser tests will fail without it",
    );

    if !tool_available("python3", &["--version"]) && !tool_available("python", &["--version"]) {
        eprintln!(
            "warning: python3 (or python) not found - `make soak-test` needs a Python 3 \
             interpreter for its origin server"
        );
    }

    warn_if_missing(
        "bash",
        &["--version"],
        "bash not found or not runnable - only `make soak-test` needs it; \
         `cargo xtask`/`cargo build`/`cargo test` do not",
    );

    let yarn = platform_cmd("yarn");
    if !yarn_available(&root, &yarn) {
        eprintln!(
            "warning: `{yarn}` isn't runnable yet - `cargo xtask build-ui` will attempt \
             `corepack enable yarn` on first use (may need one elevated run on Windows if \
             Node is under Program Files, see ensure_yarn_shim in xtask/src/main.rs)"
        );
    }

    install_pre_commit_hook(&root)?;
    Ok(())
}

fn tool_available(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn warn_if_missing(program: &str, args: &[&str], message: &str) {
    if !tool_available(program, args) {
        eprintln!("warning: {message}");
    }
}

const PRE_COMMIT_HOOK: &str =
    "#!/usr/bin/env sh\nset -e\necho \"Running cargo fmt --all...\"\ncargo fmt --all -- --check\n";

fn install_pre_commit_hook(root: &Path) -> Result<(), String> {
    let hooks_dir = root.join(".git/hooks");
    fs::create_dir_all(&hooks_dir)
        .map_err(|e| format!("failed to create {}: {e}", hooks_dir.display()))?;

    let hook_path = hooks_dir.join("pre-commit");
    fs::write(&hook_path, PRE_COMMIT_HOOK)
        .map_err(|e| format!("failed to write {}: {e}", hook_path.display()))?;

    // Unix needs the executable bit set explicitly; Windows/NTFS has no
    // equivalent permission bit and git-for-windows invokes hooks via their
    // shebang line regardless, so there's nothing to set there.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&hook_path)
            .map_err(|e| format!("failed to read metadata for {}: {e}", hook_path.display()))?
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&hook_path, perms)
            .map_err(|e| format!("failed to chmod {}: {e}", hook_path.display()))?;
    }

    println!("pre-commit hook installed: {}", hook_path.display());
    println!("  runs: cargo fmt --all -- --check");
    println!("  fix:  cargo fmt --all   (or `make fmt`)");
    Ok(())
}

/// Mirrors the Makefile `ui` target and the "Build UI" CI step: corepack
/// enable, immutable yarn install, yarn build.
fn build_ui() -> Result<(), String> {
    let root = repo_root();
    let yarn = platform_cmd("yarn");

    ensure_yarn_shim(&root, &yarn)?;
    run(
        &yarn,
        &["--cwd", "src/design", "install", "--immutable"],
        &root,
    )?;
    run(&yarn, &["--cwd", "src/design", "build"], &root)?;

    let index = root.join("src/design/dist/index.html");
    if !index.exists() {
        return Err(format!(
            "yarn build finished but {} is missing",
            index.display()
        ));
    }
    println!("UI build OK: src/design/dist");
    Ok(())
}

/// Bare `corepack enable` shims every package manager it knows (npm, pnpm,
/// pnpx, yarn) into the Node install dir. On Windows, if Node lives under
/// `C:\Program Files\nodejs`, that's admin-protected, so a non-elevated
/// `corepack enable` fails with EPERM on `pnpx` before it ever reaches
/// `yarn`. `corepack enable yarn` scopes the shim to yarn only, avoiding it.
///
/// Checks the resolved yarn's *version*, not just that it runs: a stray
/// classic Yarn 1.x from an old global install resolves via PATH just as
/// easily as the corepack shim and can silently corrupt yarn.lock back to
/// the old v1 format.
fn ensure_yarn_shim(root: &Path, yarn: &str) -> Result<(), String> {
    if yarn_matches_pinned_version(root, yarn) {
        return Ok(());
    }

    let corepack = platform_cmd("corepack");
    if run(&corepack, &["enable", "yarn"], root).is_ok() {
        return Ok(());
    }

    Err(format!(
        "`{corepack} enable yarn` failed and `{yarn}` still isn't the pinned version. On \
         Windows this is usually because Node is installed under 'C:\\Program \
         Files\\nodejs', which needs admin rights to write shims into. Fix: open \
         one terminal as Administrator, run `corepack enable yarn` there once, \
         then re-run `cargo xtask build-ui` normally — or install Node somewhere \
         user-writable (e.g. via nvm-windows) to avoid the elevation requirement \
         altogether. If a `yarn` already resolves on PATH but is the wrong version \
         (e.g. an old global Yarn Classic install), remove it or put the corepack \
         shim earlier on PATH."
    ))
}

/// Reads `src/design/package.json`'s `"packageManager": "yarn@X.Y.Z"` field.
/// Deliberately simple string scanning, consistent with this crate's
/// std-only scope — same approach as `parse_package_version`.
fn pinned_yarn_version(root: &Path) -> Option<String> {
    let contents = fs::read_to_string(root.join("src/design/package.json")).ok()?;
    let after_key = contents.split("\"packageManager\"").nth(1)?;
    let after_at = after_key.split("yarn@").nth(1)?;
    let end = after_at.find('"')?;
    Some(after_at[..end].to_string())
}

fn yarn_matches_pinned_version(root: &Path, yarn: &str) -> bool {
    let Some(expected) = pinned_yarn_version(root) else {
        // Can't find the pin — fall back to "any yarn that runs" rather than
        // blocking build-ui over a parsing gap.
        return yarn_available(root, yarn);
    };

    Command::new(yarn)
        .arg("--version")
        .current_dir(root)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == expected)
        .unwrap_or(false)
}

fn yarn_available(root: &Path, yarn: &str) -> bool {
    Command::new(yarn)
        .arg("--version")
        .current_dir(root)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// The running binary reports CARGO_PKG_VERSION; the in-app update check
/// compares it to the latest GitHub release tag. If Cargo.toml isn't bumped
/// to match the tag being released, every build of that release would
/// report "update available" against itself — fail fast instead.
///
/// Ported from the inline `sed`-based check in
/// .github/workflows/release-docker-asset.yaml so it's runnable (and
/// testable) locally before a tag is ever pushed, not just in CI after.
fn verify_release_version(tag: &str) -> Result<(), String> {
    let root = repo_root();
    let cargo_toml_path = root.join("Cargo.toml");
    let contents = fs::read_to_string(&cargo_toml_path)
        .map_err(|e| format!("failed to read {}: {e}", cargo_toml_path.display()))?;

    let cargo_version = parse_package_version(&contents).ok_or_else(|| {
        format!(
            "could not find a `version = \"...\"` line under [package] in {}",
            cargo_toml_path.display()
        )
    })?;

    let tag_version = tag.strip_prefix('v').unwrap_or(tag);

    if tag_version != cargo_version {
        return Err(format!(
            "release tag '{tag}' (version '{tag_version}') does not match Cargo.toml version \
             '{cargo_version}'. Bump [package].version in Cargo.toml to {tag_version} before \
             publishing the release."
        ));
    }

    println!("OK: release tag '{tag}' matches Cargo.toml version '{cargo_version}'");
    Ok(())
}

/// Deliberately simple string parsing instead of a `toml` dependency —
/// consistent with this crate's std-only scope. Scans only the [package]
/// table (the first table in the manifest) so a `version` key belonging to
/// a dependency further down the file is never mistaken for the package
/// version.
fn parse_package_version(cargo_toml: &str) -> Option<String> {
    for line in cargo_toml.lines() {
        let line = line.trim();
        if line.starts_with('[') && line != "[package]" {
            break;
        }
        if let Some(rest) = line.strip_prefix("version") {
            let rest = rest.trim_start();
            let Some(rest) = rest.strip_prefix('=') else {
                continue;
            };
            let rest = rest.trim();
            let Some(rest) = rest.strip_prefix('"') else {
                continue;
            };
            if let Some(end) = rest.find('"') {
                return Some(rest[..end].to_string());
            }
        }
    }
    None
}

// Must never appear in the shipped UI bundle. Ported from the `grep -R`
// checks in release-docker-asset.yaml.
const CDN_AND_DEV_MARKERS: &[&str] = &[
    "text/babel",
    "unpkg",
    "cdn.jsdelivr",
    "fonts.googleapis",
    "fonts.gstatic",
];

const DEV_ONLY_UI_COPY: &[&str] = &[
    "Trust status",
    "oproxy probes the OS keychain",
    "Command palette",
    "command palette",
    "Save session log",
    "Pause / resume recording",
];

fn check_dist() -> Result<(), String> {
    let root = repo_root();
    let dist = root.join("src/design/dist");

    for rel in ["index.html", "assets/app.js", "assets/app.css"] {
        let p = dist.join(rel);
        if !p.exists() {
            return Err(format!(
                "{} is missing — run `cargo xtask build-ui` first",
                p.display()
            ));
        }
    }

    scan_for_markers(&dist, CDN_AND_DEV_MARKERS, "CDN/dev-mode reference")?;

    let management_rs = root.join("src/management.rs");
    if management_rs.exists() {
        scan_for_markers(
            &management_rs,
            CDN_AND_DEV_MARKERS,
            "CDN/dev-mode reference",
        )?;
    }

    scan_for_markers(
        &dist,
        DEV_ONLY_UI_COPY,
        "dev-only UI copy that shouldn't ship",
    )?;

    println!("OK: src/design/dist is present and free of known dev-only/CDN leakage");
    Ok(())
}

fn scan_for_markers(target: &Path, markers: &[&str], label: &str) -> Result<(), String> {
    let mut hits = Vec::new();
    walk(target, &mut |file| {
        if let Ok(contents) = fs::read_to_string(file) {
            for marker in markers {
                if contents.contains(marker) {
                    hits.push(format!("{}: contains '{}'", file.display(), marker));
                }
            }
        }
    })?;

    if hits.is_empty() {
        Ok(())
    } else {
        Err(format!("found {label}:\n  {}", hits.join("\n  ")))
    }
}

fn walk(path: &Path, visit: &mut impl FnMut(&Path)) -> Result<(), String> {
    if path.is_file() {
        visit(path);
        return Ok(());
    }
    let entries =
        fs::read_dir(path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    for entry in entries {
        let entry =
            entry.map_err(|e| format!("failed to read an entry in {}: {e}", path.display()))?;
        let p = entry.path();
        if p.is_dir() {
            walk(&p, visit)?;
        } else {
            visit(&p);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_package_version() {
        let toml = "[package]\nname = \"oproxy\"\nversion = \"0.1.9\"\nedition = \"2024\"\n\n[dependencies]\nfoo = { version = \"9.9.9\" }\n";
        assert_eq!(parse_package_version(toml).as_deref(), Some("0.1.9"));
    }

    #[test]
    fn missing_version_returns_none() {
        let toml = "[package]\nname = \"oproxy\"\n";
        assert_eq!(parse_package_version(toml), None);
    }

    #[test]
    fn tag_with_v_prefix_matches() {
        let root = std::env::temp_dir().join(format!("xtask-test-{}", std::process::id()));
        let _ = fs::create_dir_all(&root);
        fs::write(
            root.join("Cargo.toml_for_test"),
            "[package]\nversion = \"1.2.3\"\n",
        )
        .unwrap();
        let contents = fs::read_to_string(root.join("Cargo.toml_for_test")).unwrap();
        let version = parse_package_version(&contents).unwrap();
        assert_eq!(version, "1.2.3");
        assert_eq!("v1.2.3".strip_prefix('v').unwrap(), version);
        let _ = fs::remove_dir_all(&root);
    }
}
