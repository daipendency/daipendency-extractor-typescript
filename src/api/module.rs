use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use daipendency_extractor::Symbol;

/// A TypeScript module (i.e. a file).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Module {
    pub path: PathBuf,
    pub jsdoc: Option<String>,
    pub symbols: Vec<TypeScriptSymbol>,
    pub default_export_name: Option<String>,
}

/// The target of an import in a TypeScript module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportTarget {
    /// The default export from another module (e.g. `import React from 'react';`).
    Default {
        /// The name of the default export (e.g. `React` in `import React from 'react';`).
        name: String,
    },
    /// A namespace import from another module (e.g. `import * as React from 'react';`).
    Namespace {
        /// The name of the namespace (e.g. `React` in `import * as React from 'react';`).
        name: String,
    },
    /// A named import from another module (e.g. `import { useState } from 'react';`).
    Named {
        /// The names of the symbols to import (e.g. `useState` in `import { useState } from 'react';`).
        names: Vec<String>,
        /// The aliases for the imported symbols (e.g. `useState: foo` in `import { useState as foo } from 'react';`).
        aliases: HashMap<String, String>,
    },
}

impl Hash for ImportTarget {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            ImportTarget::Default { name } => {
                0.hash(state);
                name.hash(state);
            }
            ImportTarget::Namespace { name } => {
                1.hash(state);
                name.hash(state);
            }
            ImportTarget::Named { names, .. } => {
                2.hash(state);
                names.hash(state);
                // Skip aliases in hash calculation as HashMap doesn't implement Hash
            }
        }
    }
}

/// The target of an export in a TypeScript module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportTarget {
    /// A namespace export from another module (e.g. `export * as React from 'react';`).
    Namespace {
        /// The name of the namespace (e.g. `React` in `export * as React from 'react';`).
        name: String,
    },
    /// A named export from another module (e.g. `export { useState } from 'react';`).
    Named {
        /// The names of the symbols to export (e.g. `useState` in `export { useState } from 'react';`).
        names: Vec<String>,
        /// The aliases for the exported symbols (e.g. `useState: foo` in `export { useState as foo } from 'react';`).
        aliases: HashMap<String, String>,
    },
    /// A barrel export from another module (e.g. `export * from './module.js';`).
    Barrel,
}

impl Hash for ExportTarget {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            ExportTarget::Namespace { name } => {
                0.hash(state);
                name.hash(state);
            }
            ExportTarget::Named { names, .. } => {
                1.hash(state);
                names.hash(state);
                // Skip aliases in hash calculation as HashMap doesn't implement Hash
            }
            ExportTarget::Barrel => {
                2.hash(state);
            }
        }
    }
}

/// A symbol in a TypeScript module.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TypeScriptSymbol {
    /// A symbol (e.g. class, interface, function, constant, type alias).
    Symbol {
        symbol: Symbol,
        /// Whether the symbol was exported when declared.
        is_exported: bool,
    },
    /// A TypeScript namespace.
    Namespace {
        name: String,
        jsdoc: Option<String>,
        content: Vec<TypeScriptSymbol>,
        /// Whether the symbol was exported when declared.
        is_exported: bool,
    },
    /// An import from another module (e.g. `import Foo from './foo.js';`).
    ///
    /// If a single `import` statement uses multiple types of targets, it will be represented as multiple `ModuleImport` symbols.
    ModuleImport {
        /// The module from which symbols are imported (e.g. `./foo.js` in `import Foo from './foo.js';`).
        source_module: String,
        /// The target of the import (e.g. `Foo` in `import Foo from './foo.js';`).
        target: ImportTarget,
    },
    /// An export from another module (e.g. `export Foo from './foo.js';`).
    ///
    /// If a single `export` statement uses multiple types of targets, it will be represented as multiple `ModuleExport` symbols.
    ModuleExport {
        /// The module from which symbols are exported (e.g. `./foo.js` in `export Foo from './foo.js';`).
        ///
        /// The source is `None` when the symbol was previously declared or imported in the current file.
        source_module: Option<String>,
        /// The target of the export (e.g. `Foo` in `export Foo from './foo.js';`).
        target: ExportTarget,
    },
}
