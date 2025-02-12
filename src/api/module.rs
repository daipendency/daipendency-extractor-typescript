use std::collections::HashMap;

use daipendency_extractor::Symbol;

/// A TypeScript module (i.e. a file).
#[derive(Debug, Clone)]
pub struct Module {
    pub jsdoc: Option<String>,
    pub symbols: Vec<TypeScriptSymbol>,
    pub default_export_name: Option<String>,
}

/// The target of an import in a TypeScript module.
#[derive(Debug, Clone, PartialEq)]
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

/// The target of an export in a TypeScript module.
#[derive(Debug, Clone, PartialEq)]
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

/// A symbol in a TypeScript module.
#[derive(Debug, Clone, PartialEq)]
pub enum TypeScriptSymbol {
    /// A symbol (e.g. class, interface, function, constant, type alias).
    Symbol {
        symbol: Symbol,
        /// Whether the symbol is exported (either when declared, or later in the file).
        exported: bool,
    },
    /// A TypeScript namespace.
    Namespace {
        name: String,
        jsdoc: Option<String>,
        content: Vec<TypeScriptSymbol>,
        /// Whether the symbol is exported (either when declared, or later in the file).
        exported: bool,
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
        source_module: String,
        /// The target of the export (e.g. `Foo` in `export Foo from './foo.js';`).
        target: ExportTarget,
    },
}
