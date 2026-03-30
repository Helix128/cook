use crate::config::BuildCompiler;
use crate::resolver::ResolvedDependency;
use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildProfile {
    Debug,
    Release,
}

impl BuildProfile {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Release => "release",
        }
    }

    pub fn cmake_config_name(self) -> &'static str {
        match self {
            Self::Debug => "Debug",
            Self::Release => "Release",
        }
    }
}

#[derive(Debug, Clone)]
pub struct BuildPlan {
    pub project_name: String,
    pub cpp_standard: String,
    pub compiler: BuildCompiler,
    pub profile: BuildProfile,
    pub target_dir: String,
    pub sources: Vec<String>,
    pub dependencies: Vec<ResolvedDependency>,
}

impl BuildPlan {
    pub fn profile_target_dir(&self) -> PathBuf {
        Path::new(&self.target_dir).join(self.profile.as_str())
    }

    pub fn backend_build_dir(&self) -> PathBuf {
        self.profile_target_dir()
            .join(format!("cook-build-{}", compiler_id(self.compiler)))
    }
}

#[derive(Debug, Clone)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
}

pub trait BuildBackend {
    fn backend_id(&self) -> &'static str;
    fn manifest_path(&self, plan: &BuildPlan) -> String;
    fn render(&self, plan: &BuildPlan) -> Result<String>;
    fn configure_steps(&self, plan: &BuildPlan) -> Vec<CommandSpec>;
    fn build_steps(&self, plan: &BuildPlan) -> Vec<CommandSpec>;
    fn artifact_candidates(&self, plan: &BuildPlan) -> Vec<String>;
}

pub struct CmakeBackend;

impl BuildBackend for CmakeBackend {
    fn backend_id(&self) -> &'static str {
        "cmake"
    }

    fn manifest_path(&self, _plan: &BuildPlan) -> String {
        "CMakeLists.txt".to_string()
    }

    fn render(&self, plan: &BuildPlan) -> Result<String> {
        if plan.sources.is_empty() {
            bail!("no source files were discovered under src/");
        }

        let source_block = plan
            .sources
            .iter()
            .map(|src| format!("    {src}"))
            .collect::<Vec<_>>()
            .join("\n");

        let add_subdirectory = plan
            .dependencies
            .iter()
            .filter(|dep| dep.external_build_system.is_none())
            .map(|dep| {
                format!(
                    "add_subdirectory(\"{}\" \"${{CMAKE_BINARY_DIR}}/deps/{}\")",
                    dep.root_dir, dep.name
                )
            })
            .collect::<Vec<_>>()
            .join("\n");

        let external_include_dirs = plan
            .dependencies
            .iter()
            .filter(|dep| dep.external_build_system.is_some())
            .flat_map(|dep| {
                dep.exports.include_dirs.iter().map(|dir| {
                    format!(
                        "target_include_directories({} PRIVATE \"{}\")",
                        plan.project_name, dir
                    )
                })
            })
            .collect::<Vec<_>>()
            .join("\n");

        let external_link_dirs = plan
            .dependencies
            .iter()
            .filter(|dep| dep.external_build_system.is_some())
            .flat_map(|dep| {
                dep.exports.lib_dirs.iter().map(|dir| {
                    format!(
                        "target_link_directories({} PRIVATE \"{}\")",
                        plan.project_name, dir
                    )
                })
            })
            .collect::<Vec<_>>()
            .join("\n");

        let link_libraries = plan
            .dependencies
            .iter()
            .flat_map(|dep| {
                if dep.external_build_system.is_some() {
                    dep.exports
                        .libs
                        .iter()
                        .map(|lib| cmake_link_token(lib))
                        .collect::<Vec<_>>()
                } else {
                    vec![dep.name.clone()]
                }
            })
            .collect::<Vec<_>>()
            .join(" ");

        let maybe_link = if link_libraries.is_empty() {
            String::new()
        } else {
            format!(
                "target_link_libraries({} PRIVATE {})",
                plan.project_name, link_libraries
            )
        };

        let profile_output_dir = normalize_path(&absolute_path(&plan.profile_target_dir()));

        Ok(format!(
            r#"cmake_minimum_required(VERSION 3.15)
project({} C CXX)

set(CMAKE_C_STANDARD 11)
set(CMAKE_CXX_STANDARD {})
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)
set(CMAKE_RUNTIME_OUTPUT_DIRECTORY "{}")
set(CMAKE_RUNTIME_OUTPUT_DIRECTORY_DEBUG "{}")
set(CMAKE_RUNTIME_OUTPUT_DIRECTORY_RELEASE "{}")
set(CMAKE_LIBRARY_OUTPUT_DIRECTORY "{}")
set(CMAKE_ARCHIVE_OUTPUT_DIRECTORY "{}")

{}

add_executable({}
{}
)

{}
{}

{}
"#,
            plan.project_name,
            plan.cpp_standard,
            profile_output_dir,
            profile_output_dir,
            profile_output_dir,
            profile_output_dir,
            profile_output_dir,
            add_subdirectory,
            plan.project_name,
            source_block,
            external_include_dirs,
            external_link_dirs,
            maybe_link,
        ))
    }

    fn configure_steps(&self, plan: &BuildPlan) -> Vec<CommandSpec> {
        let build_dir = normalize_path(&plan.backend_build_dir());
        let build_type = plan.profile.cmake_config_name().to_string();
        let mut args = vec![
            "-B".to_string(),
            build_dir,
            "-S".to_string(),
            ".".to_string(),
            format!("-DCMAKE_BUILD_TYPE={build_type}"),
        ];
        args.extend(cmake_toolchain_args(plan.compiler));
        vec![CommandSpec {
            program: "cmake".to_string(),
            args,
        }]
    }

    fn build_steps(&self, plan: &BuildPlan) -> Vec<CommandSpec> {
        let build_dir = normalize_path(&plan.backend_build_dir());
        let config = plan.profile.cmake_config_name().to_string();
        vec![CommandSpec {
            program: "cmake".to_string(),
            args: vec![
                "--build".to_string(),
                build_dir,
                "--config".to_string(),
                config,
            ],
        }]
    }

    fn artifact_candidates(&self, plan: &BuildPlan) -> Vec<String> {
        let profile_dir = normalize_path(&plan.profile_target_dir());
        let config_name = plan.profile.cmake_config_name();
        vec![
            format!("{profile_dir}/{}", plan.project_name),
            format!("{profile_dir}/{}.exe", plan.project_name),
            format!("{profile_dir}/{config_name}/{}", plan.project_name),
            format!("{profile_dir}/{config_name}/{}.exe", plan.project_name),
        ]
    }
}

