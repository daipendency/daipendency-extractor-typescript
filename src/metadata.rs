use daipendency_extractor::{LibraryMetadata, LibraryMetadataError};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct TSEntryPoint {
    /// Path to the TypeScript declaration file specified in the `types` or `typings` field of `package.json`.
    pub types_path: PathBuf,
}

/// TypeScript library metadata.
pub type TSLibraryMetadata = LibraryMetadata<TSEntryPoint>;

#[derive(Debug, Deserialize)]
struct PackageJson {
    name: String,
    version: String,
    #[serde(default)]
    types: Option<String>,
    #[serde(default)]
    typings: Option<String>,
}

pub fn extract_metadata(path: &Path) -> Result<TSLibraryMetadata, LibraryMetadataError> {
    let package_json_path = path.join("package.json");
    let content = std::fs::read_to_string(&package_json_path)
        .map_err(LibraryMetadataError::MissingManifest)?;

    let package_json: PackageJson = serde_json::from_str(&content)
        .map_err(|e| LibraryMetadataError::MalformedManifest(e.to_string()))?;

    let entry_point = get_entry_point(&package_json, path)?;

    let documentation = read_readme(path);

    Ok(TSLibraryMetadata {
        name: package_json.name,
        version: Some(package_json.version),
        documentation,
        entry_point,
    })
}

fn read_readme(path: &Path) -> String {
    let readme_paths = ["README.md", "README.txt", "README"];
    for readme_path in readme_paths {
        if let Ok(content) = std::fs::read_to_string(path.join(readme_path)) {
            return content;
        }
    }
    String::new()
}

fn get_entry_point(
    package_json: &PackageJson,
    path: &Path,
) -> Result<TSEntryPoint, LibraryMetadataError> {
    let types_path = package_json
        .types
        .as_ref()
        .or(package_json.typings.as_ref())
        .ok_or_else(|| {
            LibraryMetadataError::MalformedManifest(
                "neither 'types' nor 'typings' field specified".to_string(),
            )
        })?;
    let entry_point = TSEntryPoint {
        types_path: path.join(types_path),
    };
    Ok(entry_point)
}

#[cfg(test)]
mod tests {
    use super::*;
    use assertables::assert_matches;
    use daipendency_testing::tempdir::TempDir;

    #[test]
    fn missing_manifest() {
        let temp_dir = TempDir::new();

        let result = extract_metadata(&temp_dir.path);

        assert_matches!(result, Err(LibraryMetadataError::MissingManifest(ref e)) if e.kind() == std::io::ErrorKind::NotFound);
    }

    #[test]
    fn malformed_manifest() {
        let temp_dir = TempDir::new();
        temp_dir.create_file("package.json", "not json").unwrap();

        let result = extract_metadata(&temp_dir.path);

        assert_matches!(result, Err(LibraryMetadataError::MalformedManifest(ref e)) if e.contains("expected ident"));
    }

    #[test]
    fn missing_package_name() {
        let temp_dir = TempDir::new();
        temp_dir
            .create_file("package.json", r#"{"version": "1.0.0"}"#)
            .unwrap();

        let result = extract_metadata(&temp_dir.path);

        assert_matches!(result, Err(LibraryMetadataError::MalformedManifest(ref s)) if s.contains("missing field `name`"));
    }

    #[test]
    fn missing_package_version() {
        let temp_dir = TempDir::new();
        temp_dir
            .create_file(
                "package.json",
                r#"{"name": "test-pkg", "types": "dist/index.d.ts"}"#,
            )
            .unwrap();

        let result = extract_metadata(&temp_dir.path);

        assert_matches!(result, Err(LibraryMetadataError::MalformedManifest(ref s)) if s.contains("missing field `version`"));
    }

    #[test]
    fn missing_types() {
        let temp_dir = TempDir::new();
        temp_dir
            .create_file(
                "package.json",
                r#"{"name": "test-pkg", "version": "1.0.0"}"#,
            )
            .unwrap();

        let result = extract_metadata(&temp_dir.path);

        assert_matches!(result, Err(LibraryMetadataError::MalformedManifest(ref s)) if s.contains("neither 'types' nor 'typings' field specified"));
    }

    #[test]
    fn valid_manifest() {
        let temp_dir = TempDir::new();
        temp_dir
            .create_file(
                "package.json",
                r#"{"name": "test-pkg", "version": "1.0.0", "types": "dist/index.d.ts"}"#,
            )
            .unwrap();

        let metadata = extract_metadata(&temp_dir.path).unwrap();

        assert_eq!(metadata.name, "test-pkg");
        assert_eq!(metadata.version, Some("1.0.0".to_string()));
        assert_eq!(
            metadata.entry_point.types_path,
            temp_dir.path.join("dist/index.d.ts")
        );
    }

    #[test]
    fn valid_manifest_with_typings() {
        let temp_dir = TempDir::new();
        temp_dir
            .create_file(
                "package.json",
                r#"{"name": "test-pkg", "version": "1.0.0", "typings": "dist/index.d.ts"}"#,
            )
            .unwrap();

        let metadata = extract_metadata(&temp_dir.path).unwrap();

        assert_eq!(
            metadata.entry_point.types_path,
            temp_dir.path.join("dist/index.d.ts")
        );
    }

    #[test]
    fn valid_manifest_with_both_types_and_typings() {
        let temp_dir = TempDir::new();
        temp_dir
            .create_file(
                "package.json",
                r#"{"name": "test-pkg", "version": "1.0.0", "types": "dist/types.d.ts", "typings": "dist/typings.d.ts"}"#,
            )
            .unwrap();

        let metadata = extract_metadata(&temp_dir.path).unwrap();

        assert_eq!(
            metadata.entry_point.types_path,
            temp_dir.path.join("dist/types.d.ts")
        );
    }

    mod readme {
        use super::*;

        const PACKAGE_JSON: &str =
            r#"{"name": "test-pkg", "version": "1.0.0", "types": "dist/index.d.ts"}"#;
        const README_CONTENT: &str = "# Test Package";

        #[test]
        fn missing_readme() {
            let temp_dir = TempDir::new();
            temp_dir.create_file("package.json", PACKAGE_JSON).unwrap();

            let metadata = extract_metadata(&temp_dir.path).unwrap();

            assert_eq!(metadata.documentation, "");
        }

        #[test]
        fn readme_md() {
            let temp_dir = TempDir::new();
            temp_dir.create_file("package.json", PACKAGE_JSON).unwrap();
            temp_dir.create_file("README.md", README_CONTENT).unwrap();

            let metadata = extract_metadata(&temp_dir.path).unwrap();

            assert_eq!(metadata.documentation, README_CONTENT);
        }

        #[test]
        fn readme_txt() {
            let temp_dir = TempDir::new();
            temp_dir.create_file("package.json", PACKAGE_JSON).unwrap();
            temp_dir.create_file("README.txt", README_CONTENT).unwrap();

            let metadata = extract_metadata(&temp_dir.path).unwrap();

            assert_eq!(metadata.documentation, README_CONTENT);
        }

        #[test]
        fn readme_without_extension() {
            let temp_dir = TempDir::new();
            temp_dir.create_file("package.json", PACKAGE_JSON).unwrap();
            temp_dir.create_file("README", README_CONTENT).unwrap();

            let metadata = extract_metadata(&temp_dir.path).unwrap();

            assert_eq!(metadata.documentation, README_CONTENT);
        }
    }
}
