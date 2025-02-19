use daipendency_extractor::{ExtractionError, Symbol};
use std::collections::HashMap;
use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Node, Parser, Query, QueryCursor};

use crate::api::module::{ImportTarget, Module, TypeScriptSymbol};

// TODO: REMOVE WHEN ALL TESTS ARE FIXED
#[cfg(test)]
use daipendency_testing::debug_node;

const SYMBOLS_QUERY: &str = r#"
(class_declaration
    name: (type_identifier) @name
    ) @declaration

(abstract_class_declaration
    name: (type_identifier) @name
    ) @declaration

(interface_declaration
  name: (type_identifier) @name) @declaration

(function_signature
    name: (identifier) @name
    ) @declaration

(type_alias_declaration
  name: (type_identifier) @name) @declaration

(enum_declaration
  name: (identifier) @name) @declaration

(lexical_declaration
    (variable_declarator
        name: (identifier) @name
        )
    ) @declaration
"#;

const IMPORT_QUERY: &str = r#"
(import_statement
    (import_clause) @target
    source: (string
        (string_fragment) @source
        )
    )
"#;

pub fn parse_typescript_file(
    content: &str,
    parser: &mut Parser,
) -> Result<Module, ExtractionError> {
    let tree = parser
        .parse(content, None)
        .ok_or_else(|| ExtractionError::Malformed("Failed to parse source file".to_string()))?;
    let node = tree.root_node();

    if node.has_error() {
        return Err(ExtractionError::Malformed(
            "Failed to parse source file".to_string(),
        ));
    }

    // TODO: REMOVE WHEN ALL TESTS ARE FIXED
    #[cfg(test)]
    println!("root: {}", debug_node(&node, content));

    let jsdoc = get_jsdoc(node.child(0), content).filter(|s| is_module_jsdoc(s.as_str()));
    let (symbols, default_export_name) =
        get_module_symbols(node, content, &parser.language().unwrap());

    Ok(Module {
        jsdoc,
        symbols,
        default_export_name,
    })
}

fn get_jsdoc(node: Option<Node>, content: &str) -> Option<String> {
    node.filter(|n| n.kind() == "comment")
        .and_then(|n| n.utf8_text(content.as_bytes()).ok())
        .filter(|comment| comment.starts_with("/**"))
        .map(|comment| comment.to_string())
}

fn is_module_jsdoc(comment: &str) -> bool {
    comment.contains("@file") || comment.contains("@fileoverview") || comment.contains("@module")
}

/// Extracts all symbols from the module.
///
/// # Arguments
///
/// * `node` - The root node of the TypeScript AST
/// * `content` - The source code content as a string
/// * `language` - The language of the TypeScript AST
///
/// # Returns
///
/// A tuple containing:
/// * A vector of all symbols found in the module
/// * The name of the default export if one exists, otherwise None
fn get_module_symbols(
    root: Node,
    content: &str,
    language: &Language,
) -> (Vec<TypeScriptSymbol>, Option<String>) {
    let (mut symbols, default_export_name) = extract_symbols(root, content, language);

    symbols.extend(extract_imports(root, content, language));
    symbols.extend(extract_namespaces(root, content, language));

    (symbols, default_export_name)
}

