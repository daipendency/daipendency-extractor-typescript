use daipendency_extractor::{ExtractionError, ParsedFile, Symbol};
use std::collections::HashMap;
use std::path::PathBuf;
use streaming_iterator::StreamingIterator;
use tree_sitter::{Node, Parser, QueryCursor};

use crate::api::module::{ExportTarget, ImportTarget, Module, TypeScriptSymbol};

const DEFAULT_EXPORT_QUERY: &str = r#"
; Default export
(export_statement
  "default"
  value: (identifier) @name
  )

; Export and declaration
(export_statement
  "default"
  (ambient_declaration
    "declare"
    (_
      name: (_) @name
      )
    )
  )
"#;

const SYMBOLS_QUERY: &str = r#"
(class_declaration
    name: (type_identifier) @name
    ) @declaration

(abstract_class_declaration
    name: (type_identifier) @name
    ) @declaration

(interface_declaration
    name: (type_identifier) @name
    ) @declaration

(function_signature
    name: (identifier) @name
    ) @declaration

(type_alias_declaration
    name: (type_identifier) @name
    ) @declaration

(enum_declaration
    name: (identifier) @name
    ) @declaration

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

const EXPORTS_QUERY: &str = r#"
; Named exports, with or without source
(export_statement
  (export_clause
    (export_specifier
      name: (identifier) @name
      alias: (identifier)? @alias
      )
    )
  source: (
    string (
      (string_fragment) @source)
    )?
  )

; CommonJS
(export_statement
  "="
  (identifier) @name
  )

; Namespace export
(export_statement
  (namespace_export
    "*"
    "as"
    (identifier) @name
    )
  source: (string (string_fragment) @source)
  )

; Barrel export
(export_statement
  "*"
  source: (string (string_fragment) @source)
  ) @barrel_export
"#;

pub fn parse_typescript_file(
    content: &str,
    parser: &mut Parser,
    file_path: PathBuf,
) -> Result<Module, ExtractionError> {
    let parsed_file = ParsedFile::parse(content, parser)?;
    let root_node = parsed_file.root_node();

    let jsdoc = get_jsdoc(root_node.child(0), &parsed_file).filter(|s| is_module_jsdoc(s.as_str()));
    let symbols = get_module_symbols(root_node, &parsed_file)?;
    let default_export_name = extract_default_export_name(root_node, &parsed_file)?;

    Ok(Module {
        path: file_path,
        jsdoc,
        symbols,
        default_export_name,
    })
}

fn get_jsdoc<'a>(node: Option<Node<'a>>, parsed_file: &'a ParsedFile) -> Option<String> {
    node.filter(|n| n.kind() == "comment")
        .and_then(|n| parsed_file.render_node(n).ok())
        .filter(|comment| comment.starts_with("/**"))
}

fn is_module_jsdoc(comment: &str) -> bool {
    comment.contains("@file") || comment.contains("@fileoverview") || comment.contains("@module")
}

/// Extracts all symbols from the module.
///
/// # Arguments
///
/// * `node` - The root node of the TypeScript AST
/// * `parsed_file` - The parsed file containing the source code
///
/// # Returns
///
/// A vector of all symbols found in the module
fn get_module_symbols<'a>(
    node: Node<'a>,
    parsed_file: &'a ParsedFile,
) -> Result<Vec<TypeScriptSymbol>, ExtractionError> {
    let mut symbols = vec![];

    symbols.extend(extract_imports(node, parsed_file)?);
    symbols.extend(extract_symbols(node, parsed_file)?);
    symbols.extend(extract_namespaces(node, parsed_file)?);
    symbols.extend(extract_exports(node, parsed_file)?);

    Ok(symbols)
}

fn extract_default_export_name<'a>(
    root: Node<'a>,
    parsed_file: &'a ParsedFile,
) -> Result<Option<String>, ExtractionError> {
    let query = parsed_file.make_query(DEFAULT_EXPORT_QUERY)?;

    let name_index = query
        .capture_index_for_name("name")
        .expect("Name capture not found");
    let mut cursor = QueryCursor::new();
    let mut matches = parsed_file.exec_query(&query, root, &mut cursor);

    Ok(matches.next().and_then(|match_| {
        match_
            .nodes_for_capture_index(name_index)
            .next()
            .and_then(|node| parsed_file.render_node(node).ok())
    }))
}

