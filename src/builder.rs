use crate::cache;
use crate::config::BuildBackend as BuildBackendKind;
use crate::config::{BuildCompiler, CookConfig, DependencyDetail, DependencySpec};
use crate::backends::{BuildBackend, BuildPlan, BuildProfile, CmakeBackend, CommandSpec, MakeBackend};
use crate::lockfile::CookLock;
use crate::registry;
use crate::resolver::{self, ExternalBuildSystem, ResolveOptions, ResolvedDependency};
use crate::scanner;
use anyhow::{Context, Result, bail};
use reqwest::blocking::Client;
use reqwest::StatusCode;
use std::collections::{BTreeMap, HashMap};
use std::env;
use std::fs;
use std::io::{Cursor, copy};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use zip::ZipArchive;

pub fn build_project(release: bool) -> Result<PathBuf> {
    build_project_with_profile(profile_from_release(release))
}

pub fn run_project(release: bool) -> Result<()> {
    let artifact = build_project(release)?;
    let status = Command::new(&artifact)
        .status()
        .with_context(|| format!("failed to execute {}", artifact.display()))?;

    if !status.success() {
        bail!("binary execution failed with status {}", status);
    }

    Ok(())
}

pub fn add_dependency(name: &str, url: Option<&str>) -> Result<()> {
    if name.trim().is_empty() {
        bail!("dependency name cannot be empty");
    }

    let mut config = load_config()?;
    if config.dependencies.contains_key(name) {
        bail!("dependency '{}' already exists in cook.toml", name);
    }

    let mut detail = if let Some(explicit_url) = url {
        if explicit_url.trim().is_empty() {
            bail!("dependency source URL cannot be empty");
        }

        DependencyDetail {
            git: Some(explicit_url.to_string()),
            ..DependencyDetail::default()
        }
    } else {
        let resolved = registry::resolve_package_from_cookbook(name)?;
        println!(
            "cook: package '{}' resolved from cookbook as '{}'",
            name,
            resolved.dependency_name
        );
        resolved.detail
    };

    let source_url = detail
        .git
        .clone()
        .context("dependency source URL is missing")?;
    let is_git_source = is_direct_git_repo_url(&source_url);

    let added_message = if is_git_source {
        let rev = if let Some(rev) = detail.rev.clone() {
            rev
        } else {
            resolve_remote_head_rev(&source_url)?
        };
        detail.rev = Some(rev.clone());
        format!("cook: dependency '{name}' added at rev {rev}")
    } else {
        let local_dep_path = materialize_archive_dependency(name, &source_url)?;
        detail.path = Some(local_dep_path);
        detail.git = None;
        detail.rev = None;
        detail.branch = None;
        detail.tag = None;
        format!(
            "cook: dependency '{name}' added from archive source {}",
            source_url
        )
    };

    config.dependencies.insert(
        name.to_string(),
        DependencySpec::Detailed(Box::new(detail)),
    );

    let rendered = toml::to_string_pretty(&config)
        .context("failed to serialize updated cook.toml")?;
    fs::write("cook.toml", rendered).context("failed to write cook.toml")?;
    println!("{added_message}");

    Ok(())
}

