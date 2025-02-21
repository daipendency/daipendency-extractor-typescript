mod module;
mod parsing;
#[cfg(test)]
mod test_helpers;

use daipendency_extractor::{ExtractionError, Namespace, Symbol};
use tree_sitter::{Node, Parser};

use crate::metadata::TSLibraryMetadata;

pub fn extract_public_api(
    library_metadata: &TSLibraryMetadata,
    parser: &mut Parser,
) -> Result<Vec<Namespace>, ExtractionError> {
    let source_code =
        std::fs::read_to_string(&library_metadata.entry_point).map_err(ExtractionError::Io)?;

    let tree = parser
        .parse(&source_code, None)
        .ok_or_else(|| ExtractionError::Malformed("Failed to parse source".to_string()))?;

    let mut namespaces = vec![Namespace {
        name: library_metadata.name.clone(),
        symbols: Vec::new(),
        doc_comment: None,
    }];

    process_node(tree.root_node(), &source_code, &mut namespaces)?;

    Ok(namespaces)
}

fn process_node(
    node: Node,
    source_code: &str,
    namespaces: &mut Vec<Namespace>,
) -> Result<(), ExtractionError> {
    if node.kind() == "export_statement" {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "enum_declaration"
                | "interface_declaration"
                | "class_declaration"
                | "function_declaration"
                | "type_alias_declaration" => {
                    let name = get_declaration_name(&child, source_code).ok_or_else(|| {
                        ExtractionError::Malformed("Declaration without name".to_string())
                    })?;
                    namespaces[0].symbols.push(Symbol {
                        name,
                        source_code: get_node_text(node, source_code),
                    });
                }
                "lexical_declaration" => {
                    let mut var_cursor = child.walk();
                    for var_child in child.children(&mut var_cursor) {
                        if var_child.kind() == "variable_declarator" {
                            let name =
                                get_declaration_name(&var_child, source_code).ok_or_else(|| {
                                    ExtractionError::Malformed("Variable without name".to_string())
                                })?;
                            namespaces[0].symbols.push(Symbol {
                                name,
                                source_code: get_node_text(node, source_code),
                            });
                        }
                    }
                }
                "internal_module" => {
                    let name = get_declaration_name(&child, source_code).ok_or_else(|| {
                        ExtractionError::Malformed("Namespace without name".to_string())
                    })?;
                    namespaces.push(Namespace {
                        name,
                        symbols: Vec::new(),
                        doc_comment: None,
                    });
                }
                _ => {}
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        process_node(child, source_code, namespaces)?;
    }

    Ok(())
}

fn get_declaration_name(node: &Node, source_code: &str) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "identifier" | "type_identifier" => {
                return Some(get_node_text(child, source_code));
            }
            _ => {}
        }
    }
    None
}

fn get_node_text(node: Node, source_code: &str) -> String {
    source_code[node.start_byte()..node.end_byte()].to_string()
}

#[cfg(test)]
mod tests {
    use super::test_helpers::make_parser;
    use super::*;
    use daipendency_testing::{debug_node, tempdir::TempDir};

    fn setup_test_dir(content: &str) -> (TempDir, TSLibraryMetadata) {
        let temp_dir = TempDir::new();
        temp_dir
            .create_file(
                "package.json",
                r#"{"name": "test-pkg", "version": "1.0.0", "types": "index.d.ts"}"#,
            )
            .unwrap();
        temp_dir.create_file("index.d.ts", content).unwrap();

        let library_metadata = TSLibraryMetadata {
            name: "test-pkg".to_string(),
            version: Some("1.0.0".to_string()),
            documentation: String::new(),
            entry_point: temp_dir.path.join("index.d.ts"),
        };

        (temp_dir, library_metadata)
    }

