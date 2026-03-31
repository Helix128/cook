use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CookConfig {
    pub project: Project,
    #[serde(default)]
    pub build: BuildConfig,
    #[serde(default)]
    pub resolver: ResolverConfig,
    #[serde(default)]
    pub dependencies: BTreeMap<String, DependencySpec>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Project {
    pub name: String,
    pub cpp_standard: String,
    #[serde(default)]
    pub abi: AbiProfile,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BuildConfig {
    #[serde(default)]
    pub backend: BuildBackend,
    #[serde(default)]
    pub compiler: BuildCompiler,
    #[serde(default = "default_true")]
    pub strict_lock_in_ci: bool,
    #[serde(default)]
    pub offline: bool,
    #[serde(default = "default_build_threads")]
    pub threads: usize,
    #[serde(default)]
    pub flags: BuildProfileFlags,
}

impl Default for BuildConfig {
    fn default() -> Self {
        Self {
            backend: BuildBackend::Cmake,
            compiler: BuildCompiler::Gcc,
            strict_lock_in_ci: true,
            offline: false,
            threads: default_build_threads(),
            flags: BuildProfileFlags::default(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct BuildProfileFlags {
    #[serde(default)]
    pub debug: BuildCompileParams,
    #[serde(default)]
    pub release: BuildCompileParams,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct BuildCompileParams {
    #[serde(default)]
    pub optimization: Option<OptimizationLevel>,
    #[serde(default)]
    pub fast_math: bool,
    #[serde(default)]
    pub c_flags: Vec<String>,
    #[serde(default)]
    pub cxx_flags: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OptimizationLevel {
    O0,
    O1,
    O2,
    O3,
    Os,
    Oz,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ResolverConfig {
    #[serde(default = "default_true")]
    pub write_lockfile: bool,
}

impl Default for ResolverConfig {
    fn default() -> Self {
        Self {
            write_lockfile: true,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default)]
#[serde(rename_all = "lowercase")]
pub enum BuildBackend {
    #[default]
    Cmake,
    Make,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum BuildCompiler {
    #[default]
    Gcc,
    Msvc,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum DependencySpec {
    Path(String),
    Detailed(Box<DependencyDetail>),
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct DependencyDetail {
    pub path: Option<String>,
    pub git: Option<String>,
    pub rev: Option<String>,
    pub branch: Option<String>,
    pub tag: Option<String>,
    pub source_sha256: Option<String>,
    pub strip_prefix: Option<String>,
    pub registry_name: Option<String>,
    pub registry_version: Option<String>,
    pub build_system: Option<DependencyBuildSystem>,
    #[serde(default)]
    pub export: DependencyExportConfig,
    #[serde(default)]
    pub build: DependencyBuildControls,
    #[serde(default)]
    pub public: bool,
    #[serde(default)]
    pub abi: AbiProfile,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct DependencyBuildControls {
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub configure_args: Vec<String>,
    #[serde(default)]
    pub build_args: Vec<String>,
    #[serde(default)]
    pub install_args: Vec<String>,
    #[serde(default)]
    pub cmake: DependencyCmakeBuildControls,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct DependencyCmakeBuildControls {
    #[serde(default)]
    pub defines: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct DependencyExportConfig {
    #[serde(default)]
    pub include_dirs: Vec<String>,
    #[serde(default)]
    pub lib_dirs: Vec<String>,
    #[serde(default)]
    pub libs: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DependencyBuildSystem {
    Auto,
    Cmake,
    Make,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct AbiProfile {
    pub compiler: Option<String>,
    pub compiler_version: Option<String>,
    pub c_runtime: Option<String>,
    pub cxx_runtime: Option<String>,
    pub exceptions: Option<bool>,
    pub rtti: Option<bool>,
    pub pic: Option<bool>,
    pub visibility: Option<String>,
    pub arch: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DependencyVisibility {
    Public,
    Private,
}

#[derive(Debug, Clone)]
pub struct DependencyDescriptor {
    pub source: DependencySource,
    pub visibility: DependencyVisibility,
    pub abi: AbiProfile,
    pub build_system: DependencyBuildSystem,
    pub export: DependencyExportConfig,
    pub build: DependencyBuildControls,
}

#[derive(Debug, Clone)]
pub enum DependencySource {
    Path(String),
    Git {
        url: String,
        rev: Option<String>,
        branch: Option<String>,
        tag: Option<String>,
    },
}

impl DependencySpec {
    pub fn descriptor(&self) -> DependencyDescriptor {
        match self {
            DependencySpec::Path(path) => DependencyDescriptor {
                source: DependencySource::Path(path.clone()),
                visibility: DependencyVisibility::Private,
                abi: AbiProfile::default(),
                build_system: DependencyBuildSystem::Auto,
                export: DependencyExportConfig::default(),
                build: DependencyBuildControls::default(),
            },
            DependencySpec::Detailed(detail) => {
                if let Some(path) = &detail.path {
                    return DependencyDescriptor {
                        source: DependencySource::Path(path.clone()),
                        visibility: visibility_from_bool(detail.public),
                        abi: detail.abi.clone(),
                        build_system: detail
                            .build_system
                            .unwrap_or(DependencyBuildSystem::Auto),
                        export: detail.export.clone(),
                        build: detail.build.clone(),
                    };
                }

                DependencyDescriptor {
                    source: DependencySource::Git {
                        url: detail.git.clone().unwrap_or_default(),
                        rev: detail.rev.clone(),
                        branch: detail.branch.clone(),
                        tag: detail.tag.clone(),
                    },
                    visibility: visibility_from_bool(detail.public),
                    abi: detail.abi.clone(),
                    build_system: detail
                        .build_system
                        .unwrap_or(DependencyBuildSystem::Auto),
                    export: detail.export.clone(),
                    build: detail.build.clone(),
                }
            }
        }
    }
}

fn visibility_from_bool(is_public: bool) -> DependencyVisibility {
    if is_public {
        DependencyVisibility::Public
    } else {
        DependencyVisibility::Private
    }
}

const fn default_true() -> bool {
    true
}

fn default_build_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}