fn extract_symbols<'a>(
    root: Node<'a>,
    content: &'a str,
    language: &Language,
) -> (Vec<TypeScriptSymbol>, Option<String>) {
    let mut symbols = vec![];
    let mut default_export_name = None;
    let query = Query::new(language, SYMBOLS_QUERY).expect("Failed to create query");

    let name_index = query
        .capture_index_for_name("name")
        .expect("Name capture not found");
    let definition_index = query
        .capture_index_for_name("declaration")
        .expect("Declaration capture not found");

    let mut cursor = QueryCursor::new();
    let matches = cursor.matches(&query, root, content.as_bytes());
    matches.for_each(|match_| {
        let name_node = match_.nodes_for_capture_index(name_index).next().unwrap();
        let mut definition_node = match_
            .nodes_for_capture_index(definition_index)
            .next()
            .unwrap();

        // Skip symbols that are inside a namespace
        if has_namespace_ancestor(definition_node, root) {
            return;
        }

        let name = name_node.utf8_text(content.as_bytes()).unwrap().to_string();

        if let Some(parent) = definition_node.parent() {
            if parent.kind() == "ambient_declaration" {
                definition_node = parent;
            }
        }

        let mut is_exported = false;
        if let Some(parent) = definition_node.parent() {
            if parent.kind() == "export_statement" {
                definition_node = parent;
                is_exported = true;

                let mut cursor = definition_node.walk();
                if definition_node
                    .children(&mut cursor)
                    .any(|child| child.kind() == "default")
                {
                    default_export_name = Some(name.clone());
                }
            }
        }

        // Get the full source code including any preceding JSDoc comment.
        let mut start_byte = definition_node.start_byte();
        let end_byte = definition_node.end_byte();
        if let Some(previous_node) = definition_node.prev_sibling() {
            if let Some(jsdoc) = get_jsdoc(Some(previous_node), content) {
                if !is_module_jsdoc(&jsdoc) {
                    start_byte = previous_node.start_byte();
                }
            }
        }

        let source_code = content[start_byte..end_byte].to_string();

        let symbol = Symbol { name, source_code };

        symbols.push(TypeScriptSymbol::Symbol {
            symbol,
            is_exported,
        });
    });

    (symbols, default_export_name)
}

fn has_namespace_ancestor(node: Node, root: Node) -> bool {
    if let Some(parent) = node.parent() {
        if parent.id() == root.id() {
            false
        } else if parent.kind() == "internal_module" {
            true
        } else {
            has_namespace_ancestor(parent, root)
        }
    } else {
        false
    }
}

fn extract_imports<'a>(
    root: Node<'a>,
    content: &'a str,
    language: &Language,
) -> Vec<TypeScriptSymbol> {
    let mut imports = vec![];
    let query = Query::new(language, IMPORT_QUERY).expect("Failed to create query");

    let target_index = query
        .capture_index_for_name("target")
        .expect("Target capture not found");
    let source_index = query
        .capture_index_for_name("source")
        .expect("Source capture not found");

    let mut cursor = QueryCursor::new();
    let matches = cursor.matches(&query, root, content.as_bytes());
    matches.for_each(|match_| {
        let source_node = match_.nodes_for_capture_index(source_index).next().unwrap();
        let source_module = source_node
            .utf8_text(content.as_bytes())
            .unwrap()
            .to_string();

        let target_node = match_.nodes_for_capture_index(target_index).next().unwrap();
        let mut target_cursor = target_node.walk();
        let subtarget_nodes = target_node.children(&mut target_cursor);

        let targets = subtarget_nodes.filter_map(|child| match child.kind() {
            "identifier" => Some(TypeScriptSymbol::ModuleImport {
                source_module: source_module.clone(),
                target: ImportTarget::Default {
                    name: extract_identifier_text(child, content).unwrap(),
                },
            }),
            "namespace_import" => {
                let mut namespace_cursor = child.walk();
                let name = child
                    .children(&mut namespace_cursor)
                    .find_map(|n| extract_identifier_text(n, content))
                    .unwrap();
                Some(TypeScriptSymbol::ModuleImport {
                    source_module: source_module.clone(),
                    target: ImportTarget::Namespace { name },
                })
            }
            "named_imports" => {
                let mut names = Vec::new();
                let mut aliases = HashMap::new();
                let mut named_cursor = child.walk();

                for import_specifier in child
                    .children(&mut named_cursor)
                    .filter(|n| n.kind() == "import_specifier")
                {
                    let mut specifier_cursor = import_specifier.walk();
                    let mut children = import_specifier.children(&mut specifier_cursor);

                    let name = children
                        .next()
                        .and_then(|n| extract_identifier_text(n, content))
                        .unwrap();
                    names.push(name.clone());

                    if let Some(alias) = children.find_map(|n| extract_identifier_text(n, content))
                    {
                        aliases.insert(name, alias);
                    }
                }

                Some(TypeScriptSymbol::ModuleImport {
                    source_module: source_module.clone(),
                    target: ImportTarget::Named { names, aliases },
                })
            }
            _ => None,
        });

        imports.extend(targets);
    });

    imports
}

