use std::collections::HashMap;

use daipendency_extractor::{ExtractionError, Symbol};
use streaming_iterator::StreamingIterator;
use tree_sitter::QueryMatches;
use tree_sitter::{Language, Node, Parser, Query, QueryCursor};
use tree_sitter_typescript::LANGUAGE_TYPESCRIPT;

use crate::api::module::{ExportTarget, ImportTarget, Module, TypeScriptSymbol};

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

    let jsdoc = get_module_jsdoc(node, content);
    let (symbols, default_export_name) = get_module_symbols(node, content);

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

fn get_module_jsdoc<'a>(node: Node<'a>, content: &'a str) -> Option<String> {
    get_jsdoc(node.child(0), content).filter(|comment_text| {
        comment_text.contains("@file")
            || comment_text.contains("@fileoverview")
            || comment_text.contains("@module")
    })
}

/// Extracts all symbols from the module.
///
/// # Arguments
///
/// * `node` - The root node of the TypeScript AST
/// * `content` - The source code content as a string
///
/// # Returns
///
/// A tuple containing:
/// * A vector of all symbols found in the module
/// * The name of the default export if one exists, otherwise None
fn get_module_symbols(root: Node, content: &str) -> (Vec<TypeScriptSymbol>, Option<String>) {
    // TODO: REMOVE WHEN ALL TESTS ARE FIXED
    #[cfg(test)]
    println!("root: {}", debug_node(&root, content));

    let mut symbols = vec![];
    let default_export_name = None;
    let mut cursor = QueryCursor::new();
    let query =
        Query::new(&LANGUAGE_TYPESCRIPT.into(), SYMBOLS_QUERY).expect("Failed to create query");

    let capture_names = query.capture_names();
    let name_index = capture_names
        .iter()
        .position(|name| *name == "name")
        .expect("Name capture not found") as u32;
    let definition_index = capture_names
        .iter()
        .position(|name| *name == "declaration")
        .expect("Declaration capture not found") as u32;

    let matches = cursor.matches(&query, root, content.as_bytes());
    matches.for_each(|match_| {
        let name_node = match_.nodes_for_capture_index(name_index).next().unwrap();
        let mut definition_node = match_
            .nodes_for_capture_index(definition_index)
            .next()
            .unwrap();

        let name = name_node.utf8_text(content.as_bytes()).unwrap().to_string();

        if let Some(parent) = definition_node.parent() {
            if parent.kind() == "ambient_declaration" {
                definition_node = parent;
            }
        }

        // Get the full source code including any preceding comments
        let mut start_byte = definition_node.start_byte();
        let end_byte = definition_node.end_byte();

        // Check if there's a preceding comment
        if let Some(prev) = definition_node.prev_sibling() {
            if prev.kind() == "comment" {
                start_byte = prev.start_byte();
            }
        }

        // Get the source code
        let source_code = content[start_byte..end_byte].to_string();

        let symbol = Symbol { name, source_code };

        symbols.push(TypeScriptSymbol::Symbol {
            symbol,
            exported: false,
        });
    });

    (symbols, default_export_name)
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
        fn abstract_class_declaration() {
            let mut parser = make_parser();
            let content = "declare abstract class Foo { bar(): void; }";

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
