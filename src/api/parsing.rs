use std::collections::HashMap;

use daipendency_extractor::{ExtractionError, Symbol};
use tree_sitter::{Node, Parser};

use crate::api::module::{ExportTarget, ImportTarget, Module, TypeScriptSymbol};

// TODO: REMOVE WHEN ALL TESTS ARE FIXED
#[cfg(test)]
use daipendency_testing::debug_node;

pub fn parse_typescript_file(
    content: &str,
    parser: &mut Parser,
) -> Result<Module, ExtractionError> {
    let tree = parser
        .parse(content, None)
        .ok_or_else(|| ExtractionError::Malformed("Failed to parse source file".to_string()))?;
    let node = tree.root_node();

    // TODO: REMOVE WHEN ALL TESTS ARE FIXED
    #[cfg(test)]
    println!("{}", debug_node(&node, content));

    if node.has_error() {
        return Err(ExtractionError::Malformed(
            "Failed to parse source file".to_string(),
        ));
    }

    let module_jsdoc = get_module_jsdoc(node, content);
    let mut module_symbols = vec![];
    let mut module_exported_names = vec![];
    let mut default_export_name = None;

    // Extract symbols and track exports
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "export_statement" => {
                let (symbols, exported, default) = extract_exports(&child, content);
                module_symbols.extend(symbols);
                module_exported_names.extend(exported);
                if default.is_some() {
                    default_export_name = default;
                }
            }
            "import_statement" => {
                let symbols = extract_imports(&child, content);
                module_symbols.extend(symbols);
            }
            "ambient_declaration" => {
                if let Some(symbol) = extract_ambient(&child, content, false) {
                    module_symbols.push(symbol);
                }
            }
            "expression_statement" => {
                if let Some(symbol) = extract_namespace(&child, content) {
                    module_symbols.push(symbol);
                }
            }
            "class_declaration"
            | "interface_declaration"
            | "type_alias_declaration"
            | "enum_declaration" => {
                let prev_sibling = child.prev_sibling();
                let jsdoc = prev_sibling
                    .filter(|n| n.kind() == "comment")
                    .and_then(|n| n.utf8_text(content.as_bytes()).ok());
                let source_start = jsdoc
                    .as_ref()
                    .map_or(child.start_byte(), |_| prev_sibling.unwrap().start_byte());
                if let Some(symbol) = extract_symbol(&child, content, source_start) {
                    module_symbols.push(symbol);
                }
            }
            _ => {}
        }
    }

    // Update exported flag for symbols that are exported
    for symbol in &mut module_symbols {
        if let TypeScriptSymbol::Symbol { symbol, exported } = symbol {
            if module_exported_names.contains(&symbol.name) {
                *exported = true;
            }
        }
    }

    Ok(Module {
        jsdoc: module_jsdoc,
        symbols: module_symbols,
        default_export_name,
    })
}

fn get_jsdoc(node: Option<Node>, content: &str) -> Option<String> {
    node.filter(|n| n.kind() == "comment")
        .and_then(|n| n.utf8_text(content.as_bytes()).ok())
        .filter(|comment| comment.starts_with("/**"))
        .map(|comment| comment.to_string())
}

fn get_module_jsdoc<'a>(node: Node<'a>, content: &'a str) -> Option<String> {
    get_jsdoc(node.child(0), content).and_then(|comment_text| {
        if comment_text.contains("@file")
            || comment_text.contains("@fileoverview")
            || comment_text.contains("@module")
        {
            Some(comment_text.to_string())
        } else {
            None
        }
    })
}

fn extract_symbol(node: &Node, content: &str, source_start: usize) -> Option<TypeScriptSymbol> {
    let name = node
        .child_by_field_name("name")?
        .utf8_text(content.as_bytes())
        .ok()?;
    let source_code = &content[source_start..node.end_byte()];
    Some(TypeScriptSymbol::Symbol {
        symbol: Symbol {
            name: name.to_string(),
            source_code: source_code.to_string(),
        },
        exported: false,
    })
}