fn extract_identifier_text(node: Node, content: &str) -> Option<String> {
    if node.kind() == "identifier" {
        node.utf8_text(content.as_bytes())
            .ok()
            .map(|s| s.to_string())
    } else {
        None
    }
}

fn extract_namespaces<'a>(
    root: Node<'a>,
    content: &'a str,
    language: &Language,
) -> Vec<TypeScriptSymbol> {
    let mut namespaces = vec![];
    let query = Query::new(
        language,
        r#"
        (internal_module
            name: (identifier) @name
            body: (statement_block) @body)
    "#,
    )
    .expect("Failed to create query");

    let name_index = query
        .capture_index_for_name("name")
        .expect("Name capture not found");
    let body_index = query
        .capture_index_for_name("body")
        .expect("Body capture not found");

    let mut cursor = QueryCursor::new();
    let matches = cursor.matches(&query, root, content.as_bytes());
    matches.for_each(|match_| {
        let name_node = match_.nodes_for_capture_index(name_index).next().unwrap();
        let namespace_node = name_node.parent().unwrap();

        if has_namespace_ancestor(namespace_node, root) {
            return;
        }

        let name = name_node.utf8_text(content.as_bytes()).unwrap().to_string();

        let body_node = match_.nodes_for_capture_index(body_index).next().unwrap();
        let (inner_content, _) = get_module_symbols(body_node, content, language);

        let mut is_exported = false;
        let mut current_node = namespace_node;
        if let Some(parent) = current_node.parent() {
            if parent.kind() == "export_statement" {
                is_exported = true;
                current_node = parent;
            }
        }

        let jsdoc = if let Some(expression_statement) = current_node.parent() {
            get_jsdoc(expression_statement.prev_sibling(), content)
        } else {
            None
        };

        namespaces.push(TypeScriptSymbol::Namespace {
            name,
            content: inner_content,
            is_exported,
            jsdoc,
        });
    });

    namespaces
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::test_helpers::make_parser;
    use assertables::assert_matches;
    use daipendency_extractor::ExtractionError;

    #[test]
    fn empty_file() {
        let mut parser = make_parser();

        let result = parse_typescript_file("", &mut parser);

        assert_matches!(result, Ok(Module { jsdoc: None, symbols: s, default_export_name: None }) if s.is_empty());
    }

    #[test]
    fn malformed_file() {
        let mut parser = make_parser();

        let result = parse_typescript_file("class {", &mut parser);

        assert_matches!(result, Err(ExtractionError::Malformed(msg)) if msg == "Failed to parse source file");
    }

    mod module_jsdoc {
        use super::*;

        const FILE_DESCRIPTION: &str = "Description of the file";

        #[test]
        fn file_tag() {
            let mut parser = make_parser();
            let content = format!("/** @file {FILE_DESCRIPTION} */\ndeclare const foo = 42;");

            let result = parse_typescript_file(&content, &mut parser);

            assert_matches!(result, Ok(Module { jsdoc: Some(j), .. }) if j == format!("/** @file {FILE_DESCRIPTION} */"));
        }

        #[test]
        fn fileoverview_tag() {
            let mut parser = make_parser();
            let content =
                format!("/** @fileoverview {FILE_DESCRIPTION} */\ndeclare const foo = 42;");

            let result = parse_typescript_file(&content, &mut parser);

            assert_matches!(result, Ok(Module { jsdoc: Some(j), .. }) if j == format!("/** @fileoverview {FILE_DESCRIPTION} */"));
        }

        #[test]
        fn module_tag() {
            let mut parser = make_parser();
            let content = format!("/** @module {FILE_DESCRIPTION} */\ndeclare const foo = 42;");

            let result = parse_typescript_file(&content, &mut parser);

            assert_matches!(result, Ok(Module { jsdoc: Some(j), .. }) if j == format!("/** @module {FILE_DESCRIPTION} */"));
        }

        #[test]
        fn no_tag() {
            let mut parser = make_parser();
            let content = "/** Just a comment */\ndeclare const foo = 42;";

            let result = parse_typescript_file(content, &mut parser);

            assert_matches!(result, Ok(Module { jsdoc: None, .. }));
        }

        #[test]
        fn non_jsdoc_block_comment() {
            let mut parser = make_parser();
            let content = "/* @module Just a comment */\ndeclare const foo = 42;";

            let result = parse_typescript_file(content, &mut parser);

            assert_matches!(result, Ok(Module { jsdoc: None, .. }));
        }

        #[test]
        fn line_comment() {
            let mut parser = make_parser();
            let content = "// @module Just a comment\ndeclare const foo = 42;";

            let result = parse_typescript_file(content, &mut parser);

            assert_matches!(result, Ok(Module { jsdoc: None, .. }));
        }
    }

    mod symbols {
        use super::*;

        #[test]
        fn class_declaration() {
            let mut parser = make_parser();
            let content = "declare class Foo { bar(): void; }";

            let module = parse_typescript_file(content, &mut parser).unwrap();

            assert_matches!(module, Module { ref symbols, .. } if symbols.len() == 1);
            let symbol = &module.symbols[0];
            assert_matches!(symbol, TypeScriptSymbol::Symbol { symbol, is_exported: false } if symbol.name == "Foo" && symbol.source_code == content);
        }

        #[test]
        fn abstract_class_declaration() {
            let mut parser = make_parser();
            let content = "declare abstract class Foo { bar(): void; }";

            let module = parse_typescript_file(content, &mut parser).unwrap();

            assert_matches!(module, Module { ref symbols, .. } if symbols.len() == 1);
            let symbol = &module.symbols[0];
            assert_matches!(symbol, TypeScriptSymbol::Symbol { symbol, is_exported: false } if symbol.name == "Foo" && symbol.source_code == content);
        }

        #[test]
        fn type_alias_declaration() {
            let mut parser = make_parser();
            let content = "type Bar = string;";

            let module = parse_typescript_file(content, &mut parser).unwrap();

            assert_matches!(module, Module { ref symbols, .. } if symbols.len() == 1);
            let symbol = &module.symbols[0];
            assert_matches!(symbol, TypeScriptSymbol::Symbol { symbol, is_exported: false } if symbol.name == "Bar" && symbol.source_code == content);
        }

        #[test]
        fn interface_declaration() {
            let mut parser = make_parser();
            let content = "interface Baz { qux: number; }";

            let module = parse_typescript_file(content, &mut parser).unwrap();

            assert_matches!(module, Module { ref symbols, .. } if symbols.len() == 1);
            let symbol = &module.symbols[0];
            assert_matches!(symbol, TypeScriptSymbol::Symbol { symbol, is_exported: false } if symbol.name == "Baz" && symbol.source_code == content);
        }

        #[test]
        fn enum_declaration() {
            let mut parser = make_parser();
            let content = "enum Status { Active, Inactive }";

            let module = parse_typescript_file(content, &mut parser).unwrap();

            assert_matches!(module, Module { ref symbols, .. } if symbols.len() == 1);
            let symbol = &module.symbols[0];
            assert_matches!(symbol, TypeScriptSymbol::Symbol { symbol, is_exported: false } if symbol.name == "Status" && symbol.source_code == content);
        }

        #[test]
        fn function_declaration() {
            let mut parser = make_parser();
            let content = "declare function greet(name: string): void;";

            let module = parse_typescript_file(content, &mut parser).unwrap();

            assert_matches!(module, Module { ref symbols, .. } if symbols.len() == 1);
            let symbol = &module.symbols[0];
            assert_matches!(symbol, TypeScriptSymbol::Symbol { symbol, is_exported: false } if symbol.name == "greet" && symbol.source_code == content);
        }

        #[test]
        fn const_declaration() {
            let mut parser = make_parser();
            let content = "declare const VERSION: string;";

            let module = parse_typescript_file(content, &mut parser).unwrap();

            assert_matches!(module, Module { ref symbols, .. } if symbols.len() == 1);
            let symbol = &module.symbols[0];
            assert_matches!(symbol, TypeScriptSymbol::Symbol { symbol, is_exported: false } if symbol.name == "VERSION" && symbol.source_code == content);
        }

        #[test]
        fn let_declaration() {
            let mut parser = make_parser();
            let content = "declare let counter: number;";

            let module = parse_typescript_file(content, &mut parser).unwrap();

            assert_matches!(module, Module { ref symbols, .. } if symbols.len() == 1);
            let symbol = &module.symbols[0];
            assert_matches!(symbol, TypeScriptSymbol::Symbol { symbol, is_exported: false } if symbol.name == "counter" && symbol.source_code == content);
        }

        #[test]
        fn symbol_with_jsdoc() {
            let mut parser = make_parser();
            let content = "/** The version number */\ndeclare const VERSION: string;";

            let module = parse_typescript_file(content, &mut parser).unwrap();

            assert_matches!(module, Module { ref symbols, .. } if symbols.len() == 1);
            let symbol = &module.symbols[0];
            assert_matches!(symbol, TypeScriptSymbol::Symbol { symbol, is_exported: false } if symbol.name == "VERSION" && symbol.source_code == content);
        }

        #[test]
        fn symbol_without_jsdoc() {
            let mut parser = make_parser();
            let content = "declare const VERSION: string;";

            let module = parse_typescript_file(content, &mut parser).unwrap();

            assert_matches!(module, Module { ref symbols, .. } if symbols.len() == 1);
            let symbol = &module.symbols[0];
            assert_matches!(symbol, TypeScriptSymbol::Symbol { symbol, is_exported: false } if symbol.name == "VERSION" && symbol.source_code == content);
        }

        #[test]
        fn symbol_with_preceding_module_jsdoc_comment() {
            let mut parser = make_parser();
            let content = "/** @module The module description */\ndeclare const VERSION: string;";

            let module = parse_typescript_file(content, &mut parser).unwrap();

            assert_matches!(module, Module { ref symbols, .. } if symbols.len() == 1);
            let symbol = &module.symbols[0];
            assert_matches!(symbol, TypeScriptSymbol::Symbol { symbol, is_exported: false } if symbol.source_code == "declare const VERSION: string;".to_string());
        }

        #[test]
        fn symbol_with_preceding_non_jsdoc_comment() {
            let mut parser = make_parser();
            let content = "// The comment\ndeclare const VERSION: string;";

            let module = parse_typescript_file(content, &mut parser).unwrap();

            assert_matches!(module, Module { ref symbols, .. } if symbols.len() == 1);
            let symbol = &module.symbols[0];
            assert_matches!(symbol, TypeScriptSymbol::Symbol { symbol, is_exported: false } if symbol.source_code == "declare const VERSION: string;".to_string());
        }
    }

    mod symbol_exports {
        use super::*;

        #[test]
        fn exported_afterwards() {
            let mut parser = make_parser();
            let content = "declare const VERSION: string;\nexport { VERSION };";

            let result = parse_typescript_file(content, &mut parser);

            assert_matches!(result, Ok(Module { symbols: s, .. }) if s.len() == 1 && matches!(&s[0], TypeScriptSymbol::Symbol { symbol, is_exported: true } if symbol.name == "VERSION" && symbol.source_code.contains("declare const VERSION: string")));
        }

        #[test]
        fn export_and_declaration() {
            let mut parser = make_parser();
            let content = "export declare function greet(name: string): void;";

            let module = parse_typescript_file(content, &mut parser).unwrap();

            assert_matches!(module, Module { ref symbols, .. } if symbols.len() == 1);
            let symbol = &module.symbols[0];
            assert_matches!(symbol, TypeScriptSymbol::Symbol { symbol, is_exported: true } if symbol.name == "greet" && symbol.source_code == content);
        }

        #[test]
        fn default_export_and_declaration() {
            let mut parser = make_parser();
            let content = "export default declare function greet(name: string): void;";

            let module = parse_typescript_file(content, &mut parser).unwrap();

            assert_matches!(&module, Module { symbols, default_export_name: Some(n), .. } if symbols.len() == 1 && n == "greet");
            let symbol = &module.symbols[0];
            assert_matches!(symbol, TypeScriptSymbol::Symbol { symbol, is_exported: true } if symbol.name == "greet" && symbol.source_code == content);
        }

        #[test]
        fn default_export_afterwards() {
            let mut parser = make_parser();
            let content = "declare const VERSION: string;\nexport default VERSION;";

            let module = parse_typescript_file(content, &mut parser).unwrap();

            assert_matches!(&module, Module { symbols, default_export_name: Some(n), .. } if symbols.len() == 1 && n == "VERSION");
            let symbol = &module.symbols[0];
            assert_matches!(symbol, TypeScriptSymbol::Symbol { symbol, is_exported: true } if symbol.name == "VERSION" && symbol.source_code.contains("declare const VERSION: string"));
        }

        #[test]
        fn commonjs_export() {
            let mut parser = make_parser();
            let content = "declare function myFunction(): void;\nexport = myFunction;";

            let result = parse_typescript_file(content, &mut parser);

            assert_matches!(result, Ok(Module { symbols: s, .. }) if s.len() == 1 && matches!(&s[0], TypeScriptSymbol::Symbol { symbol, is_exported: true } if symbol.name == "myFunction" && symbol.source_code.contains("declare function myFunction(): void")));
        }
    }

    mod namespaces {
        use super::*;

        #[test]
        fn empty_namespace() {
            let mut parser = make_parser();
            let content = "namespace Foo {}";

            let module = parse_typescript_file(content, &mut parser).unwrap();

            assert_eq!(module.symbols.len(), 1);
            let namespace = &module.symbols[0];
            assert_matches!(namespace, TypeScriptSymbol::Namespace { name, .. } if name == "Foo");
            assert_matches!(namespace, TypeScriptSymbol::Namespace { content, .. } if content.is_empty());
        }

        #[test]
        fn namespace_with_symbol() {
            let mut parser = make_parser();
            let content = "namespace Foo { declare const VERSION: string; }";

            let module = parse_typescript_file(content, &mut parser).unwrap();

            assert_eq!(module.symbols.len(), 1);
            let namespace = &module.symbols[0];
            assert_matches!(namespace, TypeScriptSymbol::Namespace { name, .. } if name == "Foo");
            assert_matches!(namespace, TypeScriptSymbol::Namespace { content, .. } if content.len() == 1);
        }

        #[test]
        fn exported_namespace() {
            let mut parser = make_parser();
            let content = "export namespace Foo { declare const VERSION: string; }";

            let module = parse_typescript_file(content, &mut parser).unwrap();

            assert_eq!(module.symbols.len(), 1);
            let namespace = &module.symbols[0];
            assert_matches!(namespace, TypeScriptSymbol::Namespace { name, .. } if name == "Foo");
            assert_matches!(
                namespace,
                TypeScriptSymbol::Namespace {
                    is_exported: true,
                    ..
                }
            );
        }

        #[test]
        fn namespace_with_multiple_symbols() {
            let mut parser = make_parser();
            let content =
                "namespace Foo { declare const VERSION: string; declare function greet(): void; }";

            let module = parse_typescript_file(content, &mut parser).unwrap();

            assert_eq!(module.symbols.len(), 1);
            let namespace = &module.symbols[0];
            assert_matches!(namespace, TypeScriptSymbol::Namespace { name, .. } if name == "Foo");
            assert_matches!(namespace, TypeScriptSymbol::Namespace { content, .. } if content.len() == 2);
        }

        #[test]
        fn namespace_with_inner_namespace() {
            let mut parser = make_parser();
            let content =
                "namespace Foo { namespace Bar { export declare const VERSION: string; } }";

            let module = parse_typescript_file(content, &mut parser).unwrap();

            assert_eq!(module.symbols.len(), 1);
            let outer_namespace = &module.symbols[0];
            assert_matches!(outer_namespace, TypeScriptSymbol::Namespace { name, .. } if name == "Foo");
            let inner_namespace = match &outer_namespace {
                TypeScriptSymbol::Namespace { content, .. } if content.len() == 1 => &content[0],
                _ => panic!("Expected namespace"),
            };
            assert_matches!(inner_namespace, TypeScriptSymbol::Namespace { name, .. } if name == "Bar");
            let symbol = match &inner_namespace {
                TypeScriptSymbol::Namespace { content, .. } if content.len() == 1 => &content[0],
                _ => panic!("Expected symbol"),
            };
            assert_matches!(symbol, TypeScriptSymbol::Symbol { symbol, is_exported: true } if symbol.name == "VERSION");
        }

        #[test]
        fn namespace_with_jsdoc() {
            let mut parser = make_parser();
            let content =
                "/** Utility functions */\nnamespace Foo { declare const VERSION: string; }";

            let module = parse_typescript_file(content, &mut parser).unwrap();

            assert_eq!(module.symbols.len(), 1);
            let namespace = &module.symbols[0];
            assert_matches!(namespace, TypeScriptSymbol::Namespace { name, .. } if name == "Foo");
            assert_matches!(namespace, TypeScriptSymbol::Namespace { jsdoc: Some(j), .. } if j == "/** Utility functions */");
        }

        #[test]
        fn namespace_without_jsdoc() {
            let mut parser = make_parser();
            let content = "namespace Foo { declare const VERSION: string; }";

            let module = parse_typescript_file(content, &mut parser).unwrap();

            assert_eq!(module.symbols.len(), 1);
            let namespace = &module.symbols[0];
            assert_matches!(namespace, TypeScriptSymbol::Namespace { jsdoc: None, .. });
        }
    }

    mod module_imports {
        use super::*;

        #[test]
        fn default_import() {
            let mut parser = make_parser();
            let content = "import foo from './foo.js';";

            let result = parse_typescript_file(content, &mut parser);

            assert_matches!(result, Ok(Module { symbols: s, .. }) if s.len() == 1 && matches!(&s[0], TypeScriptSymbol::ModuleImport { source_module, target } if source_module == "./foo.js" && matches!(target, ImportTarget::Default { name } if name == "foo")));
        }

        #[test]
        fn namespace_import() {
            let mut parser = make_parser();
            let content = "import * as foo from './foo.js';";

            let result = parse_typescript_file(content, &mut parser);

            assert_matches!(result, Ok(Module { symbols: s, .. }) if s.len() == 1 && matches!(&s[0], TypeScriptSymbol::ModuleImport { source_module, target } if source_module == "./foo.js" && matches!(target, ImportTarget::Namespace { name } if name == "foo")));
        }

        #[test]
        fn named_import() {
            let mut parser = make_parser();
            let content = "import { foo } from './foo.js';";
            let module = parse_typescript_file(content, &mut parser).unwrap();

            assert_matches!(&module, Module { symbols, .. } if symbols.len() == 1);

            let named_import = &module.symbols[0];
            assert_matches!(named_import, TypeScriptSymbol::ModuleImport { source_module, .. } if source_module == "./foo.js");
            assert_matches!(named_import, TypeScriptSymbol::ModuleImport { target, .. } if matches!(target, ImportTarget::Named { names, aliases } if *names == vec!["foo".to_string()] && aliases.is_empty()));
        }

        #[test]
        fn named_import_with_alias() {
            let mut parser = make_parser();
            let content = "import { foo as bar } from './foo.js';";
            let module = parse_typescript_file(content, &mut parser).unwrap();

            assert_matches!(&module, Module { symbols, .. } if symbols.len() == 1);

            let named_import = &module.symbols[0];
            assert_matches!(named_import, TypeScriptSymbol::ModuleImport { source_module, .. } if source_module == "./foo.js");
            assert_matches!(named_import, TypeScriptSymbol::ModuleImport { target, .. } if matches!(target, ImportTarget::Named { names, aliases } if *names == vec!["foo".to_string()] && aliases == &HashMap::from([("foo".to_string(), "bar".to_string())])));
        }

        #[test]
        fn mixed_import() {
            let mut parser = make_parser();
            let content = "import foo, { bar } from './foo.js';";

            let module = parse_typescript_file(content, &mut parser).unwrap();

            assert_matches!(&module, Module { symbols, .. } if symbols.len() == 2);

            let default_import = &module.symbols[0];
            assert_matches!(default_import, TypeScriptSymbol::ModuleImport { source_module, .. } if source_module == "./foo.js");
            assert_matches!(default_import, TypeScriptSymbol::ModuleImport { target, .. } if matches!(target, ImportTarget::Default { name } if name == "foo"));
            let named_import = &module.symbols[1];
            assert_matches!(named_import, TypeScriptSymbol::ModuleImport { source_module, .. } if source_module == "./foo.js");
            assert_matches!(named_import, TypeScriptSymbol::ModuleImport { target, .. } if matches!(target, ImportTarget::Named { names, aliases } if *names == vec!["bar".to_string()] && aliases.is_empty()));
        }

        #[test]
        fn multiple_named_imports() {
            let mut parser = make_parser();
            let content = "import { foo, bar as baz } from './foo.js';";

            let module = parse_typescript_file(content, &mut parser).unwrap();

            assert_matches!(&module, Module { symbols, .. } if symbols.len() == 1);

            let named_import = &module.symbols[0];
            assert_matches!(named_import, TypeScriptSymbol::ModuleImport { source_module, .. } if source_module == "./foo.js");
            assert_matches!(named_import, TypeScriptSymbol::ModuleImport { target, .. } if matches!(target, ImportTarget::Named { names, aliases } if *names == vec!["foo".to_string(), "bar".to_string()] && aliases == &HashMap::from([("bar".to_string(), "baz".to_string())])));
        }
    }

    mod module_exports {
        use crate::api::module::ExportTarget;

        use super::*;

        #[test]
        fn namespace_export() {
            let mut parser = make_parser();
            let content = "export * as foo from './foo.js';";

            let result = parse_typescript_file(content, &mut parser);

            assert_matches!(result, Ok(Module { symbols: s, .. }) if s.len() == 1 && matches!(&s[0], TypeScriptSymbol::ModuleExport { source_module, target } if source_module == "./foo.js" && matches!(target, ExportTarget::Namespace { name } if name == "foo")));
        }

        #[test]
        fn named_export() {
            let mut parser = make_parser();
            let content = "export { foo } from './foo.js';";

            let result = parse_typescript_file(content, &mut parser);

            assert_matches!(result, Ok(Module { symbols: s, .. }) if s.len() == 1 && matches!(&s[0], TypeScriptSymbol::ModuleExport { source_module, target } if source_module == "./foo.js" && matches!(target, ExportTarget::Named { names, aliases } if *names == vec!["foo".to_string()] && aliases.is_empty())));
        }

        #[test]
        fn named_with_alias() {
            let mut parser = make_parser();
            let content = "export { foo as bar } from './foo.js';";

            let module = parse_typescript_file(content, &mut parser).unwrap();

            assert_matches!(&module, Module { symbols, .. } if symbols.len() == 1);
            let symbol = &module.symbols[0];
            assert_matches!(symbol, TypeScriptSymbol::ModuleExport { source_module, .. } if source_module == "./foo.js");
            assert_matches!(symbol, TypeScriptSymbol::ModuleExport { target, .. } if matches!(target, ExportTarget::Named { names, aliases } if *names == vec!["foo".to_string()] && aliases == &HashMap::from([("foo".to_string(), "bar".to_string())])));
        }

        #[test]
        fn barrel_export() {
            let mut parser = make_parser();
            let content = "export * from './foo.js';";

            let result = parse_typescript_file(content, &mut parser);

            assert_matches!(result, Ok(Module { symbols: s, .. }) if s.len() == 1 && matches!(&s[0], TypeScriptSymbol::ModuleExport { source_module, target } if source_module == "./foo.js" && matches!(target, ExportTarget::Barrel)));
        }
    }
}