    #[test]
    fn exported_interface() {
        let (_temp_dir, library_metadata) =
            setup_test_dir("export interface Person { name: string; age: number; }");
        let mut parser = make_parser();

        let tree = parser
            .parse(
                "export interface Person { name: string; age: number; }",
                None,
            )
            .unwrap();
        println!(
            "Node structure:\n{}",
            debug_node(
                &tree.root_node(),
                "export interface Person { name: string; age: number; }"
            )
        );

        let namespaces = extract_public_api(&library_metadata, &mut parser).unwrap();

        assert_eq!(namespaces.len(), 1);
        assert_eq!(namespaces[0].name, "test-pkg");
        assert_eq!(namespaces[0].symbols.len(), 1);
        assert_eq!(namespaces[0].symbols[0].name, "Person");
        assert_eq!(
            namespaces[0].symbols[0].source_code,
            "export interface Person { name: string; age: number; }"
        );
    }

    #[test]
    fn exported_enum() {
        let (_temp_dir, library_metadata) =
            setup_test_dir("export enum Status { Active = 'active', Inactive = 'inactive' }");
        let mut parser = make_parser();

        let namespaces = extract_public_api(&library_metadata, &mut parser).unwrap();

        assert_eq!(namespaces.len(), 1);
        assert_eq!(namespaces[0].symbols.len(), 1);
        assert_eq!(namespaces[0].symbols[0].name, "Status");
    }

    #[test]
    fn exported_class() {
        let (_temp_dir, library_metadata) =
            setup_test_dir("export class User { constructor(public name: string) {} }");
        let mut parser = make_parser();

        let namespaces = extract_public_api(&library_metadata, &mut parser).unwrap();

        assert_eq!(namespaces.len(), 1);
        assert_eq!(namespaces[0].symbols.len(), 1);
        assert_eq!(namespaces[0].symbols[0].name, "User");
    }

    #[test]
    fn exported_function() {
        let (_temp_dir, library_metadata) = setup_test_dir(
            "export function greet(name: string): string { return `Hello ${name}`; }",
        );
        let mut parser = make_parser();

        let namespaces = extract_public_api(&library_metadata, &mut parser).unwrap();

        assert_eq!(namespaces.len(), 1);
        assert_eq!(namespaces[0].symbols.len(), 1);
        assert_eq!(namespaces[0].symbols[0].name, "greet");
    }

    #[test]
    fn exported_type_alias() {
        let (_temp_dir, library_metadata) = setup_test_dir("export type UserId = string;");
        let mut parser = make_parser();

        let namespaces = extract_public_api(&library_metadata, &mut parser).unwrap();

        assert_eq!(namespaces.len(), 1);
        assert_eq!(namespaces[0].symbols.len(), 1);
        assert_eq!(namespaces[0].symbols[0].name, "UserId");
    }

    #[test]
    fn exported_namespace() {
        let (_temp_dir, library_metadata) =
            setup_test_dir("export namespace Utils { export function helper(): void {} }");
        let mut parser = make_parser();

        let tree = parser
            .parse(
                "export namespace Utils { export function helper(): void {} }",
                None,
            )
            .unwrap();
        println!(
            "Namespace node structure:\n{}",
            debug_node(
                &tree.root_node(),
                "export namespace Utils { export function helper(): void {} }"
            )
        );

        let namespaces = extract_public_api(&library_metadata, &mut parser).unwrap();

        assert_eq!(namespaces.len(), 2);
        assert_eq!(namespaces[1].name, "Utils");
    }

    #[test]
    fn exported_variable() {
        let (_temp_dir, library_metadata) =
            setup_test_dir("export const VERSION: string = '1.0.0';");
        let mut parser = make_parser();

        let tree = parser
            .parse("export const VERSION: string = '1.0.0';", None)
            .unwrap();
        println!(
            "Variable node structure:\n{}",
            debug_node(&tree.root_node(), "export const VERSION: string = '1.0.0';")
        );

        let namespaces = extract_public_api(&library_metadata, &mut parser).unwrap();

        assert_eq!(namespaces.len(), 1);
        assert_eq!(namespaces[0].symbols.len(), 1);
        assert_eq!(namespaces[0].symbols[0].name, "VERSION");
    }
}