fn build_project_with_profile(profile: BuildProfile) -> Result<PathBuf> {
    let config = load_config()?;

    fs::create_dir_all(".cook/deps")?;
    fs::create_dir_all("target")?;
    fs::create_dir_all(Path::new("target").join(profile.as_str()))?;
    fs::create_dir_all("target/.cook-cache")?;

    let resolution = resolver::resolve_from_manifest(
        "cook.toml",
        ResolveOptions {
            workspace_root: PathBuf::from("."),
            offline: config.build.offline,
        },
    )?;
    if !resolution.build_order.is_empty() {
        println!(
            "cook: resolved {} transitive package(s)",
            resolution.build_order.len()
        );
    }
    let lock = CookLock::from_resolution(&resolution);
    enforce_lock_policy(&config, &lock, Path::new("cook.lock"))?;

    let mut resolved_dependencies = resolution.direct_dependencies;
    prepare_external_dependencies(&mut resolved_dependencies, profile, config.build.compiler)?;

    let sources = scanner::discover_files("src")?;
    if sources.is_empty() {
        bail!("no C/C++ source files found under src/");
    }

    let plan = BuildPlan {
        project_name: config.project.name.clone(),
        cpp_standard: config.project.cpp_standard.clone(),
        compiler: config.build.compiler,
        profile,
        target_dir: "target".to_string(),
        sources,
        dependencies: resolved_dependencies,
    };

    let backend: Box<dyn BuildBackend> = match config.build.backend {
        BuildBackendKind::Cmake => Box::new(CmakeBackend),
        BuildBackendKind::Make => Box::new(MakeBackend),
    };

    let generated = backend.render(&plan)?;

    let manifest_path_string = backend.manifest_path(&plan);
    let manifest_path = Path::new(&manifest_path_string);
    if let Some(parent) = manifest_path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(manifest_path, generated)?;

    let cache_key = cache::compute_build_fingerprint(&plan, backend.backend_id(), &lock)?;
    let cache_key_path = Path::new("target/.cook-cache").join(format!("build-{}.fingerprint", profile.as_str()));
    let artifacts = backend
        .artifact_candidates(&plan)
        .into_iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    if cache::cache_hit(&cache_key_path, &cache_key, &artifacts)? {
        println!("cook: build cache hit, artifact is up-to-date");
        if let Some(path) = first_existing_path(&artifacts) {
            return Ok(path);
        }
    }

    run_commands(backend.configure_steps(&plan))?;
    run_commands(backend.build_steps(&plan))?;
    cache::write_cache_key(&cache_key_path, &cache_key)?;

    if let Some(path) = first_existing_path(&artifacts) {
        return Ok(path);
    }

    bail!(
        "build finished but no artifact was found in target/{} for project '{}'",
        profile.as_str(),
        plan.project_name
    )
}

pub fn lock_project() -> Result<()> {
    let config = load_config()?;

    fs::create_dir_all(".cook/deps")?;

    let resolution = resolver::resolve_from_manifest(
        "cook.toml",
        ResolveOptions {
            workspace_root: PathBuf::from("."),
            offline: config.build.offline,
        },
    )?;

    let lock = CookLock::from_resolution(&resolution);
    lock.write(Path::new("cook.lock"))?;
    println!("cook: lockfile updated at cook.lock");
    Ok(())
}

pub fn clean_project() -> Result<()> {
    let target_dir = Path::new("target");
    if !target_dir.exists() {
        println!("cook: nothing to clean (target/ does not exist)");
        return Ok(());
    }

    fs::remove_dir_all(target_dir)
        .with_context(|| format!("failed to remove {}", target_dir.display()))?;
    println!("cook: removed build artifacts at target/");
    Ok(())
}

fn run_commands(steps: Vec<CommandSpec>) -> Result<()> {
    for step in steps {
        let status = Command::new(&step.program)
            .args(&step.args)
            .status()
            .with_context(|| format!("failed to run command: {} {}", step.program, step.args.join(" ")))?;

        if !status.success() {
            bail!("command failed: {} {}", step.program, step.args.join(" "));
        }
    }

    Ok(())
}

fn prepare_external_dependencies(
    dependencies: &mut [ResolvedDependency],
    profile: BuildProfile,
    compiler: BuildCompiler,
) -> Result<()> {
    for dep in dependencies {
        let Some(build_system) = dep.external_build_system else {
            continue;
        };

        println!(
            "cook: preparing external dependency '{}' using {}",
            dep.name,
            build_system.as_str()
        );

        let normalized_root = match build_system {
            ExternalBuildSystem::Cmake => {
                let install_dir = build_external_cmake_dependency(dep, profile, compiler)?;
                normalize_path(&install_dir)
            }
            ExternalBuildSystem::Make => {
                let root_dir = build_external_make_dependency(dep, profile)?;
                normalize_path(&root_dir)
            }
        };

        dep.root_dir = normalized_root;
        apply_external_export_defaults(dep, profile);
    }

    Ok(())
}

