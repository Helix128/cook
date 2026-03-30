use crate::backends::BuildPlan;
use crate::lockfile::CookLock;
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

pub fn compute_build_fingerprint(plan: &BuildPlan, backend_id: &str, lock: &CookLock) -> Result<String> {
    let mut hasher = Sha256::new();

    hasher.update(format!("backend={backend_id}\n"));
    hasher.update(format!("compiler={:?}\n", plan.compiler));
    hasher.update(format!("project={}\n", plan.project_name));
    hasher.update(format!("cpp_standard={}\n", plan.cpp_standard));

    let lock_rendered = toml::to_string(lock).context("failed to serialize lockfile for fingerprint")?;
    hasher.update(lock_rendered.as_bytes());

    for src in &plan.sources {
        hasher.update(format!("src={src}\n"));
        let src_path = Path::new(src);
        if let Ok(metadata) = fs::metadata(src_path) {
            hasher.update(format!("len={}\n", metadata.len()));
            if let Ok(modified) = metadata.modified()
                && let Ok(since_unix) = modified.duration_since(std::time::UNIX_EPOCH)
            {
                hasher.update(format!("mtime={}\n", since_unix.as_secs()));
            }
        }
    }

    for dep in &plan.dependencies {
        hasher.update(format!("dep={}\n", dep.name));
        hasher.update(format!("dep_abi={}\n", dep.abi_fingerprint));
        hasher.update(format!("dep_vis={:?}\n", dep.visibility));
        hasher.update(format!("dep_external={:?}\n", dep.external_build_system));
        hasher.update(format!("dep_external_manifest={:?}\n", dep.external_manifest_path));
        hasher.update(format!("dep_external_manifest_hash={:?}\n", dep.external_manifest_hash));
        hasher.update(format!("dep_external_include_dirs={:?}\n", dep.exports.include_dirs));
        hasher.update(format!("dep_external_lib_dirs={:?}\n", dep.exports.lib_dirs));
        hasher.update(format!("dep_external_libs={:?}\n", dep.exports.libs));
        hasher.update(format!("dep_build_exclude={:?}\n", dep.build.exclude));
        hasher.update(format!("dep_build_configure_args={:?}\n", dep.build.configure_args));
        hasher.update(format!("dep_build_build_args={:?}\n", dep.build.build_args));
        hasher.update(format!("dep_build_install_args={:?}\n", dep.build.install_args));
        hasher.update(format!("dep_build_cmake_defines={:?}\n", dep.build.cmake.defines));
    }

    Ok(format!("{:x}", hasher.finalize()))
}

pub fn cache_hit(cache_key_path: &Path, expected_key: &str, artifact_candidates: &[PathBuf]) -> Result<bool> {
    if !cache_key_path.exists() {
        return Ok(false);
    }

    let current_key = fs::read_to_string(cache_key_path)
        .with_context(|| format!("failed to read {}", cache_key_path.display()))?;

    let key_matches = current_key.trim() == expected_key;
    if !key_matches {
        return Ok(false);
    }

    Ok(artifact_candidates.iter().any(|path| path.exists()))
}

pub fn write_cache_key(cache_key_path: &Path, key: &str) -> Result<()> {
    if let Some(parent) = cache_key_path.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::write(cache_key_path, key)
        .with_context(|| format!("failed to write {}", cache_key_path.display()))
}
