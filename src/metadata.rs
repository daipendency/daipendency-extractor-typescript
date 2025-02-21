use daipendency_extractor::{LibraryMetadata, LibraryMetadataError};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// TypeScript library metadata.
pub type TSLibraryMetadata = LibraryMetadata<PathBuf>;

#[derive(Debug, Deserialize)]
struct PackageJson {
    name: String,
    version: String,
    types: String,
}

pub fn extract_metadata(path: &Path) -> Result<TSLibraryMetadata, LibraryMetadataError> {
    let package_json_path = path.join("package.json");
    let content = std::fs::read_to_string(&package_json_path)
        .map_err(LibraryMetadataError::MissingManifest)?;

    let package: PackageJson = serde_json::from_str(&content)
        .map_err(|e| LibraryMetadataError::MalformedManifest(e.to_string()))?;

    let documentation = read_readme(path);

    Ok(TSLibraryMetadata {
        name: package.name,
        version: Some(package.version),
        documentation,
        entry_point: path.join(package.types),
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

        assert_matches!(result, Err(LibraryMetadataError::MalformedManifest(ref s)) if s.contains("missing field `types`"));
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

        let result = extract_metadata(&temp_dir.path);

        assert!(result.is_ok());
        let metadata = result.unwrap();
        assert_eq!(metadata.name, "test-pkg");
        assert_eq!(metadata.version, Some("1.0.0".to_string()));
        assert_eq!(metadata.entry_point, temp_dir.path.join("dist/index.d.ts"));
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

            let result = extract_metadata(&temp_dir.path);

            assert!(result.is_ok());
            assert_eq!(result.unwrap().documentation, "");
        }

        #[test]
        fn readme_md() {
            let temp_dir = TempDir::new();
            temp_dir.create_file("package.json", PACKAGE_JSON).unwrap();
            temp_dir.create_file("README.md", README_CONTENT).unwrap();

            let result = extract_metadata(&temp_dir.path);

            assert!(result.is_ok());
            let metadata = result.unwrap();
            assert_eq!(metadata.documentation, README_CONTENT);
        }

        #[test]
        fn readme_txt() {
            let temp_dir = TempDir::new();
            temp_dir.create_file("package.json", PACKAGE_JSON).unwrap();
            temp_dir.create_file("README.txt", README_CONTENT).unwrap();

            let result = extract_metadata(&temp_dir.path);

            assert!(result.is_ok());
            let metadata = result.unwrap();
            assert_eq!(metadata.documentation, README_CONTENT);
        }

        #[test]
        fn readme_without_extension() {
            let temp_dir = TempDir::new();
            temp_dir.create_file("package.json", PACKAGE_JSON).unwrap();
            temp_dir.create_file("README", README_CONTENT).unwrap();

            let result = extract_metadata(&temp_dir.path);

            assert!(result.is_ok());
            let metadata = result.unwrap();
            assert_eq!(metadata.documentation, README_CONTENT);
        }
    }
}