fn extract_symbols<'a>(
    root: Node<'a>,
    parsed_file: &'a ParsedFile,
) -> Result<Vec<TypeScriptSymbol>, ExtractionError> {
    let mut symbols = vec![];
    let query = parsed_file.make_query(SYMBOLS_QUERY)?;

    let name_index = query
        .capture_index_for_name("name")
        .expect("Name capture not found");
    let definition_index = query
        .capture_index_for_name("declaration")
        .expect("Declaration capture not found");

    let mut cursor = QueryCursor::new();
    let mut matches = parsed_file.exec_query(&query, root, &mut cursor);

    while let Some(match_) = matches.next() {
        let name_node = match_
            .nodes_for_capture_index(name_index)
            .next()
            .expect("Missing name node in symbol declaration");
        let mut definition_node = match_
            .nodes_for_capture_index(definition_index)
            .next()
            .expect("Missing declaration node in symbol declaration");

        // Skip symbols that are inside a namespace
        if has_namespace_ancestor(definition_node, root) {
            continue;
        }

        let name = parsed_file.render_node(name_node)?;

        let parent = definition_node
            .parent()
            .expect("Symbol declaration has no parent");
        if parent.kind() == "ambient_declaration" {
            definition_node = parent;
        }

        let mut is_exported = false;
        let parent = definition_node
            .parent()
            .expect("Symbol declaration has no parent");
        if parent.kind() == "export_statement" {
            definition_node = parent;
            is_exported = true;
        }

        // Get the full source code including any preceding JSDoc comment.
        let mut start_byte = definition_node.start_byte();
        let end_byte = definition_node.end_byte();
        if let Some(previous_node) = definition_node.prev_sibling() {
            if let Some(jsdoc) = get_jsdoc(Some(previous_node), parsed_file) {
                if !is_module_jsdoc(&jsdoc) {
                    start_byte = previous_node.start_byte();
                }
            }
        }

        let source_code = parsed_file.render(start_byte..end_byte);

        let symbol = Symbol { name, source_code };

        symbols.push(TypeScriptSymbol::Symbol {
            symbol,
            is_exported,
        });
    }

    Ok(symbols)
}

fn has_namespace_ancestor(node: Node, root: Node) -> bool {
    let parent = node.parent().expect("Node has no parent");
    if parent.id() == root.id() {
        false
    } else if parent.kind() == "internal_module" {
        true
    } else {
        has_namespace_ancestor(parent, root)
    }
}

