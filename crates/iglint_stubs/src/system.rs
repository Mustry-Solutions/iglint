//! The `system.*` scripting API.

use std::fmt;

use crate::{Scope, Scopes, generated};

/// A `system.*` package such as `system.db`.
#[derive(Debug)]
pub struct Package {
    pub path: &'static str,
    pub scopes: Scopes,
}

/// A `system.*` function. The same path can behave differently per scope
/// (`system.util.getSessionInfo` in the gateway and in a Vision Client), so a
/// function has one [`FunctionVariant`] per group of scopes with identical
/// facts.
#[derive(Debug)]
pub struct Function {
    /// Full path, e.g. `system.db.runPrepQuery`.
    pub path: &'static str,
    pub variants: &'static [FunctionVariant],
}

impl Function {
    /// The last path segment, e.g. `runPrepQuery`.
    pub fn name(&self) -> &'static str {
        self.path.rsplit('.').next().unwrap_or(self.path)
    }

    /// The package path, e.g. `system.db`.
    pub fn package(&self) -> &'static str {
        self.path.rsplit_once('.').map_or("", |(p, _)| p)
    }

    /// Scopes in which the function exists.
    pub fn scopes(&self) -> Scopes {
        self.variants
            .iter()
            .fold(Scopes::NONE, |acc, v| acc.union(v.scopes))
    }

    /// The facts that apply in `scope`, if the function exists there.
    pub fn variant(&self, scope: Scope) -> Option<&'static FunctionVariant> {
        self.variants.iter().find(|v| v.scopes.contains(scope))
    }

    /// Whether the function is deprecated in every scope it exists in.
    pub fn is_deprecated(&self) -> bool {
        self.variants.iter().all(|v| v.deprecated)
    }

    /// The replacement Ignition names for a deprecated function, e.g.
    /// `system.historian.queryRawPoints`. Can list several, comma separated.
    pub fn replacement(&self) -> Option<&'static str> {
        self.variants.iter().find_map(|v| v.replacement)
    }

    /// Checks a call in `scope`; see [`check_call`].
    pub fn check_call<'k>(
        &self,
        scope: Scope,
        positional_count: usize,
        keyword_names: &[&'k str],
    ) -> Result<(), ArityError<'k>> {
        match self.variant(scope) {
            Some(v) => check_call(v, positional_count, keyword_names),
            None => Err(ArityError::NotInScope),
        }
    }

    /// Link to the official documentation page of this function.
    pub fn docs_url(&self) -> String {
        format!(
            "https://www.docs.inductiveautomation.com/docs/8.3/appendix/scripting-functions/{}/{}",
            self.package().replace('.', "-"),
            self.path.replace('.', "-")
        )
    }
}

/// Facts about a function in a set of scopes.
#[derive(Debug)]
pub struct FunctionVariant {
    pub scopes: Scopes,
    /// Marked deprecated by Ignition (Java `@Deprecated` or a deprecation in
    /// the Designer's completion data).
    pub deprecated: bool,
    /// What Ignition suggests instead, when it says so.
    pub replacement: Option<&'static str>,
    /// Hidden from the Designer's autocompletion (`@NoHint`). Hidden
    /// functions still work.
    pub hidden: bool,
    /// The ways the function can be called. A call is valid when it matches
    /// any overload.
    pub overloads: &'static [Overload],
}

impl FunctionVariant {
    /// The keyword-capable overload, if the function accepts keywords.
    pub fn keyword_overload(&self) -> Option<&'static Overload> {
        self.overloads
            .iter()
            .find(|o| matches!(o.kind, OverloadKind::Keyword))
    }
}

/// How an overload binds its arguments.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverloadKind {
    /// A plain Java method called through Jython: exactly `params.len()`
    /// positional arguments, no keywords.
    Positional,
    /// A Java method taking `(PyObject[], String[])` with `@KeywordArgs`:
    /// positional arguments fill `params` in order, keywords must be one of
    /// the parameter names, required parameters must be present.
    Keyword,
    /// A `(PyObject[], String[])` method without declared names. Accepts any
    /// call as far as the stubs know.
    Unchecked,
    /// A Python function.
    Python { varargs: bool, varkw: bool },
}