fn extract_imports(node: &Node, content: &str) -> Vec<TypeScriptSymbol> {
    let mut cursor = node.walk();
    let mut children = node.children(&mut cursor);
    let mut symbols = vec![];

    // Skip "import" keyword
    if children.next().is_none() {
        return symbols;
    }

    // Get import clause
    let import_clause = match children.next() {
        Some(clause) => clause,
        None => return symbols,
    };

    // Skip "from" keyword
    if children.next().is_none() {
        return symbols;
    }

    // Get source module
    let source_module = match children
        .next()
        .and_then(|n| n.utf8_text(content.as_bytes()).ok())
        .map(|s| s.trim_matches('\'').to_string())
    {
        Some(module) => module,
        None => return symbols,
    };

    if import_clause.kind() == "import_clause" {
        let mut cursor = import_clause.walk();
        let children = import_clause.children(&mut cursor);

        for child in children {
            match child.kind() {
                "identifier" => {
                    let name = match child.utf8_text(content.as_bytes()).ok() {
                        Some(name) => name.to_string(),
                        None => continue,
                    };
                    symbols.push(TypeScriptSymbol::ModuleImport {
                        source_module: source_module.clone(),
                        target: ImportTarget::Default { name },
                    });
                }
                "namespace_import" => {
                    let mut cursor = child.walk();
                    let mut children = child.children(&mut cursor);

                    // Skip "*" and "as" tokens
                    children.next();
                    children.next();

                    // Get the name
                    if let Some(name) = children
                        .next()
                        .and_then(|n| n.utf8_text(content.as_bytes()).ok())
                    {
                        symbols.push(TypeScriptSymbol::ModuleImport {
                            source_module: source_module.clone(),
                            target: ImportTarget::Namespace {
                                name: name.to_string(),
                            },
                        });
                    }
                }
                "named_imports" => {
                    let mut names = vec![];
                    let mut cursor = child.walk();
                    let mut aliases = HashMap::new();
                    for import_specifier in child.children(&mut cursor) {
                        if import_specifier.kind() == "import_specifier" {
                            let mut specifier_cursor = import_specifier.walk();
                            let mut specifier_children =
                                import_specifier.children(&mut specifier_cursor);
                            if let Some(name_node) = specifier_children.next() {
                                if name_node.kind() == "identifier" {
                                    if let Ok(name) = name_node.utf8_text(content.as_bytes()) {
                                        // Check for alias
                                        if let Some(alias_node) =
                                            specifier_children.find(|n| n.kind() == "identifier")
                                        {
                                            if let Ok(alias) =
                                                alias_node.utf8_text(content.as_bytes())
                                            {
                                                aliases.insert(name.to_string(), alias.to_string());
                                            }
                                        }
                                        names.push(name.to_string());
                                    }
                                }
                            }
                        }
                    }
                    if !names.is_empty() {
                        symbols.push(TypeScriptSymbol::ModuleImport {
                            source_module: source_module.clone(),
                            target: ImportTarget::Named { names, aliases },
                        });
                    }
                }
                _ => {}
            }
        }
    }

    symbols
}

