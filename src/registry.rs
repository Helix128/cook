use crate::config::{
	DependencyBuildControls, DependencyBuildSystem, DependencyDetail, DependencyExportConfig,
};
use anyhow::{Context, Result, bail};
use reqwest::blocking::Client;
use reqwest::StatusCode;
use serde::Deserialize;
use std::env;
use std::time::Duration;

const DEFAULT_COOKBOOK_RAW_BASE_URL: &str =
	"https://github.com/Helix128/cookbook/tree/main/packages";

#[derive(Debug, Clone)]
pub struct RegistryResolution {
	pub dependency_name: String,
	pub detail: DependencyDetail,
}

pub fn resolve_package_from_cookbook(name: &str) -> Result<RegistryResolution> {
	let normalized = normalize_package_name(name)?;
	let manifest_url = cookbook_manifest_url(&normalized);
	let manifest_raw = fetch_cookbook_manifest(&manifest_url, name)?;
	let manifest: CookbookPackageManifest = toml::from_str(&manifest_raw)
		.with_context(|| format!("failed to parse cookbook manifest {}", manifest_url))?;

	let git = manifest
		.source
		.git
		.clone()
		.or_else(|| manifest.source.url.clone())
		.or_else(|| manifest.source.archive.clone())
		.with_context(|| {
			format!(
				"package '{}' has no supported source in cookbook manifest {} (requires [source.git], [source.url], or [source.archive])",
				name,
				manifest_url
			)
		})?;

	let build_system = manifest
		.build
		.as_ref()
		.and_then(|build| build.system)
		.or(manifest.defaults.as_ref().and_then(|defaults| defaults.build_system));

	let export = manifest
		.defaults
		.as_ref()
		.map(|defaults| defaults.export.clone())
		.unwrap_or_default();

	let build = manifest
		.build
		.as_ref()
		.map(cookbook_build_to_controls)
		.unwrap_or_default();

	Ok(RegistryResolution {
		dependency_name: normalized,
		detail: DependencyDetail {
			git: Some(git),
			rev: manifest.source.rev.clone(),
			registry_name: Some(manifest.package.name.clone()),
			registry_version: manifest.package.version.clone(),
			build_system,
			export,
			build,
			..DependencyDetail::default()
		},
	})
}

fn cookbook_build_to_controls(build: &CookbookBuildSection) -> DependencyBuildControls {
	DependencyBuildControls {
		exclude: build.exclude.clone(),
		configure_args: build.configure_args.clone(),
		build_args: build.build_args.clone(),
		install_args: build.install_args.clone(),
		cmake: build.cmake.clone(),
	}
}

fn cookbook_manifest_url(package_name: &str) -> String {
	let base = env::var("COOKBOOK_RAW_BASE_URL")
		.unwrap_or_else(|_| DEFAULT_COOKBOOK_RAW_BASE_URL.to_string());
	let base = base.trim_end_matches('/');
	format!("{base}/{package_name}.toml?raw=true")
}

fn fetch_cookbook_manifest(url: &str, package_name: &str) -> Result<String> {
	let client = Client::builder()
		.timeout(Duration::from_secs(20))
		.build()
		.context("failed to initialize HTTP client for cookbook lookup")?;

	let response = client
		.get(url)
		.header("User-Agent", "cook/0.1")
		.send()
		.with_context(|| format!("failed to fetch cookbook manifest {}", url))?;

	if response.status() == StatusCode::NOT_FOUND {
		bail!(
			"package '{}' was not found in cookbook at {}",
			package_name,
			url
		);
	}

	if !response.status().is_success() {
		bail!(
			"failed to fetch cookbook manifest {}: HTTP {}",
			url,
			response.status()
		);
	}

	response
		.text()
		.with_context(|| format!("failed to read cookbook response body {}", url))
}

fn normalize_package_name(input: &str) -> Result<String> {
	let normalized = input.trim().to_ascii_lowercase();
	if normalized.is_empty() {
		bail!("package name cannot be empty");
	}

	if !normalized
		.chars()
		.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
	{
		bail!(
			"package name '{}' contains unsupported characters (allowed: a-z, 0-9, '-', '_' and '.')",
			input
		);
	}

	Ok(normalized)
}

#[derive(Debug, Clone, Deserialize)]
struct CookbookPackageManifest {
	package: CookbookPackageSection,
	source: CookbookSourceSection,
	#[serde(default)]
	defaults: Option<CookbookDefaultsSection>,
	#[serde(default)]
	build: Option<CookbookBuildSection>,
}

#[derive(Debug, Clone, Deserialize)]
struct CookbookPackageSection {
	name: String,
	#[serde(default)]
	version: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct CookbookSourceSection {
	#[serde(default)]
	git: Option<String>,
	#[serde(default)]
	url: Option<String>,
	#[serde(default)]
	rev: Option<String>,
	#[serde(default)]
	archive: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct CookbookDefaultsSection {
	#[serde(default)]
	build_system: Option<DependencyBuildSystem>,
	#[serde(default)]
	export: DependencyExportConfig,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct CookbookBuildSection {
	#[serde(default)]
	system: Option<DependencyBuildSystem>,
	#[serde(default)]
	exclude: Vec<String>,
	#[serde(default)]
	configure_args: Vec<String>,
	#[serde(default)]
	build_args: Vec<String>,
	#[serde(default)]
	install_args: Vec<String>,
	#[serde(default)]
	cmake: crate::config::DependencyCmakeBuildControls,
}