/// One callable signature.
#[derive(Debug)]
pub struct Overload {
    pub kind: OverloadKind,
    pub params: &'static [Param],
    /// Return type as Ignition names it (`str`, `Dataset`, `PyDataset`, a
    /// Java class name, ...).
    pub returns: Option<&'static str>,
}

/// A parameter.
#[derive(Debug)]
pub struct Param {
    /// `None` when Ignition does not name the parameter of this overload.
    pub name: Option<&'static str>,
    /// Type as Ignition names it (`str`, `list`, `Date`, `any`, a Java class
    /// name, ...).
    pub ty: Option<&'static str>,
    /// Whether a call must supply it. For keyword functions this was
    /// measured on the gateway: a parameter is required when calling without
    /// it fails because it is missing. Parameters that are only checked later
    /// are reported as optional.
    pub required: bool,
    /// Default as Ignition documents it in the Designer, as source text.
    pub default: Option<&'static str>,
}

/// A constant such as `system.db.INTEGER`.
#[derive(Debug)]
pub struct Constant {
    pub path: &'static str,
    pub variants: &'static [ConstantVariant],
}

impl Constant {
    /// The value in `scope`, if the constant exists there.
    pub fn value(&self, scope: Scope) -> Option<&'static ConstValue> {
        self.variants
            .iter()
            .find(|v| v.scopes.contains(scope))
            .map(|v| &v.value)
    }

    /// Scopes in which the constant exists.
    pub fn scopes(&self) -> Scopes {
        self.variants
            .iter()
            .fold(Scopes::NONE, |acc, v| acc.union(v.scopes))
    }
}

/// A constant's value in a set of scopes.
#[derive(Debug)]
pub struct ConstantVariant {
    pub scopes: Scopes,
    pub value: ConstValue,
}

/// The value of a constant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ConstValue {
    Int(i64),
    Float(f64),
    Str(&'static str),
    Bool(bool),
}

/// A `system.*` member that is neither a function, a constant nor a
/// package: an object (`system.net.httpClient`, `system.util.globals`) or a
/// class (`system.bacnet.enumerated.ObjectType`).
#[derive(Debug)]
pub struct ScriptObject {
    pub path: &'static str,
    pub variants: &'static [ObjectVariant],
}

impl ScriptObject {
    /// Scopes in which the member exists.
    pub fn scopes(&self) -> Scopes {
        self.variants
            .iter()
            .fold(Scopes::NONE, |acc, v| acc.union(v.scopes))
    }
}

/// Facts about a [`ScriptObject`] in a set of scopes.
#[derive(Debug)]
pub struct ObjectVariant {
    pub scopes: Scopes,
    pub kind: ObjectKind,
    /// Type as Ignition names it, when known.
    pub ty: Option<&'static str>,
}

/// Whether a [`ScriptObject`] is an instance or a class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectKind {
    Instance,
    Class,
}

