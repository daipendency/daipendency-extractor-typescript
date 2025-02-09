use std::path::{Path, PathBuf};

use daipendency_extractor::DependencyResolutionError;

pub fn resolve_dependency_path(
    name: &str,
    dependant_path: &Path,
) -> Result<PathBuf, DependencyResolutionError> {
    if let Some(path) = recursive_resolve_dependency_path(name, dependant_path) {
        Ok(path)
    } else {
        Err(DependencyResolutionError::MissingDependency(
            name.to_string(),
        ))
    }
}

fn recursive_resolve_dependency_path(name: &str, dependant_path: &Path) -> Option<PathBuf> {
    if !dependant_path.join("package.json").exists() {
        return None;
    }

    let node_modules_path = dependant_path.join("node_modules").join(name);
    if node_modules_path.exists() {
        return Some(node_modules_path);
    }

    dependant_path
        .parent()
        .and_then(|parent| recursive_resolve_dependency_path(name, parent))
}

#[cfg(test)]
mod tests {
    use super::*;
    use assertables::assert_matches;
    use daipendency_testing::tempdir::TempDir;
    use std::fs;

    #[test]
    fn missing_manifest() {
        let temp_dir = TempDir::new();
        let dependant_path = temp_dir.path.clone();

        let result = resolve_dependency_path("some-dep", &dependant_path);

        assert_matches!(
            result,
            Err(DependencyResolutionError::MissingDependency(msg)) if msg == "some-dep"
        );
    }

    #[test]
    fn dependant_contains_dependency() {
        let temp_dir = TempDir::new();
        let dependant_path = temp_dir.path.clone();
        temp_dir.create_file("package.json", "{}").unwrap();
        fs::create_dir_all(dependant_path.join("node_modules/some-dep")).unwrap();

        let result = resolve_dependency_path("some-dep", &dependant_path);

        assert_eq!(
            result.unwrap(),
            dependant_path.join("node_modules/some-dep")
        );
    }

    #[test]
    fn parent_contains_dependency() {
        let temp_dir = TempDir::new();
        let parent_path = temp_dir.path.clone();
        temp_dir.create_file("package.json", "{}").unwrap();
        temp_dir
            .create_file("node_modules/some-dep/package.json", "{}")
            .unwrap();
        let child_manifest_path = temp_dir.create_file("child/package.json", "{}").unwrap();
        let child_directory = child_manifest_path.parent().unwrap();

        let result = resolve_dependency_path("some-dep", &child_directory);

        assert_eq!(result.unwrap(), parent_path.join("node_modules/some-dep"));
    }

    #[test]
    fn grandparent_contains_dependency() {
        let temp_dir = TempDir::new();
        let grandparent_path = temp_dir.path.clone();
        temp_dir.create_file("package.json", "{}").unwrap();
        temp_dir
            .create_file("node_modules/some-dep/package.json", "{}")
            .unwrap();
        temp_dir.create_file("parent/package.json", "{}").unwrap();
        let child_manifest_path = temp_dir
            .create_file("parent/child/package.json", "{}")
            .unwrap();
        let child_directory = child_manifest_path.parent().unwrap();

        let result = resolve_dependency_path("some-dep", &child_directory);

        assert_eq!(
            result.unwrap(),
            grandparent_path.join("node_modules/some-dep")
        );
    }
}