fn apply_external_export_defaults(dep: &mut ResolvedDependency, profile: BuildProfile) {
    let Some(build_system) = dep.external_build_system else {
        return;
    };

    let inferred_pkg_config = infer_pkg_config_link_metadata(dep);

    if dep.exports.include_dirs.is_empty() {
        push_unique(&mut dep.exports.include_dirs, format!("{}/include", dep.root_dir));
        if matches!(build_system, ExternalBuildSystem::Make) {
            push_unique(&mut dep.exports.include_dirs, dep.root_dir.clone());
        }
    }

    if dep.exports.lib_dirs.is_empty() {
        match build_system {
            ExternalBuildSystem::Cmake => {
                push_unique(&mut dep.exports.lib_dirs, format!("{}/lib", dep.root_dir));
                push_unique(&mut dep.exports.lib_dirs, format!("{}/lib64", dep.root_dir));
            }
            ExternalBuildSystem::Make => {
                push_unique(&mut dep.exports.lib_dirs, format!("{}/lib", dep.root_dir));
                push_unique(
                    &mut dep.exports.lib_dirs,
                    format!("{}/target/{}", dep.root_dir, profile.as_str()),
                );
                push_unique(&mut dep.exports.lib_dirs, dep.root_dir.clone());
            }
        }
    }

    if let Some(metadata) = &inferred_pkg_config {
        for dir in &metadata.lib_dirs {
            push_unique(&mut dep.exports.lib_dirs, dir.clone());
        }
    }

    if dep.exports.libs.is_empty() {
        dep.exports.libs.push(dep.name.clone());
    }

    if let Some(metadata) = inferred_pkg_config {
        for lib in metadata.libs {
            push_unique(&mut dep.exports.libs, lib);
        }
    }

    // Some static libraries (notably raylib on Windows/MinGW) may omit
    // required system libs in pkg-config output; keep link stable by adding
    // well-known platform runtime libs.
    if cfg!(windows) && dep.name.eq_ignore_ascii_case("raylib") {
        push_unique(&mut dep.exports.libs, "winmm".to_string());
        push_unique(&mut dep.exports.libs, "gdi32".to_string());
        push_unique(&mut dep.exports.libs, "opengl32".to_string());
    }
}

fn push_unique(target: &mut Vec<String>, value: String) {
    if !target.iter().any(|existing| existing == &value) {
        target.push(value);
    }
}

#[derive(Debug, Default, Clone)]
struct PkgConfigLinkMetadata {
    lib_dirs: Vec<String>,
    libs: Vec<String>,
}

fn infer_pkg_config_link_metadata(dep: &ResolvedDependency) -> Option<PkgConfigLinkMetadata> {
    let pkgconfig_dir = Path::new(&dep.root_dir).join("lib").join("pkgconfig");
    if !pkgconfig_dir.exists() {
        return None;
    }

    let preferred = pkgconfig_dir.join(format!("{}.pc", dep.name));
    let pc_path = if preferred.exists() {
        preferred
    } else {
        fs::read_dir(&pkgconfig_dir)
            .ok()?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .find(|path| {
                path.extension()
                    .map(|ext| ext.to_string_lossy().eq_ignore_ascii_case("pc"))
                    .unwrap_or(false)
            })?
    };

    let content = fs::read_to_string(&pc_path).ok()?;
    Some(parse_pkg_config_link_metadata(&content))
}

fn parse_pkg_config_link_metadata(content: &str) -> PkgConfigLinkMetadata {
    let mut metadata = PkgConfigLinkMetadata::default();
    let mut variables = HashMap::<String, String>::new();
    let mut libs_sections = Vec::<String>::new();

    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            if !key.is_empty()
                && !key.contains(' ')
                && !key.contains('\t')
                && !key.contains(':')
            {
                variables.insert(key.to_string(), value.trim().to_string());
                continue;
            }
        }

        if let Some(value) = line.strip_prefix("Libs:") {
            libs_sections.push(value.trim().to_string());
            continue;
        }

        if let Some(value) = line.strip_prefix("Libs.private:") {
            libs_sections.push(value.trim().to_string());
        }
    }

    for section in libs_sections {
        let expanded = expand_pkg_config_value(&section, &variables, 0);
        let mut tokens = expanded
            .split_whitespace()
            .map(|token| token.trim_matches('"').trim_matches('\'').to_string())
            .peekable();

        while let Some(token) = tokens.next() {
            if token.is_empty() {
                continue;
            }

            if token == "-L" {
                if let Some(dir) = tokens.next() {
                    let clean_dir = dir.trim_matches('"').trim_matches('\'');
                    if !clean_dir.is_empty() {
                        let normalized = normalize_path(Path::new(clean_dir));
                        push_unique(&mut metadata.lib_dirs, normalized);
                    }
                }
                continue;
            }

            if token == "-l" {
                if let Some(lib) = tokens.next() {
                    let clean_lib = lib.trim_matches('"').trim_matches('\'');
                    if !clean_lib.is_empty() {
                        push_unique(&mut metadata.libs, clean_lib.to_string());
                    }
                }
                continue;
            }

            if let Some(dir) = token.strip_prefix("-L") {
                let clean_dir = dir.trim_matches('"').trim_matches('\'');
                if !clean_dir.is_empty() {
                    let normalized = normalize_path(Path::new(clean_dir));
                    push_unique(&mut metadata.lib_dirs, normalized);
                }
                continue;
            }

            if let Some(lib) = token.strip_prefix("-l") {
                let clean_lib = lib.trim_matches('"').trim_matches('\'');
                if !clean_lib.is_empty() {
                    push_unique(&mut metadata.libs, clean_lib.to_string());
                }
                continue;
            }

            if token.starts_with('-') || token.contains('/') || token.contains('\\') {
                push_unique(&mut metadata.libs, token);
            }
        }
    }

    metadata
}

