use anyhow::Result;
use std::path::Path;
use walkdir::WalkDir;

pub fn discover_files(root: impl AsRef<Path>) -> Result<Vec<String>> {
	let root = root.as_ref();
	if !root.exists() {
		return Ok(Vec::new());
	}

	let mut files = WalkDir::new(root)
		.into_iter()
		.filter_map(Result::ok)
		.filter(|entry| entry.file_type().is_file())
		.filter_map(|entry| {
			let path = entry.path();
			let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
			if matches!(ext.as_str(), "c" | "cc" | "cpp" | "cxx") {
				let rel = path.strip_prefix(".").unwrap_or(path);
				Some(rel.to_string_lossy().replace('\\', "/"))
			} else {
				None
			}
		})
		.collect::<Vec<_>>();

	files.sort();
	files.dedup();
	Ok(files)
}
