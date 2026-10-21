//! Convenience queries over a syntax tree.

use crate::ast::{
    Expr, ExprCall, ExprName, Module, Stmt, StmtClassDef, StmtFunctionDef, StmtImport,
    StmtImportFrom,
};
use crate::visitor::{Visitor, walk_expr, walk_stmt};

impl Module {
    /// Every `Name` node, in source order, including assignment targets and
    /// parameters.
    pub fn names(&self) -> Vec<&ExprName> {
        let mut collector = Collector::default();
        collector.visit_module(self);
        collector.names
    }

    /// Every call, in source order (outer calls before the calls in their
    /// arguments).
    pub fn calls(&self) -> Vec<&ExprCall> {
        let mut collector = Collector::default();
        collector.visit_module(self);
        collector.calls
    }

    /// Every function definition, including nested ones and methods.
    pub fn function_defs(&self) -> Vec<&StmtFunctionDef> {
        let mut collector = Collector::default();
        collector.visit_module(self);
        collector.functions
    }

    /// Every class definition, including nested ones.
    pub fn class_defs(&self) -> Vec<&StmtClassDef> {
        let mut collector = Collector::default();
        collector.visit_module(self);
        collector.classes
    }

    /// Every `import` and `from ... import` statement, including those
    /// inside functions.
    pub fn imports(&self) -> Vec<Import<'_>> {
        let mut collector = Collector::default();
        collector.visit_module(self);
        collector.imports
    }
}

/// An import statement.
#[derive(Clone, Copy, Debug)]
pub enum Import<'a> {
    Import(&'a StmtImport),
    ImportFrom(&'a StmtImportFrom),
}

impl Expr {
    /// The dotted name of a `Name` or of a chain of attribute accesses on a
    /// name: `system.tag.readBlocking` for `system.tag.readBlocking`.
    pub fn dotted_name(&self) -> Option<String> {
        match self {
            Expr::Name(name) => Some(name.id.to_string()),
            Expr::Attribute(attribute) => {
                let mut dotted = attribute.value.dotted_name()?;
                dotted.push('.');
                dotted.push_str(&attribute.attr.id);
                Some(dotted)
            }
            _ => None,
        }
    }
}

impl ExprCall {
    /// The dotted name of the called function, if it is a name or an
    /// attribute chain on a name.
    pub fn callee_name(&self) -> Option<String> {
        self.func.dotted_name()
    }
}

#[derive(Default)]
struct Collector<'a> {
    names: Vec<&'a ExprName>,
    calls: Vec<&'a ExprCall>,
    functions: Vec<&'a StmtFunctionDef>,
    classes: Vec<&'a StmtClassDef>,
    imports: Vec<Import<'a>>,
}

impl<'a> Visitor<'a> for Collector<'a> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::FunctionDef(function) => self.functions.push(function),
            Stmt::ClassDef(class) => self.classes.push(class),
            Stmt::Import(import) => self.imports.push(Import::Import(import)),
            Stmt::ImportFrom(import) => self.imports.push(Import::ImportFrom(import)),
            _ => {}
        }
        walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        match expr {
            Expr::Name(name) => self.names.push(name),
            Expr::Call(call) => self.calls.push(call),
            _ => {}
        }
        walk_expr(self, expr);
    }
}

#[cfg(test)]
mod tests {
    use crate::parse_module;

    #[test]
    fn queries() {
        let source = "import system\n\
                      def f(a, (b, c)):\n    \
                          class C:\n        \
                              def g(self):\n            \
                                  return system.tag.readBlocking([a])\n";
        let parsed = parse_module(source);
        let module = &parsed.module;
        let functions: Vec<&str> = module
            .function_defs()
            .iter()
            .map(|function| function.name.as_str())
            .collect();
        assert_eq!(functions, ["f", "g"]);
        assert_eq!(module.class_defs().len(), 1);
        assert_eq!(module.imports().len(), 1);
        let calls: Vec<String> = module
            .calls()
            .iter()
            .filter_map(|call| call.callee_name())
            .collect();
        assert_eq!(calls, ["system.tag.readBlocking"]);
        let names: Vec<&str> = module.names().iter().map(|name| &*name.id).collect();
        assert_eq!(names, ["a", "b", "c", "self", "system", "a"]);
    }
}
