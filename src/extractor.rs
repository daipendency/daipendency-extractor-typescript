use std::path::{Path, PathBuf};

use crate::{
    api, dependencies,
    metadata::{extract_metadata, TSLibraryMetadata},
};
use daipendency_extractor::{
    DependencyResolutionError, ExtractionError, Extractor, LibraryMetadataError, Namespace,
};
use tree_sitter::{Language, Parser};

pub struct TypeScriptExtractor;

impl Extractor<PathBuf> for TypeScriptExtractor {
    fn get_parser_language(&self) -> Language {
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
    }

    fn get_library_metadata(&self, path: &Path) -> Result<TSLibraryMetadata, LibraryMetadataError> {
        extract_metadata(path)
    }

    fn extract_public_api(
        &self,
        library_metadata: &TSLibraryMetadata,
        parser: &mut Parser,
    ) -> Result<Vec<Namespace>, ExtractionError> {
        api::extract_public_api(library_metadata, parser)
    }

    fn resolve_dependency_path(
        &self,
        name: &str,
        dependant_path: &Path,
    ) -> Result<PathBuf, DependencyResolutionError> {
        dependencies::resolve_dependency_path(name, dependant_path)
    }
}