fn expand_pkg_config_value(value: &str, vars: &HashMap<String, String>, depth: usize) -> String {
    if depth >= 8 {
        return value.to_string();
    }

    let mut expanded = value.to_string();
    let mut changed = false;

    loop {
        let Some(start) = expanded.find("${") else {
            break;
        };
        let Some(end_rel) = expanded[start + 2..].find('}') else {
            break;
        };
        let end = start + 2 + end_rel;
        let key = &expanded[start + 2..end];
        let replacement = vars
            .get(key)
            .map(|candidate| expand_pkg_config_value(candidate, vars, depth + 1))
            .unwrap_or_default();
        expanded.replace_range(start..=end, &replacement);
        changed = true;
    }

    if changed {
        expand_pkg_config_value(&expanded, vars, depth + 1)
    } else {
        expanded
    }
}

fn build_external_cmake_dependency(
    dep: &ResolvedDependency,
    profile: BuildProfile,
    compiler: BuildCompiler,
) -> Result<PathBuf> {
    let dep_root = PathBuf::from(&dep.root_dir);
    let compiler_dir = match compiler {
        BuildCompiler::Gcc => "gcc",
        BuildCompiler::Msvc => "msvc",
    };
    let base = Path::new("target")
        .join(profile.as_str())
        .join("deps")
        .join(&dep.name)
        .join(compiler_dir);
    let build_dir = base.join("build");
    let install_dir = base.join("install");

    fs::create_dir_all(&build_dir)?;
    fs::create_dir_all(&install_dir)?;

    let build_type = match profile {
        BuildProfile::Debug => "Debug",
        BuildProfile::Release => "Release",
    };

    let mut configure_args = vec![
        "-S".to_string(),
        normalize_path(&dep_root),
        "-B".to_string(),
        normalize_path(&build_dir),
        format!("-DCMAKE_BUILD_TYPE={build_type}"),
        format!("-DCMAKE_INSTALL_PREFIX={}", normalize_path(&install_dir)),
    ];
    for (key, value) in cmake_defines_for_dependency(dep) {
        configure_args.push(format!("-D{key}={value}"));
    }
    configure_args.extend(cmake_toolchain_args(compiler));
    configure_args.extend(dep.build.configure_args.clone());

    let mut cmake_build_args = vec![
        "--build".to_string(),
        normalize_path(&build_dir),
        "--config".to_string(),
        build_type.to_string(),
    ];
    cmake_build_args.extend(dep.build.build_args.clone());

    let mut cmake_install_args = vec![
        "--install".to_string(),
        normalize_path(&build_dir),
        "--config".to_string(),
        build_type.to_string(),
    ];
    cmake_install_args.extend(dep.build.install_args.clone());

    run_commands(vec![
        CommandSpec {
            program: "cmake".to_string(),
            args: configure_args,
        },
        CommandSpec {
            program: "cmake".to_string(),
            args: cmake_build_args,
        },
        CommandSpec {
            program: "cmake".to_string(),
            args: cmake_install_args,
        },
    ])?;

    Ok(install_dir)
}

fn build_external_make_dependency(dep: &ResolvedDependency, profile: BuildProfile) -> Result<PathBuf> {
    let dep_root = PathBuf::from(&dep.root_dir);

    let mut command = Command::new("make");
    command.arg("-C").arg(&dep_root);
    if matches!(profile, BuildProfile::Release) {
        command.env("CFLAGS", "-O3 -DNDEBUG");
        command.env("CXXFLAGS", "-O3 -DNDEBUG");
    } else {
        command.env("CFLAGS", "-O0 -g");
        command.env("CXXFLAGS", "-O0 -g");
    }

    let status = command
        .status()
        .with_context(|| format!("failed to run make for dependency {}", dep.name))?;
    if !status.success() {
        bail!(
            "make build failed for dependency '{}' at {}",
            dep.name,
            dep_root.display()
        );
    }

    Ok(dep_root)
}

