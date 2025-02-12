#![cfg(test)]

use crate::TypeScriptExtractor;
use daipendency_extractor::Extractor;
use tree_sitter::Parser;

pub fn make_parser() -> Parser {
    let mut parser = Parser::new();
    let language = TypeScriptExtractor.get_parser_language();
    parser.set_language(&language).unwrap();
    parser
}