pub struct MakeBackend;

impl BuildBackend for MakeBackend {
    fn backend_id(&self) -> &'static str {
        "make"
    }

    fn manifest_path(&self, plan: &BuildPlan) -> String {
        normalize_path(&plan.profile_target_dir().join("Makefile"))
    }

    fn render(&self, plan: &BuildPlan) -> Result<String> {
        if plan.sources.is_empty() {
            bail!("no source files were discovered under src/");
        }

        let mut object_files = Vec::new();
        let mut compile_rules = Vec::new();

        let profile_dir = normalize_path(&plan.profile_target_dir());
        for (idx, src) in plan.sources.iter().enumerate() {
            let object = format!("{profile_dir}/obj/{idx}.o");
            object_files.push(object.clone());

            let source_lower = src.to_ascii_lowercase();
            let compiler = if source_lower.ends_with(".c") {
                "$(CC)"
            } else {
                "$(CXX)"
            };
            let flags = if source_lower.ends_with(".c") {
                "$(CFLAGS)"
            } else {
                "$(CXXFLAGS)"
            };

            compile_rules.push(format!(
                "{object}: {src}\n\t@$(call MKDIR_P,$(dir $@))\n\t{compiler} {flags} $(INCLUDES) -c $< -o $@"
            ));
        }

        let include_flags = plan
            .dependencies
            .iter()
            .flat_map(include_flags_for_dependency)
            .collect::<Vec<_>>()
            .join(" ");

        let link_flags = plan
            .dependencies
            .iter()
            .flat_map(|dep| link_flags_for_dependency(dep, plan.profile))
            .collect::<Vec<_>>()
            .join(" ");

        let (c_flags, cxx_flags) = match plan.profile {
            BuildProfile::Debug => (
                "-O0 -g -Wall".to_string(),
                format!("-O0 -g -Wall -std=c++{}", plan.cpp_standard),
            ),
            BuildProfile::Release => (
                "-O3 -DNDEBUG -Wall".to_string(),
                format!("-O3 -DNDEBUG -Wall -std=c++{}", plan.cpp_standard),
            ),
        };

        Ok(format!(
            r#"CC ?= gcc
CXX ?= g++
CFLAGS ?= {}
CXXFLAGS ?= {}
INCLUDES := {}
DEP_LINKS := {}
OUTPUT := {}/{}
OBJECTS := {}

ifeq ($(OS),Windows_NT)
MKDIR_P = if not exist "$(subst /,\,$(patsubst %/,%,$(1)))" mkdir "$(subst /,\,$(patsubst %/,%,$(1)))"
RM_RF = if exist "$(subst /,\,$(patsubst %/,%,$(1)))" rmdir /S /Q "$(subst /,\,$(patsubst %/,%,$(1)))"
else
MKDIR_P = mkdir -p "$(1)"
RM_RF = rm -rf "$(1)"
endif

all: $(OUTPUT)

$(OUTPUT): $(OBJECTS)
	@$(call MKDIR_P,$(dir $@))
	$(CXX) $(OBJECTS) $(DEP_LINKS) -o $@

{}

clean:
	@$(call RM_RF,{})
"#,
            c_flags,
            cxx_flags,
            include_flags,
            link_flags,
            profile_dir,
            plan.project_name,
            object_files.join(" "),
            compile_rules.join("\n\n"),
            profile_dir,
        ))
    }

    fn configure_steps(&self, _plan: &BuildPlan) -> Vec<CommandSpec> {
        Vec::new()
    }

    fn build_steps(&self, plan: &BuildPlan) -> Vec<CommandSpec> {
        vec![CommandSpec {
            program: "make".to_string(),
            args: vec![
                "-f".to_string(),
                normalize_path(&plan.profile_target_dir().join("Makefile")),
                "all".to_string(),
            ],
        }]
    }

    fn artifact_candidates(&self, plan: &BuildPlan) -> Vec<String> {
        let profile_dir = normalize_path(&plan.profile_target_dir());
        vec![
            format!("{profile_dir}/{}", plan.project_name),
            format!("{profile_dir}/{}.exe", plan.project_name),
        ]
    }
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

