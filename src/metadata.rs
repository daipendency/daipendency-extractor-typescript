use daipendency_extractor::{LibraryMetadata, LibraryMetadataError};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub type TSEntryPoint = HashMap<String, PathBuf>;

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
    #[serde(default)]
    exports: Option<ExportConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ExportConfig {
    Simple(String),
    Map(HashMap<String, ExportConfig>),
}

pub fn extract_metadata(path: &Path) -> Result<TSLibraryMetadata, LibraryMetadataError> {
    let package_json_path = path.join("package.json");
    let content = std::fs::read_to_string(&package_json_path)
        .map_err(LibraryMetadataError::MissingManifest)?;

    let package_json: PackageJson = serde_json::from_str(&content)
        .map_err(|e| LibraryMetadataError::MalformedManifest(e.to_string()))?;

    let entry_point = get_entry_point(&package_json, path);

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

fn get_entry_point(package_json: &PackageJson, path: &Path) -> TSEntryPoint {
    let mut entry_point = HashMap::new();

    // Handle exports
    if let Some(export_config) = &package_json.exports {
        match export_config {
            ExportConfig::Map(export_map) => {
                for (subpath, config) in export_map {
                    if let ExportConfig::Map(conditions) = config {
                        if let Some(ExportConfig::Simple(types_path)) = conditions.get("types") {
                            entry_point.insert(
                                subpath.clone(),
                                path.join(types_path.trim_start_matches("./")),
                            );
                        }
                    }
                }
            }
            ExportConfig::Simple(_) => {}
        }
    } else if let Some(types) = package_json
        .types
        .as_ref()
        .or(package_json.typings.as_ref())
    {
        // Only use types/typings if there's no exports field
        entry_point.insert(".".to_string(), path.join(types));
    }

    entry_point
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
            metadata.entry_point.get("."),
            Some(&temp_dir.path.join("dist/index.d.ts"))
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

    mod entry_point {
        use super::*;

        #[test]
        fn missing_types() {
            let temp_dir = TempDir::new();
            temp_dir
                .create_file(
                    "package.json",
                    r#"{"name": "test-pkg", "version": "1.0.0"}"#,
                )
                .unwrap();

            let metadata = extract_metadata(&temp_dir.path).unwrap();

            assert!(metadata.entry_point.is_empty());
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
                metadata.entry_point.get("."),
                Some(&temp_dir.path.join("dist/index.d.ts"))
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
                metadata.entry_point.get("."),
                Some(&temp_dir.path.join("dist/types.d.ts"))
            );
        }

        mod exports {
            use super::*;

            #[test]
            fn no_exports() {
                let temp_dir = TempDir::new();
                temp_dir
                    .create_file(
                        "package.json",
                        r#"{"name": "test-pkg", "version": "1.0.0", "types": "dist/index.d.ts"}"#,
                    )
                    .unwrap();

                let metadata = extract_metadata(&temp_dir.path).unwrap();

                assert_eq!(
                    metadata.entry_point.get("."),
                    Some(&temp_dir.path.join("dist/index.d.ts"))
                );
            }

            #[test]
            fn export_without_types() {
                let temp_dir = TempDir::new();
                temp_dir
                    .create_file(
                        "package.json",
                        r#"{
                            "name": "test-pkg",
                            "version": "1.0.0",
                            "types": "dist/index.d.ts",
                            "exports": {
                                ".": {
                                    "import": "./dist/index.js"
                                }
                            }
                        }"#,
                    )
                    .unwrap();

                let metadata = extract_metadata(&temp_dir.path).unwrap();

                assert!(metadata.entry_point.is_empty());
            }

            #[test]
            fn single_type_export() {
                let temp_dir = TempDir::new();
                temp_dir
                    .create_file(
                        "package.json",
                        r#"{
                            "name": "test-pkg",
                            "version": "1.0.0",
                            "types": "dist/index.d.ts",
                            "exports": {
                                ".": {
                                    "types": "./dist/index.d.ts"
                                }
                            }
                        }"#,
                    )
                    .unwrap();

                let metadata = extract_metadata(&temp_dir.path).unwrap();

                assert_eq!(metadata.entry_point.len(), 1);
                assert_eq!(
                    metadata.entry_point.get("."),
                    Some(&temp_dir.path.join("dist/index.d.ts"))
                );
            }

            #[test]
            fn multiple_type_exports() {
                let temp_dir = TempDir::new();
                temp_dir
                    .create_file(
                        "package.json",
                        r#"{
                            "name": "test-pkg",
                            "version": "1.0.0",
                            "types": "dist/index.d.ts",
                            "exports": {
                                ".": {
                                    "types": "./dist/index.d.ts"
                                },
                                "./utils": {
                                    "types": "./dist/utils.d.ts"
                                }
                            }
                        }"#,
                    )
                    .unwrap();

                let metadata = extract_metadata(&temp_dir.path).unwrap();

                assert_eq!(metadata.entry_point.len(), 2);
                assert_eq!(
                    metadata.entry_point.get("."),
                    Some(&temp_dir.path.join("dist/index.d.ts"))
                );
                assert_eq!(
                    metadata.entry_point.get("./utils"),
                    Some(&temp_dir.path.join("dist/utils.d.ts"))
                );
            }

            #[test]
            fn export_as_string() {
                let temp_dir = TempDir::new();
                temp_dir
                    .create_file(
                        "package.json",
                        r#"{
                            "name": "test-pkg",
                            "version": "1.0.0",
                            "types": "dist/index.d.ts",
                            "exports": {
                                ".": "./dist/index.js"
                            }
                        }"#,
                    )
                    .unwrap();

                let metadata = extract_metadata(&temp_dir.path).unwrap();

                assert!(metadata.entry_point.is_empty());
            }

            #[test]
            fn exports_as_string() {
                let temp_dir = TempDir::new();
                temp_dir
                    .create_file(
                        "package.json",
                        r#"{
                            "name": "test-pkg",
                            "version": "1.0.0",
                            "types": "dist/index.d.ts",
                            "exports": "./dist/index.js"
                        }"#,
                    )
                    .unwrap();

                let metadata = extract_metadata(&temp_dir.path).unwrap();

                assert!(metadata.entry_point.is_empty());
            }
        }
    }
}
