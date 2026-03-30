use crate::config::DependencyVisibility;
use crate::resolver::{ExternalBuildSystem, PackageSource, ResolutionResult};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CookLock {
    pub version: u32,
    pub packages: Vec<LockedPackage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockedPackage {
    pub name: String,
    pub root_dir: String,
    pub source: LockedSource,
    pub abi_fingerprint: String,
    pub external_build_system: Option<LockedExternalBuildSystem>,
    pub external_manifest_path: Option<String>,
    pub external_manifest_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub external_include_dirs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub external_lib_dirs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub external_libs: Vec<String>,
    pub dependencies: Vec<LockedDependency>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LockedExternalBuildSystem {
    Cmake,
    Make,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockedDependency {
    pub name: String,
    pub visibility: DependencyVisibility,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum LockedSource {
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

impl CookLock {
    pub fn from_resolution(resolution: &ResolutionResult) -> Self {
        let mut packages = resolution
            .packages
            .iter()
            .map(|pkg| {
                let mut deps = pkg
                    .dependencies
                    .iter()
                    .map(|dep| LockedDependency {
                        name: dep.name.clone(),
                        visibility: dep.visibility,
                    })
                    .collect::<Vec<_>>();
                deps.sort_by(|a, b| a.name.cmp(&b.name));

                LockedPackage {
                    name: pkg.name.clone(),
                    root_dir: pkg.root_dir.clone(),
                    source: source_from_package(&pkg.source),
                    abi_fingerprint: pkg.abi_fingerprint.clone(),
                    external_build_system: pkg.external_build_system.map(external_build_system_to_locked),
                    external_manifest_path: pkg.external_manifest_path.clone(),
                    external_manifest_hash: pkg.external_manifest_hash.clone(),
                    external_include_dirs: pkg.exports.include_dirs.clone(),
                    external_lib_dirs: pkg.exports.lib_dirs.clone(),
                    external_libs: pkg.exports.libs.clone(),
                    dependencies: deps,
                }
            })
            .collect::<Vec<_>>();

        packages.sort_by(|a, b| a.name.cmp(&b.name).then(a.root_dir.cmp(&b.root_dir)));

        Self {
            version: 1,
            packages,
        }
    }

    pub fn read(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        toml::from_str(&raw)
            .with_context(|| format!("failed to parse {}", path.display()))
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        let rendered = toml::to_string_pretty(self)
            .with_context(|| format!("failed to serialize lockfile {}", path.display()))?;
        fs::write(path, rendered)
            .with_context(|| format!("failed to write {}", path.display()))
    }

    pub fn ensure_matches_file(&self, path: &Path) -> Result<()> {
        if !path.exists() {
            bail!(
                "lockfile is required but missing: {}. Run 'cook lock' locally and commit the file",
                path.display()
            );
        }

        let existing = Self::read(path)?;
        if existing == *self {
            return Ok(());
        }

        bail!(
            "lockfile drift detected in {}. Regenerate with 'cook lock' and commit the updated lockfile",
            path.display()
        )
    }
}

fn external_build_system_to_locked(system: ExternalBuildSystem) -> LockedExternalBuildSystem {
    match system {
        ExternalBuildSystem::Cmake => LockedExternalBuildSystem::Cmake,
        ExternalBuildSystem::Make => LockedExternalBuildSystem::Make,
    }
}

fn source_from_package(source: &PackageSource) -> LockedSource {
    match source {
        PackageSource::Root => LockedSource::Root,
        PackageSource::Path { manifest } => LockedSource::Path {
            manifest: manifest.clone(),
        },
        PackageSource::Git {
            url,
            requested_rev,
            resolved_rev,
        } => LockedSource::Git {
            url: url.clone(),
            requested_rev: requested_rev.clone(),
            resolved_rev: resolved_rev.clone(),
        },
    }
}
