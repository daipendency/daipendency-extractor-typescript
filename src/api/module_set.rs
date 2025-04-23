use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::read_to_string;
use std::path::{Path, PathBuf};

use daipendency_extractor::ExtractionError;
use tree_sitter::Parser;

use crate::api::module::{Module, TypeScriptSymbol};
use crate::api::parsing::parse_typescript_file;
use crate::metadata::TSEntryPoint;

/// Represents a set of TypeScript modules.
///
/// We derive Default to allow creating an empty ModuleSet instance with ModuleSet::default().
/// This is useful in cases where you need to initialize a ModuleSet before populating it.
#[derive(Debug, Default)]
pub struct ModuleSet(pub HashMap<PathBuf, Module>);

impl ModuleSet {
    /// Builds a module set from the given entry points.
    ///
    /// # Arguments
    ///
    /// * `entry_points` - A map of entry point names to file paths
    /// * `parser` - A tree-sitter parser configured for TypeScript
    ///
    /// # Returns
    ///
    /// A complete set of modules reachable from the entry points
    pub fn from_entrypoints(
        entry_points: &TSEntryPoint,
        parser: &mut Parser,
    ) -> Result<Self, ExtractionError> {
        let mut modules = HashMap::new();
        let mut queue = VecDeque::new();
        let mut visited_paths = HashSet::new();

        for path in entry_points.values() {
            queue.push_back(path.clone());
        }

        while let Some(current_path) = queue.pop_front() {
            if visited_paths.contains(&current_path) {
                continue;
            }

            visited_paths.insert(current_path.clone());

            let content = match read_to_string(&current_path) {
                Ok(content) => content,
                Err(e) => {
                    let path_str = current_path.display().to_string();
                    return Err(ExtractionError::Io(std::io::Error::new(
                        e.kind(),
                        format!("Failed to read file at '{}': {}", path_str, e),
                    )));
                }
            };
            let module = parse_typescript_file(&content, parser)?;
            modules.insert(current_path.clone(), module.clone());

            let dependencies = get_imported_module_paths(&module, &current_path);
            for dependency in dependencies {
                queue.push_back(dependency);
            }
        }

        Ok(ModuleSet(modules))
    }
}

