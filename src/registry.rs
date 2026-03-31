use crate::config::{
	DependencyBuildControls, DependencyBuildSystem, DependencyDetail, DependencyExportConfig,
};
use anyhow::{Context, Result, bail};
use reqwest::blocking::Client;
use reqwest::StatusCode;
use semver::{Version, VersionReq};
use serde::Deserialize;
use std::env;
use std::time::Duration;

const DEFAULT_COOKBOOK_RAW_BASE_URL: &str =
	"https://raw.githubusercontent.com/Helix128/cookbook/refs/heads/main/packages";

#[derive(Debug, Clone)]
pub struct RegistryResolution {
	pub dependency_name: String,
	pub detail: DependencyDetail,
}

pub fn resolve_package_from_cookbook(name: &str, version_req: Option<&str>) -> Result<RegistryResolution> {
	let normalized = normalize_package_name(name)?;
	let manifest_url = cookbook_manifest_url(&normalized);
	let manifest_raw = fetch_cookbook_manifest(&manifest_url, name)?;
	let manifest: CookbookPackageManifest = toml::from_str(&manifest_raw)
		.with_context(|| format!("failed to parse cookbook manifest {}", manifest_url))?;

	let selection = if manifest.versions.is_empty() {
		None
	} else {
		Some(select_manifest_version(name, &manifest.versions, version_req)?)
	};

	let (source, selected_version, defaults, build) = if let Some(selected) = selection {
		(
			selected.source,
			Some(selected.version),
			selected.defaults.or(manifest.defaults.clone()),
			selected.build.or(manifest.build.clone()),
		)
	} else {
		let source = manifest.source.as_ref().with_context(|| {
			format!(
				"package '{}' has no source definition in cookbook manifest {}",
				name, manifest_url
			)
		})?;
		(
			resolve_source_section(source).with_context(|| {
				format!(
					"package '{}' has invalid source definition in cookbook manifest {}",
					name, manifest_url
				)
			})?,
			manifest.package.version.clone(),
			manifest.defaults.clone(),
			manifest.build.clone(),
		)
	};

	let build_system = build
		.as_ref()
		.and_then(|build| build.system)
		.or(defaults.as_ref().and_then(|defaults| defaults.build_system));

	let export = defaults
		.as_ref()
		.map(|defaults| defaults.export.clone())
		.unwrap_or_default();

	let build = build
		.as_ref()
		.map(cookbook_build_to_controls)
		.unwrap_or_default();

	Ok(RegistryResolution {
		dependency_name: normalized,
		detail: DependencyDetail {
			git: Some(source.url),
			rev: source.rev,
			source_sha256: source.sha256,
			strip_prefix: source.strip_prefix,
			registry_name: Some(manifest.package.name.clone()),
			registry_version: selected_version,
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

fn select_manifest_version(
	package_name: &str,
	versions: &[CookbookVersionSection],
	version_req: Option<&str>,
) -> Result<SelectedCookbookVersion> {
	if versions.is_empty() {
		bail!("package '{}' has no versions in cookbook recipe", package_name);
	}

	let mut parsed_versions = Vec::with_capacity(versions.len());
	for (index, candidate) in versions.iter().enumerate() {
		let parsed = parse_semver_loose(&candidate.version).with_context(|| {
			format!(
				"package '{}' contains invalid version '{}' in cookbook",
				package_name, candidate.version
			)
		})?;
		parsed_versions.push((index, parsed));
	}

	let best_idx = if let Some(raw_req) = version_req {
		let matcher = parse_version_matcher(raw_req).with_context(|| {
			format!(
				"failed to parse requested version '{}' for package '{}'",
				raw_req, package_name
			)
		})?;

		parsed_versions
			.iter()
			.filter(|(_, version)| matcher.matches(version))
			.max_by(|(_, left), (_, right)| left.cmp(right))
			.map(|(index, _)| *index)
			.with_context(|| {
				format!(
					"package '{}' has no version that satisfies '{}'",
					package_name, raw_req
				)
			})?
	} else {
		let stable_idx = parsed_versions
			.iter()
			.filter(|(index, version)| is_stable_release(&versions[*index], version))
			.max_by(|(_, left), (_, right)| left.cmp(right))
			.map(|(index, _)| *index);

		stable_idx.unwrap_or_else(|| {
			parsed_versions
				.iter()
				.max_by(|(_, left), (_, right)| left.cmp(right))
				.map(|(index, _)| *index)
				.unwrap_or(0)
		})
	};

	let selected = &versions[best_idx];
	Ok(SelectedCookbookVersion {
		version: selected.version.clone(),
		source: resolve_typed_source(&selected.source)?,
		defaults: selected.defaults.clone(),
		build: selected.build.clone(),
	})
}

fn is_stable_release(candidate: &CookbookVersionSection, version: &Version) -> bool {
	match candidate.stability {
		Some(CookbookVersionStability::Stable) => true,
		Some(CookbookVersionStability::Prerelease) => false,
		None => version.pre.is_empty(),
	}
}

fn parse_semver_loose(input: &str) -> Result<Version> {
	let trimmed = input.trim();
	if trimmed.is_empty() {
		bail!("version cannot be empty");
	}

	if let Ok(version) = Version::parse(trimmed) {
		return Ok(version);
	}

	let split_idx = trimmed.find(|ch| ch == '-' || ch == '+').unwrap_or(trimmed.len());
	let (core, suffix) = trimmed.split_at(split_idx);
	let parts = core.split('.').collect::<Vec<_>>();

	let normalized = match parts.len() {
		1 => format!("{}.0.0{}", parts[0], suffix),
		2 => format!("{}.0{}", core, suffix),
		_ => bail!("version '{}' is not valid semver", input),
	};

	Version::parse(&normalized).with_context(|| format!("version '{}' is not valid semver", input))
}

fn resolve_source_section(source: &CookbookSourceSection) -> Result<ResolvedCookbookSource> {
	match source {
		CookbookSourceSection::Legacy(legacy) => {
			let url = legacy
				.git
				.clone()
				.or_else(|| legacy.url.clone())
				.or_else(|| legacy.archive.clone())
				.context("source requires either git, url, or archive")?;

			Ok(ResolvedCookbookSource {
				url,
				rev: legacy.rev.clone(),
				sha256: legacy.sha256.clone(),
				strip_prefix: legacy.strip_prefix.clone(),
			})
		}
		CookbookSourceSection::Typed(typed) => resolve_typed_source(typed),
	}
}

fn resolve_typed_source(source: &CookbookTypedSourceSection) -> Result<ResolvedCookbookSource> {
	if source.url.trim().is_empty() {
		bail!("source URL cannot be empty");
	}

	match source.kind {
		CookbookSourceKind::Git | CookbookSourceKind::Archive | CookbookSourceKind::Url => {
			Ok(ResolvedCookbookSource {
				url: source.url.clone(),
				rev: source.rev.clone(),
				sha256: source.sha256.clone(),
				strip_prefix: source.strip_prefix.clone(),
			})
		}
	}
}

fn parse_version_matcher(input: &str) -> Result<VersionMatcher> {
	let trimmed = input.trim();
	if trimmed.is_empty() {
		bail!("version requirement cannot be empty");
	}

	let has_requirement_operator = trimmed
		.chars()
		.any(|ch| matches!(ch, '^' | '~' | '*' | '>' | '<' | '=' | ','));

	if !has_requirement_operator {
		return Ok(VersionMatcher::Exact(parse_semver_loose(trimmed)?));
	}

	let req = VersionReq::parse(trimmed)
		.with_context(|| format!("'{}' is not a valid semver requirement", input))?;
	Ok(VersionMatcher::Requirement(req))
}

#[derive(Debug, Clone, Deserialize)]
struct CookbookPackageManifest {
	#[serde(default, rename = "schema_version")]
	_schema_version: Option<u32>,
	package: CookbookPackageSection,
	#[serde(default)]
	source: Option<CookbookSourceSection>,
	#[serde(default)]
	versions: Vec<CookbookVersionSection>,
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
#[serde(untagged)]
enum CookbookSourceSection {
	Legacy(CookbookLegacySourceSection),
	Typed(CookbookTypedSourceSection),
}

#[derive(Debug, Clone, Deserialize)]
struct CookbookLegacySourceSection {
	#[serde(default)]
	git: Option<String>,
	#[serde(default)]
	url: Option<String>,
	#[serde(default)]
	rev: Option<String>,
	#[serde(default)]
	archive: Option<String>,
	#[serde(default)]
	sha256: Option<String>,
	#[serde(default)]
	strip_prefix: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct CookbookTypedSourceSection {
	kind: CookbookSourceKind,
	url: String,
	#[serde(default)]
	rev: Option<String>,
	#[serde(default)]
	sha256: Option<String>,
	#[serde(default)]
	strip_prefix: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum CookbookSourceKind {
	Git,
	Archive,
	Url,
}

#[derive(Debug, Clone, Deserialize)]
struct CookbookVersionSection {
	version: String,
	#[serde(default)]
	stability: Option<CookbookVersionStability>,
	source: CookbookTypedSourceSection,
	#[serde(default)]
	defaults: Option<CookbookDefaultsSection>,
	#[serde(default)]
	build: Option<CookbookBuildSection>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum CookbookVersionStability {
	Stable,
	Prerelease,
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

#[derive(Debug, Clone)]
struct ResolvedCookbookSource {
	url: String,
	rev: Option<String>,
	sha256: Option<String>,
	strip_prefix: Option<String>,
}

#[derive(Debug, Clone)]
struct SelectedCookbookVersion {
	version: String,
	source: ResolvedCookbookSource,
	defaults: Option<CookbookDefaultsSection>,
	build: Option<CookbookBuildSection>,
}

enum VersionMatcher {
	Exact(Version),
	Requirement(VersionReq),
}

impl VersionMatcher {
	fn matches(&self, candidate: &Version) -> bool {
		match self {
			Self::Exact(exact) => candidate == exact,
			Self::Requirement(req) => req.matches(candidate),
		}
	}
}