/// Any `system.*` member.
#[derive(Clone, Copy, Debug)]
pub enum Member {
    Package(&'static Package),
    Function(&'static Function),
    Constant(&'static Constant),
    Object(&'static ScriptObject),
}

impl Member {
    /// Full path of the member.
    pub fn path(&self) -> &'static str {
        match self {
            Member::Package(p) => p.path,
            Member::Function(f) => f.path,
            Member::Constant(c) => c.path,
            Member::Object(o) => o.path,
        }
    }

    /// Scopes in which the member exists.
    pub fn scopes(&self) -> Scopes {
        match self {
            Member::Package(p) => p.scopes,
            Member::Function(f) => f.scopes(),
            Member::Constant(c) => c.scopes(),
            Member::Object(o) => o.scopes(),
        }
    }
}

fn find<T>(table: &'static [T], path: &str, key: impl Fn(&T) -> &str) -> Option<&'static T> {
    table
        .binary_search_by(|e| key(e).cmp(path))
        .ok()
        .map(|i| &table[i])
}

/// Looks up a function by full path, e.g. `system.tag.readBlocking`.
pub fn system_function(path: &str) -> Option<&'static Function> {
    find(generated::FUNCTIONS, path, |f| f.path)
}

/// Looks up a constant by full path, e.g. `system.db.INTEGER`.
pub fn system_constant(path: &str) -> Option<&'static Constant> {
    find(generated::CONSTANTS, path, |c| c.path)
}

/// Looks up an object or class member by full path.
pub fn system_object(path: &str) -> Option<&'static ScriptObject> {
    find(generated::OBJECTS, path, |o| o.path)
}

/// Looks up a package by full path, e.g. `system.perspective`.
pub fn system_package(path: &str) -> Option<&'static Package> {
    find(generated::PACKAGES, path, |p| p.path)
}

/// All `system.*` packages, sorted by path.
pub fn system_packages() -> &'static [Package] {
    generated::PACKAGES
}

/// Whether the `system.*` chain `path` is one of `listed` or a member read
/// from one (`system.mes.getThing` lists `system.mes.getThing` and
/// `system.mes.getThing.x`, not `system.mes.getThings`).
pub fn system_path_listed(path: &str, listed: &[String]) -> bool {
    listed.iter().any(|l| {
        path.strip_prefix(l.as_str())
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
    })
}

/// Looks up any member by full path.
pub fn system_member(path: &str) -> Option<Member> {
    if let Some(f) = system_function(path) {
        return Some(Member::Function(f));
    }
    if let Some(p) = system_package(path) {
        return Some(Member::Package(p));
    }
    if let Some(c) = system_constant(path) {
        return Some(Member::Constant(c));
    }
    system_object(path).map(Member::Object)
}

fn children<T>(
    table: &'static [T],
    package: &str,
    key: fn(&T) -> &str,
) -> impl Iterator<Item = &'static T> {
    let prefix = format!("{package}.");
    let start = table.partition_point(|e| key(e) < prefix.as_str());
    let len = prefix.len();
    table[start..]
        .iter()
        .take_while(move |e| key(e).starts_with(prefix.as_str()))
        // Every key here starts with `prefix`, so `len` is a boundary.
        .filter(move |e| !key(e).get(len..).unwrap_or_default().contains('.'))
}

/// The direct members of a package (`"system"` for the top level), in path
/// order per kind: packages, functions, constants, objects.
pub fn system_members(package: &str) -> impl Iterator<Item = Member> {
    children(generated::PACKAGES, package, |p| p.path)
        .map(Member::Package)
        .chain(children(generated::FUNCTIONS, package, |f| f.path).map(Member::Function))
        .chain(children(generated::CONSTANTS, package, |c| c.path).map(Member::Constant))
        .chain(children(generated::OBJECTS, package, |o| o.path).map(Member::Object))
}

/// Why a call does not match any overload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArityError<'k> {
    /// The function does not exist in the requested scope.
    NotInScope,
    /// A plain Java function was called with keyword arguments; Jython
    /// rejects that ("takes no keyword arguments").
    KeywordsNotAccepted { keyword: &'k str },
    /// The positional argument count matches no overload.
    PositionalCount {
        given: usize,
        min: usize,
        max: usize,
    },
    /// More positional arguments than parameters.
    TooManyPositional { given: usize, max: usize },
    /// A keyword that is not a parameter name.
    UnknownKeyword { keyword: &'k str },
    /// A parameter given both positionally and as a keyword, or twice.
    DuplicateArgument { name: &'k str },
    /// A required parameter is missing.
    MissingRequired { name: &'static str },
}

impl fmt::Display for ArityError<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArityError::NotInScope => write!(f, "not available in this scope"),
            ArityError::KeywordsNotAccepted { keyword } => {
                write!(f, "takes no keyword arguments (got `{keyword}`)")
            }
            ArityError::PositionalCount { given, min, max } if min == max => {
                write!(f, "expected {min} arguments, got {given}")
            }
            ArityError::PositionalCount { given, min, max } => {
                write!(f, "expected {min} to {max} arguments, got {given}")
            }
            ArityError::TooManyPositional { given, max } => {
                write!(f, "expected at most {max} arguments, got {given}")
            }
            ArityError::UnknownKeyword { keyword } => {
                write!(f, "unexpected keyword argument `{keyword}`")
            }
            ArityError::DuplicateArgument { name } => {
                write!(f, "got multiple values for argument `{name}`")
            }
            ArityError::MissingRequired { name } => {
                write!(f, "missing required argument `{name}`")
            }
        }
    }
}