fn extract_exports(
    node: &Node,
    content: &str,
) -> (Vec<TypeScriptSymbol>, Vec<String>, Option<String>) {
    let mut cursor = node.walk();
    let mut children = node.children(&mut cursor);
    let mut symbols = vec![];
    let mut exported_names = vec![];
    let mut default_export_name = None;

    // Skip "export" keyword
    children.next();

    if let Some(first_child) = children.next() {
        match first_child.kind() {
            "default" => {
                // Handle "export default ..."
                if let Some(next_child) = children.next() {
                    match next_child.kind() {
                        "ambient_declaration" => {
                            if let Some(symbol) = extract_ambient(&next_child, content, true) {
                                if let TypeScriptSymbol::Symbol { symbol, .. } = &symbol {
                                    exported_names.push(symbol.name.clone());
                                    default_export_name = Some(symbol.name.clone());
                                }
                                symbols.push(symbol);
                            }
                        }
                        "identifier" => {
                            if let Ok(name) = next_child.utf8_text(content.as_bytes()) {
                                exported_names.push(name.to_string());
                                default_export_name = Some(name.to_string());
                            }
                        }
                        _ => {}
                    }
                }
            }
            "=" => {
                // Handle CommonJS export (export = ...)
                if let Some(next_child) = children.next() {
                    if next_child.kind() == "identifier" {
                        if let Ok(name) = next_child.utf8_text(content.as_bytes()) {
                            exported_names.push(name.to_string());
                        }
                    }
                }
            }
            "ambient_declaration" => {
                if let Some(symbol) = extract_ambient(&first_child, content, true) {
                    if let TypeScriptSymbol::Symbol { symbol, .. } = &symbol {
                        exported_names.push(symbol.name.clone());
                    }
                    symbols.push(symbol);
                }
            }
            "export_clause" => {
                let mut specifier_cursor = first_child.walk();
                let mut names = vec![];
                let mut aliases = HashMap::new();

                for export_specifier in first_child.children(&mut specifier_cursor) {
                    if export_specifier.kind() == "export_specifier" {
                        let mut specifier_cursor = export_specifier.walk();
                        let mut specifier_children =
                            export_specifier.children(&mut specifier_cursor);
                        if let Some(name_node) = specifier_children.next() {
                            if name_node.kind() == "identifier" {
                                if let Ok(name) = name_node.utf8_text(content.as_bytes()) {
                                    names.push(name.to_string());

                                    // Check for alias
                                    if let Some(alias_node) =
                                        specifier_children.find(|n| n.kind() == "identifier")
                                    {
                                        if let Ok(alias) = alias_node.utf8_text(content.as_bytes())
                                        {
                                            aliases.insert(name.to_string(), alias.to_string());
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // Skip "from" keyword
                if let Some(from_node) = children.next() {
                    if from_node.kind() != "from" {
                        // This is a local export, not a re-export
                        exported_names.extend(names);
                    } else {
                        // Get source module
                        if let Some(source_module) = children
                            .next()
                            .and_then(|n| n.utf8_text(content.as_bytes()).ok())
                        {
                            let source_module = source_module.trim_matches('\'').to_string();

                            symbols.push(TypeScriptSymbol::ModuleExport {
                                source_module,
                                target: ExportTarget::Named { names, aliases },
                            });
                        }
                    }
                }
            }
            "*" => {
                // Skip "from" keyword
                children.next();

                // Get source module
                if let Some(source_module) = children
                    .next()
                    .and_then(|n| n.utf8_text(content.as_bytes()).ok())
                {
                    let source_module = source_module.trim_matches('\'').to_string();
                    symbols.push(TypeScriptSymbol::ModuleExport {
                        source_module,
                        target: ExportTarget::Barrel,
                    });
                }
            }
            "namespace_export" => {
                let mut cursor = first_child.walk();
                let mut namespace_children = first_child.children(&mut cursor);

                // Skip "*" token
                namespace_children.next();
                // Skip "as" token
                namespace_children.next();

                // Get the name
                if let Some(name) = namespace_children
                    .next()
                    .and_then(|n| n.utf8_text(content.as_bytes()).ok())
                {
                    // Skip "from" keyword
                    children.next();

                    // Get source module
                    if let Some(source_module) = children
                        .next()
                        .and_then(|n| n.utf8_text(content.as_bytes()).ok())
                    {
                        let source_module = source_module.trim_matches('\'').to_string();
                        symbols.push(TypeScriptSymbol::ModuleExport {
                            source_module,
                            target: ExportTarget::Namespace {
                                name: name.to_string(),
                            },
                        });
                    }
                }
            }
            _ => {}
        }
    }

    (symbols, exported_names, default_export_name)
}

fn extract_ambient(node: &Node, content: &str, exported: bool) -> Option<TypeScriptSymbol> {
    let declaration = node.child(1)?;
    let source_start = if exported {
        // If exported, we need to include the entire export statement
        node.parent()?.start_byte()
    } else {
        // Otherwise, include JSDoc if present
        get_jsdoc(node.prev_sibling(), content).map_or(node.start_byte(), |_| {
            node.prev_sibling().unwrap().start_byte()
        })
    };
    let source_end = if exported {
        // If exported, we need to include the entire export statement
        node.parent()?.end_byte()
    } else {
        node.end_byte()
    };
    let source_code = &content[source_start..source_end];
    match declaration.kind() {
        "function_signature" => {
            let name = declaration
                .child_by_field_name("name")?
                .utf8_text(content.as_bytes())
                .ok()?;
            Some(TypeScriptSymbol::Symbol {
                symbol: Symbol {
                    name: name.to_string(),
                    source_code: source_code.to_string(),
                },
                exported,
            })
        }
        "lexical_declaration" => {
            let variable = declaration.child(1)?;
            let name = variable
                .child_by_field_name("name")?
                .utf8_text(content.as_bytes())
                .ok()?;
            Some(TypeScriptSymbol::Symbol {
                symbol: Symbol {
                    name: name.to_string(),
                    source_code: source_code.to_string(),
                },
                exported,
            })
        }
        "class_declaration" => {
            let name = declaration
                .child_by_field_name("name")?
                .utf8_text(content.as_bytes())
                .ok()?;
            Some(TypeScriptSymbol::Symbol {
                symbol: Symbol {
                    name: name.to_string(),
                    source_code: source_code.to_string(),
                },
                exported,
            })
        }
        _ => None,
    }
}

fn extract_namespace(node: &Node, content: &str) -> Option<TypeScriptSymbol> {
    if let Some(internal_module) = node.child(0).filter(|n| n.kind() == "internal_module") {
        let name = internal_module
            .child_by_field_name("name")?
            .utf8_text(content.as_bytes())
            .ok()?;
        let statement_block = internal_module.child_by_field_name("body")?;
        let mut content_symbols = vec![];
        let mut cursor = statement_block.walk();
        for child in statement_block.children(&mut cursor) {
            let kind = child.kind();
            if let Some(symbol) = match kind {
                "ambient_declaration" => extract_ambient(&child, content, false),
                _ => None,
            } {
                content_symbols.push(symbol);
            }
        }

        Some(TypeScriptSymbol::Namespace {
            name: name.to_string(),
            content: content_symbols,
            exported: false,
            jsdoc: get_jsdoc(node.prev_sibling(), content),
        })
    } else {
        None
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

            let result = parse_typescript_file(content, &mut parser);

            assert_matches!(result, Ok(Module { symbols: s, .. }) if s.len() == 1 && matches!(&s[0], TypeScriptSymbol::Symbol { symbol, exported: false } if symbol.name == "Foo" && symbol.source_code == content));
        }

        #[test]
        fn type_alias_declaration() {
            let mut parser = make_parser();
            let content = "type Bar = string;";

            let result = parse_typescript_file(content, &mut parser);

            assert_matches!(result, Ok(Module { symbols: s, .. }) if s.len() == 1 && matches!(&s[0], TypeScriptSymbol::Symbol { symbol, exported: false } if symbol.name == "Bar" && symbol.source_code == content));
        }

        #[test]
        fn interface_declaration() {
            let mut parser = make_parser();
            let content = "interface Baz { qux: number; }";

            let result = parse_typescript_file(content, &mut parser);

            assert_matches!(result, Ok(Module { symbols: s, .. }) if s.len() == 1 && matches!(&s[0], TypeScriptSymbol::Symbol { symbol, exported: false } if symbol.name == "Baz" && symbol.source_code == content));
        }

        #[test]
        fn enum_declaration() {
            let mut parser = make_parser();
            let content = "enum Status { Active, Inactive }";

            let result = parse_typescript_file(content, &mut parser);

            assert_matches!(result, Ok(Module { symbols: s, .. }) if s.len() == 1 && matches!(&s[0], TypeScriptSymbol::Symbol { symbol, exported: false } if symbol.name == "Status" && symbol.source_code == content));
        }

        #[test]
        fn function_declaration() {
            let mut parser = make_parser();
            let content = "declare function greet(name: string): void;";

            let result = parse_typescript_file(content, &mut parser);

            assert_matches!(result, Ok(Module { symbols: s, .. }) if s.len() == 1 && matches!(&s[0], TypeScriptSymbol::Symbol { symbol, exported: false } if symbol.name == "greet" && symbol.source_code == content));
        }

        #[test]
        fn const_declaration() {
            let mut parser = make_parser();
            let content = "declare const VERSION: string;";

            let result = parse_typescript_file(content, &mut parser);

            assert_matches!(result, Ok(Module { symbols: s, .. }) if s.len() == 1 && matches!(&s[0], TypeScriptSymbol::Symbol { symbol, exported: false } if symbol.name == "VERSION" && symbol.source_code == content));
        }

        #[test]
        fn let_declaration() {
            let mut parser = make_parser();
            let content = "declare let counter: number;";

            let result = parse_typescript_file(content, &mut parser);

            assert_matches!(result, Ok(Module { symbols: s, .. }) if s.len() == 1 && matches!(&s[0], TypeScriptSymbol::Symbol { symbol, exported: false } if symbol.name == "counter" && symbol.source_code == content));
        }

        #[test]
        fn symbol_with_jsdoc() {
            let mut parser = make_parser();
            let content = "/** The version number */\ndeclare const VERSION: string;";

            let result = parse_typescript_file(content, &mut parser);

            assert_matches!(result, Ok(Module { symbols: s, .. }) if s.len() == 1 && matches!(&s[0], TypeScriptSymbol::Symbol { symbol, exported: false } if symbol.name == "VERSION" && symbol.source_code == content));
        }

        #[test]
        fn symbol_without_jsdoc() {
            let mut parser = make_parser();
            let content = "declare const VERSION: string;";

            let result = parse_typescript_file(content, &mut parser);

            assert_matches!(result, Ok(Module { symbols: s, .. }) if s.len() == 1 && matches!(&s[0], TypeScriptSymbol::Symbol { symbol, exported: false } if symbol.name == "VERSION" && symbol.source_code == content));
        }
    }

    mod symbol_exports {
        use super::*;

        #[test]
        fn exported_afterwards() {
            let mut parser = make_parser();
            let content = "declare const VERSION: string;\nexport { VERSION };";

            let result = parse_typescript_file(content, &mut parser);

            assert_matches!(result, Ok(Module { symbols: s, .. }) if s.len() == 1 && matches!(&s[0], TypeScriptSymbol::Symbol { symbol, exported: true } if symbol.name == "VERSION" && symbol.source_code.contains("declare const VERSION: string")));
        }

        #[test]
        fn export_and_declaration() {
            let mut parser = make_parser();
            let content = "export declare function greet(name: string): void;";

            let result = parse_typescript_file(content, &mut parser);

            assert_matches!(result, Ok(Module { symbols: s, .. }) if s.len() == 1 && matches!(&s[0], TypeScriptSymbol::Symbol { symbol, exported: true } if symbol.name == "greet" && symbol.source_code == content));
        }

        #[test]
        fn default_export_and_declaration() {
            let mut parser = make_parser();
            let content = "export default declare function greet(name: string): void;";

            let module = parse_typescript_file(content, &mut parser).unwrap();

            assert_matches!(&module, Module { symbols, default_export_name: Some(n), .. } if symbols.len() == 1 && n == "greet");
            let symbol = &module.symbols[0];
            assert_matches!(symbol, TypeScriptSymbol::Symbol { symbol, exported: true } if symbol.name == "greet" && symbol.source_code == content);
        }

        #[test]
        fn default_export_afterwards() {
            let mut parser = make_parser();
            let content = "declare const VERSION: string;\nexport default VERSION;";

            let module = parse_typescript_file(content, &mut parser).unwrap();

            assert_matches!(&module, Module { symbols, default_export_name: Some(n), .. } if symbols.len() == 1 && n == "VERSION");
            let symbol = &module.symbols[0];
            assert_matches!(symbol, TypeScriptSymbol::Symbol { symbol, exported: true } if symbol.name == "VERSION" && symbol.source_code.contains("declare const VERSION: string"));
        }

        #[test]
        fn commonjs_export() {
            let mut parser = make_parser();
            let content = "declare function myFunction(): void;\nexport = myFunction;";

            let result = parse_typescript_file(content, &mut parser);

            assert_matches!(result, Ok(Module { symbols: s, .. }) if s.len() == 1 && matches!(&s[0], TypeScriptSymbol::Symbol { symbol, exported: true } if symbol.name == "myFunction" && symbol.source_code.contains("declare function myFunction(): void")));
        }
    }

    mod namespaces {
        use super::*;

        #[test]
        fn empty_namespace() {
            let mut parser = make_parser();
            let content = "namespace Foo {}";

            let result = parse_typescript_file(content, &mut parser);

            assert_matches!(result, Ok(Module { symbols: s, .. }) if s.len() == 1 && matches!(&s[0], TypeScriptSymbol::Namespace { name, content, exported: false, jsdoc: None } if name == "Foo" && content.is_empty()));
        }

        #[test]
        fn namespace_with_symbol() {
            let mut parser = make_parser();
            let content = "namespace Foo { declare const VERSION: string; }";

            let result = parse_typescript_file(content, &mut parser);

            assert_matches!(result, Ok(Module { symbols: s, .. }) if s.len() == 1 && matches!(&s[0], TypeScriptSymbol::Namespace { name, content, exported: false, jsdoc: None } if name == "Foo" && content.len() == 1));
        }

        #[test]
        fn namespace_with_symbols() {
            let mut parser = make_parser();
            let content =
                "namespace Foo { declare const VERSION: string; declare function greet(): void; }";

            let result = parse_typescript_file(content, &mut parser);

            assert_matches!(result, Ok(Module { symbols: s, .. }) if s.len() == 1 && matches!(&s[0], TypeScriptSymbol::Namespace { name, content, jsdoc: None, .. } if name == "Foo" && content.len() == 2));
        }

        #[test]
        fn namespace_with_jsdoc() {
            let mut parser = make_parser();
            let content =
                "/** Utility functions */\nnamespace Foo { declare const VERSION: string; }";

            let result = parse_typescript_file(content, &mut parser);

            assert_matches!(result, Ok(Module { symbols: s, .. }) if s.len() == 1 && matches!(&s[0], TypeScriptSymbol::Namespace { name, jsdoc: Some(j), .. } if name == "Foo" && j == "/** Utility functions */"));
        }

        #[test]
        fn namespace_without_jsdoc() {
            let mut parser = make_parser();
            let content = "namespace Foo { declare const VERSION: string; }";

            let result = parse_typescript_file(content, &mut parser);

            assert_matches!(result, Ok(Module { symbols: s, .. }) if s.len() == 1 && matches!(&s[0], TypeScriptSymbol::Namespace { name, jsdoc: None, .. } if name == "Foo"));
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
    }

    mod module_exports {
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
