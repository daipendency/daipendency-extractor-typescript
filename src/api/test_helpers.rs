#![cfg(test)]

use crate::TypeScriptExtractor;
use assertables::assert_matches;
use daipendency_extractor::Extractor;
use std::collections::HashMap;
use tree_sitter::Parser;

use super::module::{ExportTarget, ImportTarget, TypeScriptSymbol};

pub fn make_parser() -> Parser {
    let mut parser = Parser::new();
    let language = TypeScriptExtractor.get_parser_language();
    parser.set_language(&language).unwrap();
    parser
}

/// Deconstructs a `TypeScriptSymbol::ModuleImport` into its source module and target.
pub fn deconstruct_module_import(symbol: &TypeScriptSymbol) -> (String, ImportTarget) {
    match symbol {
        TypeScriptSymbol::ModuleImport {
            source_module,
            target,
        } => (source_module.clone(), target.clone()),
        _ => panic!("Expected module import"),
    }
}

/// Deconstructs a `TypeScriptSymbol::Namespace` into its name, content, is_exported and jsdoc.
pub fn deconstruct_namespace(
    symbol: &TypeScriptSymbol,
) -> (String, Vec<TypeScriptSymbol>, bool, Option<String>) {
    match symbol {
        TypeScriptSymbol::Namespace {
            name,
            content,
            is_exported,
            jsdoc,
        } => (name.clone(), content.clone(), *is_exported, jsdoc.clone()),
        _ => panic!("Expected namespace"),
    }
}

/// Deconstructs a `TypeScriptSymbol::ModuleExport` into its source module and target.
pub fn deconstruct_module_export(symbol: &TypeScriptSymbol) -> (Option<String>, ExportTarget) {
    match symbol {
        TypeScriptSymbol::ModuleExport {
            source_module,
            target,
        } => (source_module.clone(), target.clone()),
        _ => panic!("Expected module export"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod module_import_deconstruction {
        use super::*;

        #[test]
        fn import() {
            let symbol = TypeScriptSymbol::ModuleImport {
                source_module: "lodash".to_string(),
                target: ImportTarget::Default {
                    name: "lodash".to_string(),
                },
            };

            let (module, target) = deconstruct_module_import(&symbol);

            assert_eq!(module, "lodash");
            assert_eq!(
                target,
                ImportTarget::Default {
                    name: "lodash".to_string()
                }
            );
        }

        #[test]
        #[should_panic(expected = "Expected module import")]
        fn non_import() {
            let symbol = TypeScriptSymbol::Symbol {
                symbol: daipendency_extractor::Symbol {
                    name: "foo".to_string(),
                    source_code: "foo".to_string(),
                },
                is_exported: false,
            };

            deconstruct_module_import(&symbol);
        }
    }

    mod namespace_deconstruction {
        use super::*;

        #[test]
        fn namespace() {
            let symbol = TypeScriptSymbol::Namespace {
                name: "Foo".to_string(),
                content: vec![TypeScriptSymbol::Symbol {
                    symbol: daipendency_extractor::Symbol {
                        name: "bar".to_string(),
                        source_code: "const bar = 42;".to_string(),
                    },
                    is_exported: false,
                }],
                is_exported: true,
                jsdoc: Some("/** Utility functions */".to_string()),
            };

            let (name, content, is_exported, jsdoc) = deconstruct_namespace(&symbol);

            assert_eq!(name, "Foo");
            assert_eq!(content.len(), 1);
            assert!(is_exported);
            assert_eq!(jsdoc, Some("/** Utility functions */".to_string()));
        }

        #[test]
        #[should_panic(expected = "Expected namespace")]
        fn non_namespace() {
            let symbol = TypeScriptSymbol::Symbol {
                symbol: daipendency_extractor::Symbol {
                    name: "foo".to_string(),
                    source_code: "foo".to_string(),
                },
                is_exported: false,
            };

            deconstruct_namespace(&symbol);
        }
    }

    mod module_export_deconstruction {
        use super::*;

        #[test]
        fn export_with_source() {
            let symbol = TypeScriptSymbol::ModuleExport {
                source_module: Some("lodash".to_string()),
                target: ExportTarget::Named {
                    names: vec!["map".to_string()],
                    aliases: HashMap::new(),
                },
            };

            let (source_module, target) = deconstruct_module_export(&symbol);

            assert_eq!(source_module, Some("lodash".to_string()));
            assert_matches!(target, ExportTarget::Named { names, aliases } if names == vec!["map".to_string()] && aliases.is_empty());
        }

        #[test]
        fn export_without_source() {
            let symbol = TypeScriptSymbol::ModuleExport {
                source_module: None,
                target: ExportTarget::Namespace {
                    name: "utils".to_string(),
                },
            };

            let (source_module, target) = deconstruct_module_export(&symbol);

            assert_eq!(source_module, None);
            assert_matches!(target, ExportTarget::Namespace { name } if name == "utils");
        }

        #[test]
        #[should_panic(expected = "Expected module export")]
        fn non_export() {
            let symbol = TypeScriptSymbol::Symbol {
                symbol: daipendency_extractor::Symbol {
                    name: "foo".to_string(),
                    source_code: "foo".to_string(),
                },
                is_exported: false,
            };

            deconstruct_module_export(&symbol);
        }
    }
}