impl std::error::Error for ArityError<'_> {}

/// Checks a call with `positional_count` positional arguments and the given
/// keyword names against the overloads of `variant`.
///
/// Only call sites without `*args` / `**kwargs` unpacking can be checked;
/// skip the others.
pub fn check_call<'k>(
    variant: &FunctionVariant,
    positional_count: usize,
    keyword_names: &[&'k str],
) -> Result<(), ArityError<'k>> {
    let mut keyword_error = None;
    let mut positional_counts: Option<(usize, usize)> = None;
    for overload in variant.overloads {
        let result = match overload.kind {
            OverloadKind::Unchecked => Ok(()),
            OverloadKind::Positional => {
                let n = overload.params.len();
                positional_counts = Some(match positional_counts {
                    Some((lo, hi)) => (lo.min(n), hi.max(n)),
                    None => (n, n),
                });
                match keyword_names.first() {
                    Some(&keyword) => Err(ArityError::KeywordsNotAccepted { keyword }),
                    None if positional_count == n => Ok(()),
                    None => Err(ArityError::PositionalCount {
                        given: positional_count,
                        min: n,
                        max: n,
                    }),
                }
            }
            OverloadKind::Keyword => {
                let r = check_named(overload, positional_count, keyword_names, false, false);
                if r.is_err() {
                    keyword_error = Some(r.clone());
                }
                r
            }
            OverloadKind::Python { varargs, varkw } => {
                let r = check_named(overload, positional_count, keyword_names, varargs, varkw);
                if r.is_err() {
                    keyword_error = Some(r.clone());
                }
                r
            }
        };
        if result.is_ok() {
            return Ok(());
        }
    }
    if let Some(err) = keyword_error {
        return err;
    }
    match (keyword_names.first(), positional_counts) {
        (Some(&keyword), _) => Err(ArityError::KeywordsNotAccepted { keyword }),
        (None, Some((min, max))) => Err(ArityError::PositionalCount {
            given: positional_count,
            min,
            max,
        }),
        (None, None) => Ok(()),
    }
}

fn check_named<'k>(
    overload: &Overload,
    positional_count: usize,
    keyword_names: &[&'k str],
    varargs: bool,
    varkw: bool,
) -> Result<(), ArityError<'k>> {
    let params = overload.params;
    if positional_count > params.len() && !varargs {
        return Err(ArityError::TooManyPositional {
            given: positional_count,
            max: params.len(),
        });
    }
    for (i, &keyword) in keyword_names.iter().enumerate() {
        if keyword_names[..i].contains(&keyword) {
            return Err(ArityError::DuplicateArgument { name: keyword });
        }
        match params.iter().position(|p| p.name == Some(keyword)) {
            Some(index) if index < positional_count => {
                return Err(ArityError::DuplicateArgument { name: keyword });
            }
            Some(_) => {}
            None if varkw => {}
            None => return Err(ArityError::UnknownKeyword { keyword }),
        }
    }
    for (index, p) in params.iter().enumerate() {
        if !p.required || index < positional_count {
            continue;
        }
        let Some(name) = p.name else { continue };
        if !keyword_names.contains(&name) {
            return Err(ArityError::MissingRequired { name });
        }
    }
    Ok(())
}