fn resolve_remote_head_rev(url: &str) -> Result<String> {
    let output = Command::new("git")
        .args(["ls-remote", url, "HEAD"])
        .output()
        .with_context(|| format!("failed to run git ls-remote for '{url}'"))?;

    if !output.status.success() {
        bail!("git ls-remote failed for '{}': status {}", url, output.status);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout
        .lines()
        .find(|candidate| !candidate.trim().is_empty())
        .context("git ls-remote returned empty output")?;

    let rev = line
        .split_whitespace()
        .next()
        .context("could not parse revision from git ls-remote output")?;

    if rev.len() < 8 {
        bail!("resolved revision is invalid: {rev}");
    }

    Ok(rev.to_string())
}

fn enforce_lock_policy(config: &CookConfig, lock: &CookLock, lock_path: &Path) -> Result<()> {
    let ci_env = env::var("CI").unwrap_or_default().to_lowercase();
    let is_ci = matches!(ci_env.as_str(), "1" | "true" | "yes");

    let locked_env = env::var("COOK_LOCKED").unwrap_or_default().to_lowercase();
    let strict_locked_mode = matches!(locked_env.as_str(), "1" | "true" | "yes");

    let require_matching_lock = strict_locked_mode || (is_ci && config.build.strict_lock_in_ci);

    if require_matching_lock {
        return lock.ensure_matches_file(lock_path);
    }

    if config.resolver.write_lockfile {
        lock.write(lock_path)?;
    }

    Ok(())
}

pub fn new_project(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        bail!("project name cannot be empty");
    }

    let root = Path::new(name);
    if root.exists() {
        let mut entries = fs::read_dir(root)?;
        if entries.next().is_some() {
            bail!("target directory '{}' already exists and is not empty", name);
        }
    }

    fs::create_dir_all(root.join("src"))?;

    let cook_toml = format!(
        r#"[project]
name = "{name}"
cpp_standard = "17"

[build]
backend = "cmake"
compiler = "gcc"
strict_lock_in_ci = true
offline = false

[resolver]
write_lockfile = true

[project.abi]
# compiler = "clang"
# compiler_version = "18"
# arch = "x86_64"

[dependencies]
"#
    );

    let main_cpp = r#"#include <iostream>

int main() {
    std::cout << "Hello from Cook!" << std::endl;
    return 0;
}
"#;

    fs::write(root.join("cook.toml"), cook_toml)?;
    fs::write(root.join("src/main.cpp"), main_cpp)?;

    Ok(())
}

fn load_config() -> Result<CookConfig> {
    let config_str = fs::read_to_string("cook.toml")?;
    let config: CookConfig = toml::from_str(&config_str)?;
    Ok(config)
}

fn first_existing_path(paths: &[PathBuf]) -> Option<PathBuf> {
    paths.iter().find(|path| path.exists()).cloned()
}

fn normalize_path(path: &Path) -> String {
    strip_windows_verbatim_prefix(&path.to_string_lossy()).replace('\\', "/")
}

fn strip_windows_verbatim_prefix(input: &str) -> String {
    if let Some(rest) = input.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{}", rest);
    }

    if let Some(rest) = input.strip_prefix(r"\\?\") {
        return rest.to_string();
    }

    input.to_string()
}

fn cmake_toolchain_args(compiler: BuildCompiler) -> Vec<String> {
    match compiler {
        BuildCompiler::Gcc => {
            let mut args = Vec::new();
            if cfg!(windows) {
                args.push("-G".to_string());
                args.push("MinGW Makefiles".to_string());
            }
            args.push("-DCMAKE_C_COMPILER=gcc".to_string());
            args.push("-DCMAKE_CXX_COMPILER=g++".to_string());
            args
        }
        BuildCompiler::Msvc => {
            if cfg!(windows) {
                vec![
                    "-G".to_string(),
                    "Visual Studio 17 2022".to_string(),
                ]
            } else {
                Vec::new()
            }
        }
    }
}

fn profile_from_release(release: bool) -> BuildProfile {
    if release {
        BuildProfile::Release
    } else {
        BuildProfile::Debug
    }
}