fn absolute_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }

    match std::env::current_dir() {
        Ok(cwd) => cwd.join(path),
        Err(_) => path.to_path_buf(),
    }
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

fn compiler_id(compiler: BuildCompiler) -> &'static str {
    match compiler {
        BuildCompiler::Gcc => "gcc",
        BuildCompiler::Msvc => "msvc",
    }
}

fn include_flags_for_dependency(dep: &ResolvedDependency) -> Vec<String> {
    if dep.external_build_system.is_some() {
        dep.exports
            .include_dirs
            .iter()
            .map(|dir| format!("-I{dir}"))
            .collect()
    } else {
        vec![format!("-I{}/include", dep.root_dir)]
    }
}

fn link_flags_for_dependency(dep: &ResolvedDependency, profile: BuildProfile) -> Vec<String> {
    let mut flags = Vec::new();

    if dep.external_build_system.is_some() {
        flags.extend(dep.exports.lib_dirs.iter().map(|dir| format!("-L{dir}")));
        flags.extend(dep.exports.libs.iter().map(|lib| make_link_token(lib)));
        return flags;
    }

    flags.push(format!("-L{}/target/{}", dep.root_dir, profile.as_str()));
    flags.push(format!("-l{}", dep.name));
    flags
}

fn cmake_link_token(lib: &str) -> String {
    if lib.contains('/') || lib.contains('\\') {
        format!("\"{lib}\"")
    } else {
        lib.to_string()
    }
}

fn make_link_token(lib: &str) -> String {
    let lower = lib.to_ascii_lowercase();
    if lib.starts_with("-") {
        return lib.to_string();
    }

    if lib.contains('/')
        || lib.contains('\\')
        || lib.starts_with('.')
        || lower.ends_with(".a")
        || lower.ends_with(".so")
        || lower.ends_with(".dylib")
        || lower.ends_with(".lib")
    {
        return lib.to_string();
    }

    format!("-l{lib}")
}
