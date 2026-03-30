use crate::config::{
    AbiProfile, CookConfig, DependencyBuildControls, DependencyBuildSystem,
    DependencyExportConfig, DependencySource, DependencySpec, DependencyVisibility,
};
use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalBuildSystem {
    Cmake,
    Make,
}

impl ExternalBuildSystem {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cmake => "cmake",
            Self::Make => "make",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResolveOptions {
    pub workspace_root: PathBuf,
    pub offline: bool,
}

impl Default for ResolveOptions {
    fn default() -> Self {
        Self {
            workspace_root: PathBuf::from("."),
            offline: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageSource {
    Root,
    Path {
        manifest: String,
    },
    Git {
        url: String,
        requested_rev: String,
        resolved_rev: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedDependency {
    pub name: String,
    pub root_dir: String,
    pub visibility: DependencyVisibility,
    pub abi_fingerprint: String,
    pub external_build_system: Option<ExternalBuildSystem>,
    pub external_manifest_path: Option<String>,
    pub external_manifest_hash: Option<String>,
    pub exports: ResolvedExports,
    pub build: DependencyBuildControls,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedExports {
    pub include_dirs: Vec<String>,
    pub lib_dirs: Vec<String>,
    pub libs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedEdge {
    pub name: String,
    pub visibility: DependencyVisibility,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPackage {
    pub name: String,
    pub root_dir: String,
    pub source: PackageSource,
    pub dependencies: Vec<ResolvedEdge>,
    pub abi_fingerprint: String,
    pub external_build_system: Option<ExternalBuildSystem>,
    pub external_manifest_path: Option<String>,
    pub external_manifest_hash: Option<String>,
    pub exports: ResolvedExports,
    pub build: DependencyBuildControls,
}

#[derive(Debug, Clone)]
pub struct ResolutionResult {
    pub packages: Vec<ResolvedPackage>,
    pub direct_dependencies: Vec<ResolvedDependency>,
    pub build_order: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VisitState {
    Visiting,
    Visited,
}

#[derive(Debug, Clone)]
struct InternalEdge {
    target_idx: usize,
    visibility: DependencyVisibility,
}

#[derive(Debug, Clone)]
struct InternalNode {
    name: String,
    root_dir: PathBuf,
    source: PackageSource,
    abi: AbiProfile,
    abi_fingerprint: String,
    external_build_system: Option<ExternalBuildSystem>,
    external_manifest_path: Option<PathBuf>,
    external_manifest_hash: Option<String>,
    exports: ResolvedExports,
    build: DependencyBuildControls,
    dependencies: Vec<InternalEdge>,
}

pub fn resolve_from_manifest(manifest_path: impl AsRef<Path>, options: ResolveOptions) -> Result<ResolutionResult> {
    let root_manifest = canonical_manifest_path(manifest_path.as_ref())?;

    let mut resolver = Resolver::new(options);
    let root_idx = resolver.visit(&root_manifest, PackageSource::Root)?;

    let root_node = &resolver.nodes[root_idx];
    let direct_dependencies = root_node
        .dependencies
        .iter()
        .map(|edge| {
            let dep = &resolver.nodes[edge.target_idx];
            ResolvedDependency {
                name: dep.name.clone(),
                root_dir: normalize_path(&dep.root_dir),
                visibility: edge.visibility,
                abi_fingerprint: dep.abi_fingerprint.clone(),
                external_build_system: dep.external_build_system,
                external_manifest_path: dep.external_manifest_path.as_ref().map(|path| normalize_path(path)),
                external_manifest_hash: dep.external_manifest_hash.clone(),
                exports: dep.exports.clone(),
                build: dep.build.clone(),
            }
        })
        .collect::<Vec<_>>();

    let packages = resolver
        .nodes
        .iter()
        .map(|node| ResolvedPackage {
            name: node.name.clone(),
            root_dir: normalize_path(&node.root_dir),
            source: node.source.clone(),
            dependencies: node
                .dependencies
                .iter()
                .map(|edge| ResolvedEdge {
                    name: resolver.nodes[edge.target_idx].name.clone(),
                    visibility: edge.visibility,
                })
                .collect(),
            abi_fingerprint: node.abi_fingerprint.clone(),
            external_build_system: node.external_build_system,
            external_manifest_path: node
                .external_manifest_path
                .as_ref()
                .map(|path| normalize_path(path)),
            external_manifest_hash: node.external_manifest_hash.clone(),
            exports: node.exports.clone(),
            build: node.build.clone(),
        })
        .collect::<Vec<_>>();

    let build_order = resolver
        .post_order
        .iter()
        .filter(|idx| **idx != root_idx)
        .map(|idx| resolver.nodes[*idx].name.clone())
        .collect::<Vec<_>>();

    Ok(ResolutionResult {
        packages,
        direct_dependencies,
        build_order,
    })
}

struct Resolver {
    options: ResolveOptions,
    states: HashMap<PathBuf, VisitState>,
    stack: Vec<PathBuf>,
    nodes: Vec<InternalNode>,
    index_by_manifest: HashMap<PathBuf, usize>,
    index_by_external_root: HashMap<PathBuf, usize>,
    post_order: Vec<usize>,
}

impl Resolver {
    fn new(options: ResolveOptions) -> Self {
        Self {
            options,
            states: HashMap::new(),
            stack: Vec::new(),
            nodes: Vec::new(),
            index_by_manifest: HashMap::new(),
            index_by_external_root: HashMap::new(),
            post_order: Vec::new(),
        }
    }

    fn visit(&mut self, manifest_path: &Path, source: PackageSource) -> Result<usize> {
        if let Some(state) = self.states.get(manifest_path) {
            return match state {
                VisitState::Visited => Ok(*self
                    .index_by_manifest
                    .get(manifest_path)
                    .context("missing index for visited dependency")?),
                VisitState::Visiting => {
                    let cycle = self.render_cycle(manifest_path);
                    bail!("dependency cycle detected: {cycle}")
                }
            };
        }

        self.states
            .insert(manifest_path.to_path_buf(), VisitState::Visiting);
        self.stack.push(manifest_path.to_path_buf());

        let cfg = load_manifest(manifest_path)?;
        let root_dir = manifest_path
            .parent()
            .context("manifest path has no parent directory")?
            .to_path_buf();
        let abi_fingerprint = build_abi_fingerprint(&cfg.project.cpp_standard, &cfg.project.abi);

        let current_idx = self.nodes.len();
        self.nodes.push(InternalNode {
            name: cfg.project.name.clone(),
            root_dir: root_dir.clone(),
            source,
            abi: cfg.project.abi.clone(),
            abi_fingerprint,
            external_build_system: None,
            external_manifest_path: None,
            external_manifest_hash: None,
            exports: ResolvedExports::default(),
            build: DependencyBuildControls::default(),
            dependencies: Vec::new(),
        });
        self.index_by_manifest
            .insert(manifest_path.to_path_buf(), current_idx);

        for (alias, spec) in &cfg.dependencies {
            let descriptor = spec.descriptor();
            let dep_resolution = self.resolve_dependency_manifest(&root_dir, alias, spec)?;
            let dep_idx = match dep_resolution.kind {
                ResolvedManifestKind::Cook { manifest, source } => self.visit(&manifest, source)?,
                ResolvedManifestKind::External {
                    root_dir,
                    source,
                    build_system,
                    manifest_path,
                    manifest_hash,
                    name,
                    exports,
                    build,
                } => self.intern_external_node(
                    ExternalNodeData {
                        root_dir,
                        source,
                        build_system,
                        manifest_path,
                        manifest_hash,
                        name,
                        exports,
                        build: *build,
                    },
                    &cfg.project.cpp_standard,
                )?,
            };

            let child_node = &self.nodes[dep_idx];
            validate_abi_compatibility(
                &cfg.project.name,
                &cfg.project.abi,
                &child_node.name,
                &child_node.abi,
                "parent-child",
            )?;
            validate_abi_compatibility(
                alias,
                &descriptor.abi,
                &child_node.name,
                &child_node.abi,
                "dependency-constraint",
            )?;

            if !self.nodes[current_idx]
                .dependencies
                .iter()
                .any(|edge| edge.target_idx == dep_idx)
            {
                self.nodes[current_idx].dependencies.push(InternalEdge {
                    target_idx: dep_idx,
                    visibility: descriptor.visibility,
                });
            }
        }

        self.stack.pop();
        self.states
            .insert(manifest_path.to_path_buf(), VisitState::Visited);
        self.post_order.push(current_idx);

        Ok(current_idx)
    }

    fn intern_external_node(&mut self, data: ExternalNodeData, cpp_standard: &str) -> Result<usize> {
        let ExternalNodeData {
            root_dir,
            source,
            build_system,
            manifest_path,
            manifest_hash,
            name,
            exports,
            build,
        } = data;

        if let Some(idx) = self.index_by_external_root.get(&root_dir) {
            if self.nodes[*idx].exports != exports || self.nodes[*idx].build != build {
                bail!(
                    "dependency '{}' declares conflicting build metadata for external source {}",
                    name,
                    root_dir.display()
                );
            }
            return Ok(*idx);
        }

        let abi = AbiProfile::default();
        let abi_fingerprint = build_abi_fingerprint(cpp_standard, &abi);
        let idx = self.nodes.len();

        self.nodes.push(InternalNode {
            name,
            root_dir: root_dir.clone(),
            source,
            abi,
            abi_fingerprint,
            external_build_system: Some(build_system),
            external_manifest_path: Some(manifest_path),
            external_manifest_hash: Some(manifest_hash),
            exports,
            build,
            dependencies: Vec::new(),
        });

        self.index_by_external_root.insert(root_dir, idx);
        self.post_order.push(idx);

        Ok(idx)
    }

    fn resolve_dependency_manifest(
        &self,
        base_dir: &Path,
        name: &str,
        spec: &DependencySpec,
    ) -> Result<ResolvedManifest> {
        let descriptor = spec.descriptor();

        match descriptor.source {
            DependencySource::Path(path) => {
                let dep_root = canonical_directory_path(&base_dir.join(path))?;
                let cook_manifest = dep_root.join("cook.toml");
                if cook_manifest.exists() {
                    let manifest = canonical_manifest_path(&cook_manifest)?;
                    return Ok(ResolvedManifest {
                        kind: ResolvedManifestKind::Cook {
                            manifest: manifest.clone(),
                            source: PackageSource::Path {
                                manifest: normalize_path(&manifest),
                            },
                        },
                    });
                }

                let (build_system, manifest_path) =
                    detect_external_build_system(&dep_root, descriptor.build_system).with_context(|| {
                    format!(
                        "dependency '{}' has no cook.toml and no supported external build file (CMakeLists.txt or Makefile) in {}",
                        name,
                        dep_root.display()
                    )
                })?;
                let manifest_hash = hash_file_hex(&manifest_path)?;
                let exports = resolve_external_exports(&dep_root, &descriptor.export);

                Ok(ResolvedManifest {
                    kind: ResolvedManifestKind::External {
                        root_dir: dep_root.clone(),
                        source: PackageSource::Path {
                            manifest: normalize_path(&dep_root),
                        },
                        build_system,
                        manifest_path,
                        manifest_hash,
                        name: name.to_string(),
                        exports,
                        build: Box::new(descriptor.build.clone()),
                    },
                })
            }
            DependencySource::Git {
                url,
                rev,
                branch,
                tag,
            } => {
                if url.trim().is_empty() {
                    bail!("dependency '{}' defines git source with empty URL", name);
                }

                let requested_rev = rev.with_context(|| {
                    format!(
                        "dependency '{}' uses git source without 'rev'. Pin a commit SHA for reproducible builds",
                        name
                    )
                })?;

                let checkout_dir = self
                    .options
                    .workspace_root
                    .join(".cook")
                    .join("deps")
                    .join(name);

                let resolved_rev = materialize_git_dependency(
                    &checkout_dir,
                    &url,
                    &requested_rev,
                    branch.as_deref(),
                    tag.as_deref(),
                    self.options.offline,
                )?;

                let dep_root = canonical_directory_path(&checkout_dir)?;
                let cook_manifest = dep_root.join("cook.toml");
                if cook_manifest.exists() {
                    let manifest = canonical_manifest_path(&cook_manifest)?;
                    return Ok(ResolvedManifest {
                        kind: ResolvedManifestKind::Cook {
                            manifest,
                            source: PackageSource::Git {
                                url,
                                requested_rev,
                                resolved_rev,
                            },
                        },
                    });
                }

                let (build_system, manifest_path) =
                    detect_external_build_system(&dep_root, descriptor.build_system).with_context(|| {
                    format!(
                        "dependency '{}' from git '{}' has no cook.toml and no supported external build file (CMakeLists.txt or Makefile)",
                        name,
                        url
                    )
                })?;
                let manifest_hash = hash_file_hex(&manifest_path)?;
                let exports = resolve_external_exports(&dep_root, &descriptor.export);

                Ok(ResolvedManifest {
                    kind: ResolvedManifestKind::External {
                        root_dir: dep_root,
                        source: PackageSource::Git {
                            url,
                            requested_rev,
                            resolved_rev,
                        },
                        build_system,
                        manifest_path,
                        manifest_hash,
                        name: name.to_string(),
                        exports,
                        build: Box::new(descriptor.build.clone()),
                    },
                })
            }
        }
    }

    fn render_cycle(&self, back_edge: &Path) -> String {
        let mut labels = Vec::new();
        if let Some(pos) = self.stack.iter().position(|item| item == back_edge) {
            for path in &self.stack[pos..] {
                labels.push(label_for_manifest(path));
            }
            labels.push(label_for_manifest(back_edge));
            return labels.join(" -> ");
        }

        label_for_manifest(back_edge)
    }
}

#[derive(Debug, Clone)]
struct ExternalNodeData {
    root_dir: PathBuf,
    source: PackageSource,
    build_system: ExternalBuildSystem,
    manifest_path: PathBuf,
    manifest_hash: String,
    name: String,
    exports: ResolvedExports,
    build: DependencyBuildControls,
}

#[derive(Debug, Clone)]
struct ResolvedManifest {
    kind: ResolvedManifestKind,
}

#[derive(Debug, Clone)]
enum ResolvedManifestKind {
    Cook {
        manifest: PathBuf,
        source: PackageSource,
    },
    External {
        root_dir: PathBuf,
        source: PackageSource,
        build_system: ExternalBuildSystem,
        manifest_path: PathBuf,
        manifest_hash: String,
        name: String,
        exports: ResolvedExports,
        build: Box<DependencyBuildControls>,
    },
}

fn resolve_external_exports(root_dir: &Path, export: &DependencyExportConfig) -> ResolvedExports {
    ResolvedExports {
        include_dirs: export
            .include_dirs
            .iter()
            .map(|item| normalize_path(&resolve_export_path(root_dir, item)))
            .collect(),
        lib_dirs: export
            .lib_dirs
            .iter()
            .map(|item| normalize_path(&resolve_export_path(root_dir, item)))
            .collect(),
        libs: export
            .libs
            .iter()
            .map(|item| resolve_export_lib(root_dir, item))
            .collect(),
    }
}

fn resolve_export_path(root_dir: &Path, value: &str) -> PathBuf {
    let candidate = Path::new(value);
    if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        root_dir.join(candidate)
    }
}

fn resolve_export_lib(root_dir: &Path, value: &str) -> String {
    let candidate = Path::new(value);
    if candidate.is_absolute() {
        return normalize_path(candidate);
    }

    if value.starts_with(".") || value.contains('/') || value.contains('\\') {
        return normalize_path(&root_dir.join(candidate));
    }

    value.to_string()
}

fn materialize_git_dependency(
    checkout_dir: &Path,
    url: &str,
    requested_rev: &str,
    branch: Option<&str>,
    tag: Option<&str>,
    offline: bool,
) -> Result<String> {
    if !checkout_dir.exists() {
        if offline {
            bail!(
                "offline mode is enabled and dependency checkout is missing: {}",
                checkout_dir.display()
            );
        }

        if let Some(parent) = checkout_dir.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut clone_args = vec!["clone".to_string(), url.to_string()];
        if let Some(branch) = branch {
            clone_args.push("--branch".to_string());
            clone_args.push(branch.to_string());
        }
        clone_args.push(checkout_dir.to_string_lossy().to_string());

        run_git_with_retry(&clone_args, 2)?;
    } else if !offline {
        run_git_in_repo_with_retry(checkout_dir, &["fetch", "--all", "--tags"], 2)?;
    }

    if !checkout_dir.join(".git").exists() {
        bail!("dependency checkout is not a git repository: {}", checkout_dir.display());
    }

    run_git_in_repo(checkout_dir, &["checkout", "--force", requested_rev])?;

    let resolved = run_git_capture(checkout_dir, &["rev-parse", "HEAD"])?;
    if !resolved.starts_with(requested_rev) {
        bail!(
            "git checkout verification failed for {}: requested {}, got {}",
            checkout_dir.display(),
            requested_rev,
            resolved
        );
    }

    if let Some(tag) = tag {
        let tag_rev = run_git_capture(checkout_dir, &["rev-list", "-n", "1", tag])?;
        if tag_rev != resolved {
            bail!(
                "tag '{}' does not match resolved revision '{}' for {}",
                tag,
                resolved,
                checkout_dir.display()
            );
        }
    }

    Ok(resolved)
}

fn run_git_with_retry(args: &[String], retries: usize) -> Result<()> {
    let mut last_error = None;
    for _ in 0..retries {
        match Command::new("git").args(args).status() {
            Ok(status) if status.success() => return Ok(()),
            Ok(status) => {
                last_error = Some(anyhow::anyhow!("git {} failed with status {}", args.join(" "), status));
            }
            Err(err) => {
                last_error = Some(err.into());
            }
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("git command failed")))
}

fn run_git_in_repo_with_retry(repo: &Path, args: &[&str], retries: usize) -> Result<()> {
    let mut last_error = None;
    for _ in 0..retries {
        match Command::new("git").arg("-C").arg(repo).args(args).status() {
            Ok(status) if status.success() => return Ok(()),
            Ok(status) => {
                last_error = Some(anyhow::anyhow!(
                    "git -C {} {} failed with status {}",
                    repo.display(),
                    args.join(" "),
                    status
                ));
            }
            Err(err) => {
                last_error = Some(err.into());
            }
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("git command failed")))
}

fn run_git_in_repo(repo: &Path, args: &[&str]) -> Result<()> {
    let status = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .status()
        .with_context(|| format!("failed to run git -C {} {}", repo.display(), args.join(" ")))?;

    if status.success() {
        Ok(())
    } else {
        bail!(
            "git -C {} {} failed with status {}",
            repo.display(),
            args.join(" "),
            status
        )
    }
}

fn run_git_capture(repo: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .with_context(|| format!("failed to run git -C {} {}", repo.display(), args.join(" ")))?;

    if !output.status.success() {
        bail!(
            "git -C {} {} failed with status {}",
            repo.display(),
            args.join(" "),
            output.status
        );
    }

    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        bail!("git command produced empty output: git -C {} {}", repo.display(), args.join(" "));
    }

    Ok(text)
}

fn load_manifest(manifest_path: &Path) -> Result<CookConfig> {
    let raw = fs::read_to_string(manifest_path)
        .with_context(|| format!("failed to read {}", manifest_path.display()))?;

    toml::from_str(&raw)
        .with_context(|| format!("failed to parse {}", manifest_path.display()))
}

fn canonical_manifest_path(manifest_path: &Path) -> Result<PathBuf> {
    if !manifest_path.exists() {
        bail!("manifest not found: {}", manifest_path.display());
    }

    fs::canonicalize(manifest_path)
        .with_context(|| format!("failed to canonicalize {}", manifest_path.display()))
}

fn canonical_directory_path(directory: &Path) -> Result<PathBuf> {
    if !directory.exists() {
        bail!("dependency directory not found: {}", directory.display());
    }
    if !directory.is_dir() {
        bail!("dependency path is not a directory: {}", directory.display());
    }

    fs::canonicalize(directory)
        .with_context(|| format!("failed to canonicalize {}", directory.display()))
}

fn detect_external_build_system(
    root_dir: &Path,
    preference: DependencyBuildSystem,
) -> Option<(ExternalBuildSystem, PathBuf)> {
    let cmake_manifest = root_dir.join("CMakeLists.txt");
    let make_manifest = root_dir.join("Makefile");
    let make_manifest_lower = root_dir.join("makefile");

    match preference {
        DependencyBuildSystem::Cmake => {
            if cmake_manifest.exists() {
                Some((ExternalBuildSystem::Cmake, cmake_manifest))
            } else {
                None
            }
        }
        DependencyBuildSystem::Make => {
            if make_manifest.exists() {
                Some((ExternalBuildSystem::Make, make_manifest))
            } else if make_manifest_lower.exists() {
                Some((ExternalBuildSystem::Make, make_manifest_lower))
            } else {
                None
            }
        }
        DependencyBuildSystem::Auto => {
            if cmake_manifest.exists() {
                Some((ExternalBuildSystem::Cmake, cmake_manifest))
            } else if make_manifest.exists() {
                Some((ExternalBuildSystem::Make, make_manifest))
            } else if make_manifest_lower.exists() {
                Some((ExternalBuildSystem::Make, make_manifest_lower))
            } else {
                None
            }
        }
    }
}

fn hash_file_hex(path: &Path) -> Result<String> {
    let bytes = fs::read(path)
        .with_context(|| format!("failed to read external manifest {}", path.display()))?;

    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok(format!("{:x}", hasher.finalize()))
}

fn validate_abi_compatibility(
    parent_name: &str,
    parent: &AbiProfile,
    child_name: &str,
    child: &AbiProfile,
    context: &str,
) -> Result<()> {
    ensure_abi_field_match(parent_name, child_name, context, "compiler", &parent.compiler, &child.compiler)?;
    ensure_abi_field_match(
        parent_name,
        child_name,
        context,
        "compiler_version",
        &parent.compiler_version,
        &child.compiler_version,
    )?;
    ensure_abi_field_match(parent_name, child_name, context, "c_runtime", &parent.c_runtime, &child.c_runtime)?;
    ensure_abi_field_match(
        parent_name,
        child_name,
        context,
        "cxx_runtime",
        &parent.cxx_runtime,
        &child.cxx_runtime,
    )?;
    ensure_abi_field_match(parent_name, child_name, context, "exceptions", &parent.exceptions, &child.exceptions)?;
    ensure_abi_field_match(parent_name, child_name, context, "rtti", &parent.rtti, &child.rtti)?;
    ensure_abi_field_match(parent_name, child_name, context, "pic", &parent.pic, &child.pic)?;
    ensure_abi_field_match(parent_name, child_name, context, "visibility", &parent.visibility, &child.visibility)?;
    ensure_abi_field_match(parent_name, child_name, context, "arch", &parent.arch, &child.arch)?;
    Ok(())
}

fn ensure_abi_field_match<T: std::fmt::Debug + PartialEq>(
    parent_name: &str,
    child_name: &str,
    context: &str,
    field_name: &str,
    parent: &Option<T>,
    child: &Option<T>,
) -> Result<()> {
    if let (Some(left), Some(right)) = (parent, child)
        && left != right
    {
        bail!(
            "ABI incompatibility ({context}) between '{}' and '{}': field '{}' differs ({:?} != {:?})",
            parent_name,
            child_name,
            field_name,
            left,
            right
        );
    }

    Ok(())
}

fn build_abi_fingerprint(cpp_standard: &str, abi: &AbiProfile) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("cpp_standard={cpp_standard}\n"));
    hasher.update(format!("compiler={:?}\n", abi.compiler));
    hasher.update(format!("compiler_version={:?}\n", abi.compiler_version));
    hasher.update(format!("c_runtime={:?}\n", abi.c_runtime));
    hasher.update(format!("cxx_runtime={:?}\n", abi.cxx_runtime));
    hasher.update(format!("exceptions={:?}\n", abi.exceptions));
    hasher.update(format!("rtti={:?}\n", abi.rtti));
    hasher.update(format!("pic={:?}\n", abi.pic));
    hasher.update(format!("visibility={:?}\n", abi.visibility));
    hasher.update(format!("arch={:?}\n", abi.arch));
    format!("{:x}", hasher.finalize())
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

fn label_for_manifest(manifest: &Path) -> String {
    manifest
        .parent()
        .and_then(|parent| parent.file_name())
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| manifest.display().to_string())
}