/// Provides HashMap-like access semantics without needing to reference the inner field
impl std::ops::Deref for ModuleSet {
    type Target = HashMap<PathBuf, Module>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

fn normalise_file_path(path: &PathBuf) -> Option<PathBuf> {
    if let Ok(path) = path.canonicalize() {
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

fn get_imported_module_paths(module: &Module, path: &Path) -> Vec<PathBuf> {
    let mut dependencies = Vec::new();

    for symbol in &module.symbols {
        if let TypeScriptSymbol::ModuleImport { source_module, .. } = symbol {
            if let Some(resolved_path) = resolve_relative_import(path, source_module) {
                dependencies.push(resolved_path);
            }
        } else if let TypeScriptSymbol::ModuleExport {
            source_module: Some(source_module),
            ..
        } = symbol
        {
            if let Some(resolved_path) = resolve_relative_import(path, source_module) {
                dependencies.push(resolved_path);
            }
        }
    }

    dependencies
}

fn resolve_relative_import(module_path: &Path, import_path: &str) -> Option<PathBuf> {
    if import_path.starts_with("./") || import_path.starts_with("../") {
        let parent_dir = module_path.parent()?;
        let resolved_path = parent_dir.join(import_path);

        if let Some(path) = normalise_file_path(&resolved_path) {
            return Some(path);
        }

        if let Some(path) = normalise_file_path(&resolved_path.with_extension("d.ts")) {
            return Some(path);
        }

        if let Some(path) = normalise_file_path(&resolved_path.with_extension("ts")) {
            return Some(path);
        }

        if resolved_path.is_dir() {
            let with_index_dts = resolved_path.join("index.d.ts");
            if let Some(path) = normalise_file_path(&with_index_dts) {
                return Some(path);
            }

            let with_index_ts = resolved_path.join("index.ts");
            if let Some(path) = normalise_file_path(&with_index_ts) {
                return Some(path);
            }
        }

        // The path doesn't exist but it isn't our responsibility to error out due to that
        return Some(resolved_path);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::test_helpers::make_parser;
    use assertables::{assert_contains, assert_matches};
    use daipendency_testing::tempdir::TempDir;

    struct EntrypointFixture {
        temp_dir: TempDir,
        files: HashMap<String, String>,
        entrypoints: HashMap<String, String>,
    }

    impl EntrypointFixture {
        fn new<F, E>(files: F, entrypoints: E) -> Self
        where
            F: IntoIterator<Item = (&'static str, &'static str)>,
            E: IntoIterator<Item = (&'static str, &'static str)>,
        {
            let files_map = files
                .into_iter()
                .map(|(path, content)| (path.to_string(), content.to_string()))
                .collect();

            let entrypoints_map = entrypoints
                .into_iter()
                .map(|(key, path)| (key.to_string(), path.to_string()))
                .collect();

            Self {
                temp_dir: TempDir::new(),
                files: files_map,
                entrypoints: entrypoints_map,
            }
        }

        fn make_path(&self, path: &str) -> PathBuf {
            self.temp_dir.path.join(path)
        }

        fn generate_entry_points(&self) -> TSEntryPoint {
            for (path, content) in &self.files {
                self.temp_dir.create_file(path, content).unwrap();
            }

            self.entrypoints
                .iter()
                .map(|(key, path)| (key.clone(), self.make_path(path)))
                .collect()
        }
    }

    mod from_entrypoints {
        use super::*;

        #[test]
        fn empty_metadata() {
            let fixture = EntrypointFixture::new([], []);
            let entrypoints = fixture.generate_entry_points();
            let mut parser = make_parser();

            let modules = ModuleSet::from_entrypoints(&entrypoints, &mut parser).unwrap();

            assert_eq!(modules.len(), 0);
        }

        #[test]
        fn single_entry_point() {
            let fixture = EntrypointFixture::new(
                [("index.d.ts", "export const foo: string;")],
                [("main", "index.d.ts")],
            );
            let entrypoints = fixture.generate_entry_points();
            let mut parser = make_parser();

            let modules = ModuleSet::from_entrypoints(&entrypoints, &mut parser).unwrap();

            assert_eq!(modules.len(), 1);
            assert_contains!(
                modules.keys().collect::<HashSet<_>>(),
                &fixture.make_path("index.d.ts")
            );
        }

        #[test]
        fn multiple_entry_points() {
            let fixture = EntrypointFixture::new(
                [
                    ("index.d.ts", "export const foo: string;"),
                    ("other.d.ts", "export const bar: number;"),
                ],
                [("main", "index.d.ts"), ("other", "other.d.ts")],
            );
            let entrypoints = fixture.generate_entry_points();
            let mut parser = make_parser();

            let modules = ModuleSet::from_entrypoints(&entrypoints, &mut parser).unwrap();

            assert_eq!(modules.len(), 2);
            let keys: HashSet<_> = modules.keys().collect();
            assert_contains!(keys, &fixture.make_path("index.d.ts"));
            assert_contains!(keys, &fixture.make_path("other.d.ts"));
        }

        #[test]
        fn non_existing_entry_point() {
            let path = PathBuf::from("./non-existing-file.d.ts");
            let entrypoints: TSEntryPoint =
                [("main".to_string(), path.clone())].into_iter().collect();
            let mut parser = make_parser();

            let result = ModuleSet::from_entrypoints(&entrypoints, &mut parser);

            assert_matches!(result, Err(ExtractionError::Io(_)));
            assert_contains!(
                result.unwrap_err().to_string(),
                &path.to_string_lossy().to_string()
            );
        }

        #[test]
        fn parsing_error() {
            let fixture = EntrypointFixture::new(
                [("index.d.ts", "export const foo: @invalid-type;")],
                [("main", "index.d.ts")],
            );
            let entrypoints = fixture.generate_entry_points();
            let mut parser = make_parser();

            let result = ModuleSet::from_entrypoints(&entrypoints, &mut parser);

            assert_matches!(result, Err(ExtractionError::Malformed(_)));
        }
    }

    mod module_imports {
        use super::*;

        #[test]
        fn direct_import() {
            let fixture = EntrypointFixture::new(
                [
                    (
                        "index.d.ts",
                        "import { Bar } from './bar';\nexport const foo: string;",
                    ),
                    ("bar.d.ts", "export interface Bar { prop: string; }"),
                ],
                [("main", "index.d.ts")],
            );
            let entrypoints = fixture.generate_entry_points();
            let mut parser = make_parser();

            let modules = ModuleSet::from_entrypoints(&entrypoints, &mut parser).unwrap();

            assert_eq!(modules.len(), 2);
            let keys: HashSet<_> = modules.keys().collect();
            assert_contains!(keys, &fixture.make_path("index.d.ts"));
            assert_contains!(keys, &fixture.make_path("bar.d.ts"));
        }

        #[test]
        fn transitive_dependencies() {
            let fixture = EntrypointFixture::new(
                [
                    (
                        "index.d.ts",
                        "import { Bar } from './bar';\nexport const foo: string;",
                    ),
                    (
                        "bar.d.ts",
                        "import { Baz } from './baz';\nexport interface Bar { prop: Baz; }",
                    ),
                    ("baz.d.ts", "export interface Baz { value: number; }"),
                ],
                [("main", "index.d.ts")],
            );
            let entrypoints = fixture.generate_entry_points();
            let mut parser = make_parser();

            let modules = ModuleSet::from_entrypoints(&entrypoints, &mut parser).unwrap();

            assert_eq!(modules.len(), 3);
            let keys: HashSet<_> = modules.keys().collect();
            assert_contains!(keys, &fixture.make_path("index.d.ts"));
            assert_contains!(keys, &fixture.make_path("bar.d.ts"));
            assert_contains!(keys, &fixture.make_path("baz.d.ts"));
        }

        #[test]
        fn circular_dependencies() {
            let fixture = EntrypointFixture::new(
                [
                    (
                        "a.d.ts",
                        "import { B } from './b';\nexport interface A { b: B; }",
                    ),
                    (
                        "b.d.ts",
                        "import { A } from './a';\nexport interface B { a: A; }",
                    ),
                ],
                [("main", "a.d.ts")],
            );
            let entrypoints = fixture.generate_entry_points();
            let mut parser = make_parser();

            let modules = ModuleSet::from_entrypoints(&entrypoints, &mut parser).unwrap();

            assert_eq!(modules.len(), 2);
            let keys: HashSet<_> = modules.keys().collect();
            assert_contains!(keys, &fixture.make_path("a.d.ts"));
            assert_contains!(keys, &fixture.make_path("b.d.ts"));
        }

        #[test]
        fn reexport_dependencies() {
            let fixture = EntrypointFixture::new(
                [
                    ("index.d.ts", "export { Something } from './other-module';"),
                    (
                        "other-module.d.ts",
                        "export interface Something { value: number; }",
                    ),
                ],
                [("main", "index.d.ts")],
            );
            let entrypoints = fixture.generate_entry_points();
            let mut parser = make_parser();

            let modules = ModuleSet::from_entrypoints(&entrypoints, &mut parser).unwrap();

            assert_eq!(modules.len(), 2);
            let keys: HashSet<_> = modules.keys().collect();
            assert_contains!(keys, &fixture.make_path("index.d.ts"));
            assert_contains!(keys, &fixture.make_path("other-module.d.ts"));
        }
    }

    mod path_resolution {
        use super::*;

        #[test]
        fn relative_path() {
            let fixture = EntrypointFixture::new(
                [
                    (
                        "src/index.d.ts",
                        "import { Foo } from './foo';\nexport const bar: Foo;",
                    ),
                    ("src/foo.d.ts", "export interface Foo { value: string; }"),
                ],
                [("main", "src/index.d.ts")],
            );
            let entrypoints = fixture.generate_entry_points();
            let mut parser = make_parser();

            let modules = ModuleSet::from_entrypoints(&entrypoints, &mut parser).unwrap();

            assert_eq!(modules.len(), 2);
            let keys: HashSet<_> = modules.keys().collect();
            assert_contains!(keys, &fixture.make_path("src/index.d.ts"));
            assert_contains!(keys, &fixture.make_path("src/foo.d.ts"));
        }

        #[test]
        fn parent_directory() {
            let fixture = EntrypointFixture::new(
                [
                    ("src/parent-module.d.ts", "export interface ParentExport { value: string; }"),
                    ("src/nested/child-module.d.ts", "import { ParentExport } from '../parent-module';\nexport const child: ParentExport;"),
                ],
                [("child", "src/nested/child-module.d.ts")],
            );
            let entrypoints = fixture.generate_entry_points();
            let mut parser = make_parser();

            let modules = ModuleSet::from_entrypoints(&entrypoints, &mut parser).unwrap();

            assert_eq!(modules.len(), 2);
            let keys: HashSet<_> = modules.keys().collect();
            assert_contains!(keys, &fixture.make_path("src/nested/child-module.d.ts"));
            assert_contains!(keys, &fixture.make_path("src/parent-module.d.ts"));
        }

        #[test]
        fn directory_with_index() {
            let fixture = EntrypointFixture::new(
                [
                    (
                        "src/index.d.ts",
                        "import { Foo } from './utils';\nexport const bar: Foo;",
                    ),
                    (
                        "src/utils/index.d.ts",
                        "export interface Foo { value: string; }",
                    ),
                ],
                [("main", "src/index.d.ts")],
            );
            let entrypoints = fixture.generate_entry_points();
            let mut parser = make_parser();

            let modules = ModuleSet::from_entrypoints(&entrypoints, &mut parser).unwrap();

            assert_eq!(modules.len(), 2);
            let keys: HashSet<_> = modules.keys().collect();
            assert_contains!(keys, &fixture.make_path("src/index.d.ts"));
            assert_contains!(keys, &fixture.make_path("src/utils/index.d.ts"));
        }

        #[test]
        fn typescript_extension_variants() {
            let fixture = EntrypointFixture::new(
                [
                    (
                        "src/index.d.ts",
                        "import { Foo } from './foo';\nexport const bar: Foo;",
                    ),
                    ("src/foo.ts", "export interface Foo { value: string; }"),
                ],
                [("main", "src/index.d.ts")],
            );
            let entrypoints = fixture.generate_entry_points();
            let mut parser = make_parser();

            let modules = ModuleSet::from_entrypoints(&entrypoints, &mut parser).unwrap();

            assert_eq!(modules.len(), 2);
            let keys: HashSet<_> = modules.keys().collect();
            assert_contains!(keys, &fixture.make_path("src/index.d.ts"));
            assert_contains!(keys, &fixture.make_path("src/foo.ts"));
        }

        #[test]
        fn non_relative_import_is_ignored() {
            let fixture = EntrypointFixture::new(
                [(
                    "index.d.ts",
                    "import { Something } from 'external-module';\nexport const foo: Something;",
                )],
                [("main", "index.d.ts")],
            );
            let entrypoints = fixture.generate_entry_points();
            let mut parser = make_parser();

            let modules = ModuleSet::from_entrypoints(&entrypoints, &mut parser).unwrap();

            assert_eq!(modules.len(), 1);
            assert_contains!(
                modules.keys().collect::<HashSet<_>>(),
                &fixture.make_path("index.d.ts")
            );
        }

        #[test]
        fn directory_with_index_ts() {
            let fixture = EntrypointFixture::new(
                [
                    (
                        "src/index.d.ts",
                        "import { Foo } from './utils';\nexport const bar: Foo;",
                    ),
                    (
                        "src/utils/index.ts",
                        "export interface Foo { value: string; }",
                    ),
                ],
                [("main", "src/index.d.ts")],
            );
            let entrypoints = fixture.generate_entry_points();
            let mut parser = make_parser();

            let modules = ModuleSet::from_entrypoints(&entrypoints, &mut parser).unwrap();

            assert_eq!(modules.len(), 2);
            let keys: HashSet<_> = modules.keys().collect();
            assert_contains!(keys, &fixture.make_path("src/index.d.ts"));
            assert_contains!(keys, &fixture.make_path("src/utils/index.ts"));
        }

        #[test]
        fn direct_file_resolution() {
            let fixture = EntrypointFixture::new(
                [
                    (
                        "src/index.d.ts",
                        "import { Foo } from './exact-file';\nexport const bar: Foo;",
                    ),
                    ("src/exact-file", "export interface Foo { value: string; }"),
                ],
                [("main", "src/index.d.ts")],
            );
            let entrypoints = fixture.generate_entry_points();
            let mut parser = make_parser();

            let modules = ModuleSet::from_entrypoints(&entrypoints, &mut parser).unwrap();

            assert_eq!(modules.len(), 2);
            let keys: HashSet<_> = modules.keys().collect();
            assert_contains!(keys, &fixture.make_path("src/index.d.ts"));
            assert_contains!(keys, &fixture.make_path("src/exact-file"));
        }

        #[test]
        fn non_existing_import() {
            let fixture = EntrypointFixture::new(
                [(
                    "src/index.d.ts",
                    "import nonExisting from './non-existing.ts';",
                )],
                [("main", "src/index.d.ts")],
            );
            let entrypoints = fixture.generate_entry_points();
            let mut parser = make_parser();

            let result = ModuleSet::from_entrypoints(&entrypoints, &mut parser);

            assert_matches!(result, Err(ExtractionError::Io(_)));
            assert_contains!(result.unwrap_err().to_string(), "non-existing.ts");
        }
    }
}