fn extract_imports<'a>(
    root: Node<'a>,
    parsed_file: &'a ParsedFile,
) -> Result<Vec<TypeScriptSymbol>, ExtractionError> {
    let mut imports = vec![];
    let query = parsed_file.make_query(IMPORT_QUERY)?;

    let target_index = query
        .capture_index_for_name("target")
        .expect("Target capture not found");
    let source_index = query
        .capture_index_for_name("source")
        .expect("Source capture not found");

    let mut cursor = QueryCursor::new();
    let mut matches = parsed_file.exec_query(&query, root, &mut cursor);

    while let Some(match_) = matches.next() {
        let source_node = match_
            .nodes_for_capture_index(source_index)
            .next()
            .expect("Missing source node in import");
        let source_module = parsed_file.render_node(source_node)?;

        let target_node = match_
            .nodes_for_capture_index(target_index)
            .next()
            .expect("Missing target node in import");
        let mut target_cursor = target_node.walk();
        let subtarget_nodes = target_node.children(&mut target_cursor);

        let targets = subtarget_nodes.filter_map(|child| match child.kind() {
            "identifier" => Some(TypeScriptSymbol::ModuleImport {
                source_module: source_module.clone(),
                target: ImportTarget::Default {
                    name: extract_identifier_text(child, parsed_file)
                        .expect("Failed to get import identifier"),
                },
            }),
            "namespace_import" => {
                let mut namespace_cursor = child.walk();
                let name = child
                    .children(&mut namespace_cursor)
                    .find_map(|n| extract_identifier_text(n, parsed_file))
                    .expect("Failed to get import identifier");
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
                        .and_then(|n| extract_identifier_text(n, parsed_file))
                        .expect("Failed to get import identifier");
                    names.push(name.clone());

                    if let Some(alias) =
                        children.find_map(|n| extract_identifier_text(n, parsed_file))
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
    }

    Ok(imports)
}

fn extract_identifier_text(node: Node, parsed_file: &ParsedFile) -> Option<String> {
    if node.kind() == "identifier" {
        parsed_file.render_node(node).ok()
    } else {
        None
    }
}

fn extract_namespaces<'a>(
    root: Node<'a>,
    parsed_file: &'a ParsedFile,
) -> Result<Vec<TypeScriptSymbol>, ExtractionError> {
    let mut namespaces = vec![];
    let query = parsed_file.make_query(
        r#"
        (internal_module
            name: (identifier) @name
            body: (statement_block) @body)
    "#,
    )?;

    let name_index = query
        .capture_index_for_name("name")
        .expect("Name capture not found");
    let body_index = query
        .capture_index_for_name("body")
        .expect("Body capture not found");

    let mut cursor = QueryCursor::new();
    let mut matches = parsed_file.exec_query(&query, root, &mut cursor);

    while let Some(match_) = matches.next() {
        let name_node = match_
            .nodes_for_capture_index(name_index)
            .next()
            .expect("Missing name node in namespace");
        let namespace_node = name_node.parent().expect("Namespace node has no parent");

        if has_namespace_ancestor(namespace_node, root) {
            continue;
        }

        let name = parsed_file.render_node(name_node)?;
        let body_node = match_
            .nodes_for_capture_index(body_index)
            .next()
            .expect("Missing body node in namespace");

        let inner_content = get_module_symbols(body_node, parsed_file)?;
        let mut is_exported = false;
        let mut current_node = namespace_node;
        let parent = current_node.parent().expect("Namespace node has no parent");
        if parent.kind() == "export_statement" {
            is_exported = true;
            current_node = parent;
        }

        let expression_statement = current_node.parent().expect("Namespace node has no parent");
        let jsdoc = get_jsdoc(expression_statement.prev_sibling(), parsed_file);

        namespaces.push(TypeScriptSymbol::Namespace {
            name,
            content: inner_content,
            is_exported,
            jsdoc,
        });
    }

    Ok(namespaces)
}

fn extract_exports<'a>(
    root: Node<'a>,
    parsed_file: &'a ParsedFile,
) -> Result<Vec<TypeScriptSymbol>, ExtractionError> {
    let mut exports = vec![];
    let query = parsed_file.make_query(EXPORTS_QUERY)?;

    let name_index = query
        .capture_index_for_name("name")
        .expect("Name capture not found");
    let alias_index = query.capture_index_for_name("alias").unwrap();
    let source_index = query.capture_index_for_name("source").unwrap();
    let barrel_export_index = query.capture_index_for_name("barrel_export").unwrap();

    let mut cursor = QueryCursor::new();
    let mut matches = parsed_file.exec_query(&query, root, &mut cursor);

    let mut current_names = vec![];
    let mut current_aliases = HashMap::new();
    let mut current_source = None;

    while let Some(match_) = matches.next() {
        let source_module = match_
            .nodes_for_capture_index(source_index)
            .next()
            .and_then(|n| parsed_file.render_node(n).ok());

        if match_
            .nodes_for_capture_index(barrel_export_index)
            .next()
            .is_some()
        {
            exports.push(TypeScriptSymbol::ModuleExport {
                source_module,
                target: ExportTarget::Barrel,
            });
            continue;
        }

        let name_node = match_
            .nodes_for_capture_index(name_index)
            .next()
            .expect("Missing name node in export");
        let name = parsed_file.render_node(name_node)?;
        let export_node = name_node.parent().expect("Export node has no parent");

        if export_node.kind() == "namespace_export" {
            exports.push(TypeScriptSymbol::ModuleExport {
                source_module,
                target: ExportTarget::Namespace { name },
            });
            continue;
        }

        // Handle source module changes
        if source_module != current_source {
            emit_accumulated_exports(
                &mut exports,
                &mut current_names,
                &mut current_aliases,
                &current_source,
            );
            current_source = source_module;
        }

        // Accumulate the current export
        current_names.push(name.clone());

        if let Some(alias_node) = match_.nodes_for_capture_index(alias_index).next() {
            let alias = parsed_file.render_node(alias_node)?;
            current_aliases.insert(name.clone(), alias.clone());
        }

        // Handle CommonJS exports (export = myFunction)
        if export_node.kind() == "export_statement" {
            emit_accumulated_exports(
                &mut exports,
                &mut current_names,
                &mut current_aliases,
                &current_source,
            );
            current_source = None;
            continue;
        }

        // If this is the last export specifier in the statement, emit it
        let is_last_specifier = export_node.next_named_sibling().is_none();
        if is_last_specifier {
            emit_accumulated_exports(
                &mut exports,
                &mut current_names,
                &mut current_aliases,
                &current_source,
            );
            current_source = None;
        }
    }

    Ok(exports)
}

fn emit_accumulated_exports(
    exports: &mut Vec<TypeScriptSymbol>,
    current_names: &mut Vec<String>,
    current_aliases: &mut HashMap<String, String>,
    current_source: &Option<String>,
) {
    if !current_names.is_empty() {
        exports.push(TypeScriptSymbol::ModuleExport {
            source_module: current_source.clone(),
            target: ExportTarget::Named {
                names: std::mem::take(current_names),
                aliases: std::mem::take(current_aliases),
            },
        });
    }
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
        let path = PathBuf::from("/test/empty.ts");

        let result = parse_typescript_file("", &mut parser, path.clone());

        assert_matches!(result, Ok(Module { path: p, jsdoc: None, symbols: s, default_export_name: None }) if p == path && s.is_empty());
    }

    #[test]
    fn malformed_file() {
        let mut parser = make_parser();

        let result = parse_typescript_file("class {", &mut parser, PathBuf::new());

        assert_matches!(result, Err(ExtractionError::Malformed(msg)) if msg == "Failed to parse source file");
    }

    #[test]
    fn file_path_is_preserved() {
        let mut parser = make_parser();
        let test_path = PathBuf::from("/test/file/path.ts");

        let result = parse_typescript_file("const foo = 42;", &mut parser, test_path.clone());

        assert_matches!(result, Ok(Module { path, .. }) if path == test_path);
    }

    mod module_jsdoc {
        use super::*;

        const FILE_DESCRIPTION: &str = "Description of the file";

        #[test]
        fn file_tag() {
            let mut parser = make_parser();
            let content = format!("/** @file {FILE_DESCRIPTION} */\ndeclare const foo = 42;");

            let result = parse_typescript_file(&content, &mut parser, PathBuf::new());

            assert_matches!(result, Ok(Module { jsdoc: Some(j), .. }) if j == format!("/** @file {FILE_DESCRIPTION} */"));
        }

        #[test]
        fn fileoverview_tag() {
            let mut parser = make_parser();
            let content =
                format!("/** @fileoverview {FILE_DESCRIPTION} */\ndeclare const foo = 42;");

            let result = parse_typescript_file(&content, &mut parser, PathBuf::new());

            assert_matches!(result, Ok(Module { jsdoc: Some(j), .. }) if j == format!("/** @fileoverview {FILE_DESCRIPTION} */"));
        }

        #[test]
        fn module_tag() {
            let mut parser = make_parser();
            let content = format!("/** @module {FILE_DESCRIPTION} */\ndeclare const foo = 42;");

            let result = parse_typescript_file(&content, &mut parser, PathBuf::new());

            assert_matches!(result, Ok(Module { jsdoc: Some(j), .. }) if j == format!("/** @module {FILE_DESCRIPTION} */"));
        }

        #[test]
        fn no_tag() {
            let mut parser = make_parser();
            let content = "/** Just a comment */\ndeclare const foo = 42;";

            let result = parse_typescript_file(content, &mut parser, PathBuf::new());

            assert_matches!(result, Ok(Module { jsdoc: None, .. }));
        }

        #[test]
        fn non_jsdoc_block_comment() {
            let mut parser = make_parser();
            let content = "/* @module Just a comment */\ndeclare const foo = 42;";

            let result = parse_typescript_file(content, &mut parser, PathBuf::new());

            assert_matches!(result, Ok(Module { jsdoc: None, .. }));
        }

        #[test]
        fn line_comment() {
            let mut parser = make_parser();
            let content = "// @module Just a comment\ndeclare const foo = 42;";

            let result = parse_typescript_file(content, &mut parser, PathBuf::new());

            assert_matches!(result, Ok(Module { jsdoc: None, .. }));
        }
    }

    mod symbols {
        use super::*;

        #[test]
        fn class_declaration() {
            let mut parser = make_parser();
            let content = "declare class Foo { bar(): void; }";

            let module = parse_typescript_file(content, &mut parser, PathBuf::new()).unwrap();

            assert_matches!(module, Module { ref symbols, .. } if symbols.len() == 1);
            let symbol = &module.symbols[0];
            assert_matches!(symbol, TypeScriptSymbol::Symbol { symbol, is_exported: false } if symbol.name == "Foo" && symbol.source_code == content);
        }

        #[test]
        fn abstract_class_declaration() {
            let mut parser = make_parser();
            let content = "declare abstract class Foo { bar(): void; }";

            let module = parse_typescript_file(content, &mut parser, PathBuf::new()).unwrap();

            assert_matches!(module, Module { ref symbols, .. } if symbols.len() == 1);
            let symbol = &module.symbols[0];
            assert_matches!(symbol, TypeScriptSymbol::Symbol { symbol, is_exported: false } if symbol.name == "Foo" && symbol.source_code == content);
        }

        #[test]
        fn type_alias_declaration() {
            let mut parser = make_parser();
            let content = "type Bar = string;";

            let module = parse_typescript_file(content, &mut parser, PathBuf::new()).unwrap();

            assert_matches!(module, Module { ref symbols, .. } if symbols.len() == 1);
            let symbol = &module.symbols[0];
            assert_matches!(symbol, TypeScriptSymbol::Symbol { symbol, is_exported: false } if symbol.name == "Bar" && symbol.source_code == content);
        }

        #[test]
        fn interface_declaration() {
            let mut parser = make_parser();
            let content = "interface Baz { qux: number; }";

            let module = parse_typescript_file(content, &mut parser, PathBuf::new()).unwrap();

            assert_matches!(module, Module { ref symbols, .. } if symbols.len() == 1);
            let symbol = &module.symbols[0];
            assert_matches!(symbol, TypeScriptSymbol::Symbol { symbol, is_exported: false } if symbol.name == "Baz" && symbol.source_code == content);
        }

        #[test]
        fn enum_declaration() {
            let mut parser = make_parser();
            let content = "enum Status { Active, Inactive }";

            let module = parse_typescript_file(content, &mut parser, PathBuf::new()).unwrap();

            assert_matches!(module, Module { ref symbols, .. } if symbols.len() == 1);
            let symbol = &module.symbols[0];
            assert_matches!(symbol, TypeScriptSymbol::Symbol { symbol, is_exported: false } if symbol.name == "Status" && symbol.source_code == content);
        }

        #[test]
        fn function_declaration() {
            let mut parser = make_parser();
            let content = "declare function greet(name: string): void;";

            let module = parse_typescript_file(content, &mut parser, PathBuf::new()).unwrap();

            assert_matches!(module, Module { ref symbols, .. } if symbols.len() == 1);
            let symbol = &module.symbols[0];
            assert_matches!(symbol, TypeScriptSymbol::Symbol { symbol, is_exported: false } if symbol.name == "greet" && symbol.source_code == content);
        }

        #[test]
        fn const_declaration() {
            let mut parser = make_parser();
            let content = "declare const VERSION: string;";

            let module = parse_typescript_file(content, &mut parser, PathBuf::new()).unwrap();

            assert_matches!(module, Module { ref symbols, .. } if symbols.len() == 1);
            let symbol = &module.symbols[0];
            assert_matches!(symbol, TypeScriptSymbol::Symbol { symbol, is_exported: false } if symbol.name == "VERSION" && symbol.source_code == content);
        }

        #[test]
        fn let_declaration() {
            let mut parser = make_parser();
            let content = "declare let counter: number;";

            let module = parse_typescript_file(content, &mut parser, PathBuf::new()).unwrap();

            assert_matches!(module, Module { ref symbols, .. } if symbols.len() == 1);
            let symbol = &module.symbols[0];
            assert_matches!(symbol, TypeScriptSymbol::Symbol { symbol, is_exported: false } if symbol.name == "counter" && symbol.source_code == content);
        }

        #[test]
        fn symbol_with_jsdoc() {
            let mut parser = make_parser();
            let content = "/** The version number */\ndeclare const VERSION: string;";

            let module = parse_typescript_file(content, &mut parser, PathBuf::new()).unwrap();

            assert_matches!(module, Module { ref symbols, .. } if symbols.len() == 1);
            let symbol = &module.symbols[0];
            assert_matches!(symbol, TypeScriptSymbol::Symbol { symbol, is_exported: false } if symbol.name == "VERSION" && symbol.source_code == content);
        }

        #[test]
        fn symbol_without_jsdoc() {
            let mut parser = make_parser();
            let content = "declare const VERSION: string;";

            let module = parse_typescript_file(content, &mut parser, PathBuf::new()).unwrap();

            assert_matches!(module, Module { ref symbols, .. } if symbols.len() == 1);
            let symbol = &module.symbols[0];
            assert_matches!(symbol, TypeScriptSymbol::Symbol { symbol, is_exported: false } if symbol.name == "VERSION" && symbol.source_code == content);
        }

        #[test]
        fn symbol_with_preceding_module_jsdoc_comment() {
            let mut parser = make_parser();
            let content = "/** @module The module description */\ndeclare const VERSION: string;";

            let module = parse_typescript_file(content, &mut parser, PathBuf::new()).unwrap();

            assert_matches!(module, Module { ref symbols, .. } if symbols.len() == 1);
            let symbol = &module.symbols[0];
            assert_matches!(symbol, TypeScriptSymbol::Symbol { symbol, is_exported: false } if symbol.source_code == "declare const VERSION: string;".to_string());
        }

        #[test]
        fn symbol_with_preceding_non_jsdoc_comment() {
            let mut parser = make_parser();
            let content = "// The comment\ndeclare const VERSION: string;";

            let module = parse_typescript_file(content, &mut parser, PathBuf::new()).unwrap();

            assert_matches!(module, Module { ref symbols, .. } if symbols.len() == 1);
            let symbol = &module.symbols[0];
            assert_matches!(symbol, TypeScriptSymbol::Symbol { symbol, is_exported: false } if symbol.source_code == "declare const VERSION: string;".to_string());
        }

        #[test]
        fn export_and_declaration() {
            let mut parser = make_parser();
            let content = "export declare function greet(name: string): void;";

            let module = parse_typescript_file(content, &mut parser, PathBuf::new()).unwrap();

            assert_matches!(module, Module { ref symbols, .. } if symbols.len() == 1);
            let symbol = &module.symbols[0];
            assert_matches!(symbol, TypeScriptSymbol::Symbol { symbol, is_exported: true } if symbol.name == "greet" && symbol.source_code == content);
        }

        #[test]
        fn default_export_and_declaration() {
            let mut parser = make_parser();
            let content = "export default declare function greet(name: string): void;";

            let module = parse_typescript_file(content, &mut parser, PathBuf::new()).unwrap();

            assert_matches!(&module, Module { symbols, default_export_name: Some(n), .. } if symbols.len() == 1 && n == "greet");
            let symbol = &module.symbols[0];
            assert_matches!(symbol, TypeScriptSymbol::Symbol { symbol, is_exported: true } if symbol.name == "greet" && symbol.source_code == content);
        }
    }

    mod namespaces {
        use crate::api::test_helpers::deconstruct_namespace;

        use super::*;

        #[test]
        fn empty_namespace() {
            let mut parser = make_parser();
            let content = "namespace Foo {}";

            let module = parse_typescript_file(content, &mut parser, PathBuf::new()).unwrap();

            assert_eq!(module.symbols.len(), 1);
            let namespace = &module.symbols[0];
            assert_matches!(namespace, TypeScriptSymbol::Namespace { name, .. } if name == "Foo");
            assert_matches!(namespace, TypeScriptSymbol::Namespace { content, .. } if content.is_empty());
        }

        #[test]
        fn namespace_with_symbol() {
            let mut parser = make_parser();
            let content = "namespace Foo { declare const VERSION: string; }";

            let module = parse_typescript_file(content, &mut parser, PathBuf::new()).unwrap();

            assert_eq!(module.symbols.len(), 1);
            let namespace = &module.symbols[0];
            assert_matches!(namespace, TypeScriptSymbol::Namespace { name, .. } if name == "Foo");
            assert_matches!(namespace, TypeScriptSymbol::Namespace { content, .. } if content.len() == 1);
        }

        #[test]
        fn exported_namespace() {
            let mut parser = make_parser();
            let content = "export namespace Foo { declare const VERSION: string; }";

            let module = parse_typescript_file(content, &mut parser, PathBuf::new()).unwrap();

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

            let module = parse_typescript_file(content, &mut parser, PathBuf::new()).unwrap();

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

            let module = parse_typescript_file(content, &mut parser, PathBuf::new()).unwrap();

            assert_eq!(module.symbols.len(), 1);
            let (outer_name, outer_content, outer_exported, outer_jsdoc) =
                deconstruct_namespace(&module.symbols[0]);
            assert_eq!(outer_name, "Foo");
            assert_eq!(outer_content.len(), 1);
            assert!(!outer_exported);
            assert_eq!(outer_jsdoc, None);

            let (inner_name, inner_content, inner_exported, inner_jsdoc) =
                deconstruct_namespace(&outer_content[0]);
            assert_eq!(inner_name, "Bar");
            assert_eq!(inner_content.len(), 1);
            assert!(!inner_exported);
            assert_eq!(inner_jsdoc, None);

            let symbol = &inner_content[0];
            assert_matches!(symbol, TypeScriptSymbol::Symbol { symbol, is_exported: true } if symbol.name == "VERSION");
        }

        #[test]
        fn namespace_with_jsdoc() {
            let mut parser = make_parser();
            let content =
                "/** Utility functions */\nnamespace Foo { declare const VERSION: string; }";

            let module = parse_typescript_file(content, &mut parser, PathBuf::new()).unwrap();

            assert_eq!(module.symbols.len(), 1);
            let namespace = &module.symbols[0];
            assert_matches!(namespace, TypeScriptSymbol::Namespace { name, .. } if name == "Foo");
            assert_matches!(namespace, TypeScriptSymbol::Namespace { jsdoc: Some(j), .. } if j == "/** Utility functions */");
        }

        #[test]
        fn namespace_without_jsdoc() {
            let mut parser = make_parser();
            let content = "namespace Foo { declare const VERSION: string; }";

            let module = parse_typescript_file(content, &mut parser, PathBuf::new()).unwrap();

            assert_eq!(module.symbols.len(), 1);
            let namespace = &module.symbols[0];
            assert_matches!(namespace, TypeScriptSymbol::Namespace { jsdoc: None, .. });
        }
    }

    mod imports {
        use super::*;
        use crate::api::test_helpers::deconstruct_module_import;

        #[test]
        fn default_import() {
            let mut parser = make_parser();
            let content = "import foo from './foo.js';";

            let module = parse_typescript_file(content, &mut parser, PathBuf::new()).unwrap();

            assert_matches!(&module, Module { symbols, .. } if symbols.len() == 1);
            let (source_module, target) = deconstruct_module_import(&module.symbols[0]);
            assert_eq!(source_module, "./foo.js");
            assert_matches!(target, ImportTarget::Default { name } if name == "foo");
        }

        #[test]
        fn namespace_import() {
            let mut parser = make_parser();
            let content = "import * as foo from './foo.js';";

            let module = parse_typescript_file(content, &mut parser, PathBuf::new()).unwrap();

            assert_matches!(&module, Module { symbols, .. } if symbols.len() == 1);
            let (source_module, target) = deconstruct_module_import(&module.symbols[0]);
            assert_eq!(source_module, "./foo.js");
            assert_matches!(target, ImportTarget::Namespace { name } if name == "foo");
        }

        #[test]
        fn named_import() {
            let mut parser = make_parser();
            let content = "import { foo } from './foo.js';";

            let module = parse_typescript_file(content, &mut parser, PathBuf::new()).unwrap();

            assert_matches!(&module, Module { symbols, .. } if symbols.len() == 1);
            let (source_module, target) = deconstruct_module_import(&module.symbols[0]);
            assert_eq!(source_module, "./foo.js");
            assert_matches!(target, ImportTarget::Named { names, aliases } if names == vec!["foo".to_string()] && aliases.is_empty());
        }

        #[test]
        fn named_import_with_alias() {
            let mut parser = make_parser();
            let content = "import { foo as bar } from './foo.js';";

            let module = parse_typescript_file(content, &mut parser, PathBuf::new()).unwrap();

            assert_matches!(&module, Module { symbols, .. } if symbols.len() == 1);
            let (source_module, target) = deconstruct_module_import(&module.symbols[0]);
            assert_eq!(source_module, "./foo.js");
            assert_matches!(target, ImportTarget::Named { names, aliases } if names == vec!["foo".to_string()] && aliases == HashMap::from([("foo".to_string(), "bar".to_string())]));
        }

        #[test]
        fn mixed_import() {
            let mut parser = make_parser();
            let content = "import foo, { bar } from './foo.js';";

            let module = parse_typescript_file(content, &mut parser, PathBuf::new()).unwrap();

            assert_matches!(&module, Module { symbols, .. } if symbols.len() == 2);

            let (source_module, target) = deconstruct_module_import(&module.symbols[0]);
            assert_eq!(source_module, "./foo.js");
            assert_matches!(target, ImportTarget::Default { name } if name == "foo");

            let (source_module, target) = deconstruct_module_import(&module.symbols[1]);
            assert_eq!(source_module, "./foo.js");
            assert_matches!(target, ImportTarget::Named { names, aliases } if names == vec!["bar".to_string()] && aliases.is_empty());
        }

        #[test]
        fn multiple_named_imports() {
            let mut parser = make_parser();
            let content = "import { foo, bar as baz } from './foo.js';";

            let module = parse_typescript_file(content, &mut parser, PathBuf::new()).unwrap();

            assert_matches!(&module, Module { symbols, .. } if symbols.len() == 1);
            let (source_module, target) = deconstruct_module_import(&module.symbols[0]);
            assert_eq!(source_module, "./foo.js");
            assert_matches!(target, ImportTarget::Named { names, aliases } if names == vec!["foo".to_string(), "bar".to_string()] && aliases == HashMap::from([("bar".to_string(), "baz".to_string())]));
        }
    }

    mod exports {
        use super::*;
        use crate::api::test_helpers::deconstruct_module_export;

        #[test]
        fn namespace_export_from_another_module() {
            let mut parser = make_parser();
            let content = "export * as foo from './foo.js';";

            let module = parse_typescript_file(content, &mut parser, PathBuf::new()).unwrap();

            assert_matches!(&module, Module { symbols, .. } if symbols.len() == 1);
            let (source_module, target) = deconstruct_module_export(&module.symbols[0]);
            assert_eq!(source_module, Some("./foo.js".to_string()));
            assert_matches!(target, ExportTarget::Namespace { name } if name == "foo");
        }

        #[test]
        fn named_export_from_another_module() {
            let mut parser = make_parser();
            let content = "export { foo, bar } from './foo.js';";

            let module = parse_typescript_file(content, &mut parser, PathBuf::new()).unwrap();

            assert_matches!(&module, Module { symbols, .. } if symbols.len() == 1);
            let (source_module, target) = deconstruct_module_export(&module.symbols[0]);
            assert_eq!(source_module, Some("./foo.js".to_string()));
            assert_matches!(target, ExportTarget::Named { names, aliases } if *names == vec!["foo".to_string(), "bar".to_string()] && aliases.is_empty());
        }

        #[test]
        fn aliased_export_from_another_module() {
            let mut parser = make_parser();
            let content = "export { foo as bar } from './foo.js';";

            let module = parse_typescript_file(content, &mut parser, PathBuf::new()).unwrap();

            assert_matches!(&module, Module { symbols, .. } if symbols.len() == 1);
            let (source_module, target) = deconstruct_module_export(&module.symbols[0]);
            assert_eq!(source_module, Some("./foo.js".to_string()));
            assert_matches!(target, ExportTarget::Named { names, aliases } if *names == vec!["foo".to_string()] && aliases == HashMap::from([("foo".to_string(), "bar".to_string())]));
        }

        #[test]
        fn barrel_export_from_another_module() {
            let mut parser = make_parser();
            let content = "export * from './foo.js';";

            let module = parse_typescript_file(content, &mut parser, PathBuf::new()).unwrap();

            assert_matches!(&module, Module { symbols, .. } if symbols.len() == 1);
            let (source_module, target) = deconstruct_module_export(&module.symbols[0]);
            assert_eq!(source_module, Some("./foo.js".to_string()));
            assert_matches!(target, ExportTarget::Barrel);
        }

        #[test]
        fn symbol_export() {
            let mut parser = make_parser();
            let content = "\nexport { VERSION };";

            let module = parse_typescript_file(content, &mut parser, PathBuf::new()).unwrap();

            assert_matches!(&module, Module { symbols, .. } if symbols.len() == 1);
            let (source_module, target) = deconstruct_module_export(&module.symbols[0]);
            assert_eq!(source_module, None);
            assert_matches!(target, ExportTarget::Named { names, aliases } if *names == vec!["VERSION".to_string()] && aliases.is_empty());
        }

        #[test]
        fn commonjs_export() {
            let mut parser = make_parser();
            let content = "export = myFunction;";

            let module = parse_typescript_file(content, &mut parser, PathBuf::new()).unwrap();

            assert_matches!(&module, Module { symbols, .. } if symbols.len() == 1);
            let (source_module, target) = deconstruct_module_export(&module.symbols[0]);
            assert_eq!(source_module, None);
            assert_matches!(target, ExportTarget::Named { names, aliases } if *names == vec!["myFunction".to_string()] && aliases.is_empty());
        }

        #[test]
        fn default_export() {
            let mut parser = make_parser();
            let content = "export default VERSION;";

            let module = parse_typescript_file(content, &mut parser, PathBuf::new()).unwrap();

            assert_matches!(&module, Module { default_export_name: Some(n), .. } if n == "VERSION");
        }

        #[test]
        fn mixed_export_from_another_module() {
            let mut parser = make_parser();
            let content = "export { foo, bar as baz } from './module.js';";

            let module = parse_typescript_file(content, &mut parser, PathBuf::new()).unwrap();

            assert_matches!(&module, Module { symbols, .. } if symbols.len() == 1);
            let (source_module, target) = deconstruct_module_export(&module.symbols[0]);
            assert_eq!(source_module, Some("./module.js".to_string()));
            assert_matches!(target, ExportTarget::Named { names, aliases } if *names == vec!["foo".to_string(), "bar".to_string()] && aliases == HashMap::from([("bar".to_string(), "baz".to_string())]));
        }

        #[test]
        fn exports_from_multiple_modules() {
            let mut parser = make_parser();
            let content = "export { foo } from './foo.js';\nexport { bar } from './bar.js';";

            let module = parse_typescript_file(content, &mut parser, PathBuf::new()).unwrap();

            assert_matches!(&module, Module { symbols, .. } if symbols.len() == 2);
            let (source_module, target) = deconstruct_module_export(&module.symbols[0]);
            assert_eq!(source_module, Some("./foo.js".to_string()));
            assert_matches!(target, ExportTarget::Named { names, aliases } if *names == vec!["foo".to_string()] && aliases.is_empty());

            let (source_module, target) = deconstruct_module_export(&module.symbols[1]);
            assert_eq!(source_module, Some("./bar.js".to_string()));
            assert_matches!(target, ExportTarget::Named { names, aliases } if *names == vec!["bar".to_string()] && aliases.is_empty());
        }
    }
}