fn cmake_defines_for_dependency(dep: &ResolvedDependency) -> BTreeMap<String, String> {
    let mut defines = dep.build.cmake.defines.clone();

    for exclude in &dep.build.exclude {
        let key = exclude.trim().to_ascii_lowercase();
        if key.as_str() == "examples" {
            defines
                .entry("BUILD_EXAMPLES".to_string())
                .or_insert_with(|| "OFF".to_string());
        }
    }

    defines
}

fn materialize_archive_dependency(name: &str, source_url: &str) -> Result<String> {
    if !looks_like_archive_url(source_url) {
        bail!(
            "dependency source '{}' is not a direct '.git' URL and does not look like a downloadable '.zip' URL",
            source_url
        );
    }

    let bytes = download_archive_bytes(source_url)?;
    let destination = Path::new(".cook").join("deps").join(name);
    if destination.exists() {
        fs::remove_dir_all(&destination)
            .with_context(|| format!("failed to clean existing dependency directory {}", destination.display()))?;
    }
    fs::create_dir_all(&destination)
        .with_context(|| format!("failed to create dependency directory {}", destination.display()))?;

    extract_zip_archive(&bytes, &destination)
        .with_context(|| format!("failed to extract archive '{}'", source_url))?;

    Ok(normalize_path(&destination))
}

fn download_archive_bytes(source_url: &str) -> Result<Vec<u8>> {
    let client = Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .context("failed to initialize HTTP client for archive download")?;

    let response = client
        .get(source_url)
        .header("User-Agent", "cook/0.1")
        .send()
        .with_context(|| format!("failed to download dependency archive {}", source_url))?;

    if response.status() == StatusCode::NOT_FOUND {
        bail!("dependency archive was not found: {}", source_url);
    }

    if !response.status().is_success() {
        bail!(
            "failed to download dependency archive {}: HTTP {}",
            source_url,
            response.status()
        );
    }

    response
        .bytes()
        .map(|bytes| bytes.to_vec())
        .with_context(|| format!("failed to read dependency archive body {}", source_url))
}

fn extract_zip_archive(bytes: &[u8], destination: &Path) -> Result<()> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))
        .context("invalid zip archive format")?;

    let strip_components = detect_common_top_level_component(&mut archive)?;

    for idx in 0..archive.len() {
        let mut entry = archive
            .by_index(idx)
            .with_context(|| format!("failed to access archive entry {idx}"))?;

        let Some(enclosed_name) = entry.enclosed_name() else {
            continue;
        };

        let relative_path = enclosed_name
            .components()
            .skip(strip_components)
            .fold(PathBuf::new(), |mut acc, component| {
                acc.push(component.as_os_str());
                acc
            });

        if relative_path.as_os_str().is_empty() {
            continue;
        }

        let output_path = destination.join(relative_path);

        if entry.is_dir() {
            fs::create_dir_all(&output_path)
                .with_context(|| format!("failed to create directory {}", output_path.display()))?;
            continue;
        }

        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create directory {}", parent.display()))?;
        }

        let mut output_file = fs::File::create(&output_path)
            .with_context(|| format!("failed to create file {}", output_path.display()))?;
        copy(&mut entry, &mut output_file)
            .with_context(|| format!("failed to write file {}", output_path.display()))?;
    }

    Ok(())
}

fn detect_common_top_level_component(archive: &mut ZipArchive<Cursor<&[u8]>>) -> Result<usize> {
    let mut top_level: Option<String> = None;
    let mut consistent = true;
    let mut has_nested_entries = false;

    for idx in 0..archive.len() {
        let entry = archive
            .by_index(idx)
            .with_context(|| format!("failed to inspect archive entry {idx}"))?;

        let Some(enclosed_name) = entry.enclosed_name() else {
            continue;
        };

        let mut components = enclosed_name.components();
        let Some(first) = components.next() else {
            continue;
        };

        let first_name = first.as_os_str().to_string_lossy().to_string();
        if top_level.is_none() {
            top_level = Some(first_name.clone());
        } else if top_level.as_deref() != Some(first_name.as_str()) {
            consistent = false;
        }

        if components.next().is_some() {
            has_nested_entries = true;
        }
    }

    if consistent && has_nested_entries { Ok(1) } else { Ok(0) }
}

fn is_direct_git_repo_url(source_url: &str) -> bool {
    source_url.trim().to_ascii_lowercase().ends_with(".git")
}

fn looks_like_archive_url(source_url: &str) -> bool {
    let lower = source_url.trim().to_ascii_lowercase();
    lower.ends_with(".zip")
    || lower.contains(".zip?")
}