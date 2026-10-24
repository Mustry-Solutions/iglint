use crate::*;

fn function(path: &str) -> &'static Function {
    system_function(path).unwrap_or_else(|| panic!("{path} missing"))
}

#[test]
fn version_tags() {
    assert_eq!(IGNITION_VERSION, "8.3.3");
    assert!(IGNITION_BUILD.starts_with("8.3.3"));
    assert_eq!(JYTHON_VERSION, "2.7.3");
}

#[test]
fn tables_are_sorted_and_unique() {
    fn check<T>(table: &[T], key: impl Fn(&T) -> &str) {
        for pair in table.windows(2) {
            assert!(
                key(&pair[0]) < key(&pair[1]),
                "{} >= {}",
                key(&pair[0]),
                key(&pair[1])
            );
        }
    }
    check(generated::PACKAGES, |p| p.path);
    check(generated::FUNCTIONS, |f| f.path);
    check(generated::CONSTANTS, |c| c.path);
    check(generated::OBJECTS, |o| o.path);
    check(generated::EXPRESSION_FUNCTIONS, |f| f.key);
    check(generated::COMPONENT_TYPES, |c| c.id);
    check(generated::SHARED_SCHEMAS, |s| s.0);
    check(generated::BUILTINS, |b| b.name);
    for f in generated::EXPRESSION_FUNCTIONS {
        assert_eq!(f.key, f.name.to_ascii_lowercase());
    }
}

#[test]
fn counts() {
    assert!(generated::FUNCTIONS.len() > 650);
    assert!(generated::EXPRESSION_FUNCTIONS.len() > 140);
    assert!(generated::COMPONENT_TYPES.len() > 80);
}

#[test]
fn every_function_has_overloads_and_scopes() {
    for f in generated::FUNCTIONS {
        assert!(!f.variants.is_empty(), "{}", f.path);
        let mut seen = Scopes::NONE;
        for v in f.variants {
            assert!(!v.overloads.is_empty(), "{}", f.path);
            assert!(!v.scopes.is_empty(), "{}", f.path);
            assert!(seen.union(v.scopes) != seen, "{} variants overlap", f.path);
            seen = seen.union(v.scopes);
        }
    }
}

#[test]
fn scopes() {
    let navigate = function("system.perspective.navigate");
    assert!(navigate.scopes().contains(Scope::Gateway));
    assert!(!navigate.scopes().contains(Scope::Client));
    let message_box = function("system.gui.messageBox");
    assert!(!message_box.scopes().contains(Scope::Gateway));
    assert!(message_box.scopes().contains(Scope::Client));
    assert!(message_box.scopes().contains(Scope::Designer));
    assert_eq!(function("system.tag.readBlocking").scopes(), Scopes::ALL);
    assert_eq!(
        navigate.check_call(Scope::Client, 0, &[]),
        Err(ArityError::NotInScope)
    );
}

#[test]
fn names() {
    let f = function("system.db.runPrepQuery");
    assert_eq!(f.name(), "runPrepQuery");
    assert_eq!(f.package(), "system.db");
    assert_eq!(
        f.docs_url(),
        "https://www.docs.inductiveautomation.com/docs/8.3/appendix/scripting-functions/system-db/system-db-runPrepQuery"
    );
}

#[test]
fn keyword_function() {
    let f = function("system.tag.readBlocking");
    let v = f.variant(Scope::Gateway).unwrap();
    let o = v.keyword_overload().unwrap();
    let names: Vec<_> = o.params.iter().map(|p| p.name.unwrap()).collect();
    assert_eq!(names, ["tagPaths", "timeout"]);
    assert!(o.params[0].required);
    assert!(!o.params[1].required);

    assert_eq!(check_call(v, 1, &[]), Ok(()));
    assert_eq!(check_call(v, 2, &[]), Ok(()));
    assert_eq!(check_call(v, 1, &["timeout"]), Ok(()));
    assert_eq!(check_call(v, 0, &["tagPaths", "timeout"]), Ok(()));
    assert_eq!(
        check_call(v, 0, &[]),
        Err(ArityError::MissingRequired { name: "tagPaths" })
    );
    assert_eq!(
        check_call(v, 3, &[]),
        Err(ArityError::TooManyPositional { given: 3, max: 2 })
    );
    assert_eq!(
        check_call(v, 1, &["bogus"]),
        Err(ArityError::UnknownKeyword { keyword: "bogus" })
    );
    assert_eq!(
        check_call(v, 1, &["tagPaths"]),
        Err(ArityError::DuplicateArgument { name: "tagPaths" })
    );
}

#[test]
fn positional_function() {
    let f = function("system.date.addDays");
    assert_eq!(f.check_call(Scope::Gateway, 2, &[]), Ok(()));
    assert_eq!(
        f.check_call(Scope::Gateway, 1, &[]),
        Err(ArityError::PositionalCount {
            given: 1,
            min: 2,
            max: 2
        })
    );
    assert_eq!(
        f.check_call(Scope::Gateway, 1, &["days"]),
        Err(ArityError::KeywordsNotAccepted { keyword: "days" })
    );
    let encode = function("system.util.jsonEncode");
    assert_eq!(encode.check_call(Scope::Gateway, 1, &[]), Ok(()));
    assert_eq!(encode.check_call(Scope::Gateway, 2, &[]), Ok(()));
    assert_eq!(
        encode.check_call(Scope::Gateway, 0, &[]),
        Err(ArityError::PositionalCount {
            given: 0,
            min: 1,
            max: 2
        })
    );
    assert_eq!(
        encode
            .check_call(Scope::Gateway, 0, &[])
            .unwrap_err()
            .to_string(),
        "expected 1 to 2 arguments, got 0"
    );
}

#[test]
fn shorter_overloads_are_named_from_longer_ones() {
    let v = function("system.gui.messageBox")
        .variant(Scope::Client)
        .unwrap();
    assert_eq!(v.overloads[0].params[0].name, Some("message"));
}

#[test]
fn mixed_function_accepts_positional_and_keywords() {
    let f = function("system.util.getSessionInfo");
    assert_eq!(f.check_call(Scope::Gateway, 0, &[]), Ok(()));
    assert_eq!(f.check_call(Scope::Gateway, 2, &[]), Ok(()));
    assert_eq!(f.check_call(Scope::Gateway, 0, &["projectFilter"]), Ok(()));
    assert_eq!(
        f.check_call(Scope::Gateway, 3, &[]),
        Err(ArityError::TooManyPositional { given: 3, max: 2 })
    );
}

#[test]
fn deprecation() {
    let f = function("system.tag.queryTagHistory");
    assert!(f.is_deprecated());
    assert_eq!(f.replacement(), Some("system.historian.queryRawPoints"));
    assert!(function("system.db.runQuery").is_deprecated());
    assert!(!function("system.tag.readBlocking").is_deprecated());
    // Java @Deprecated without a Designer hint.
    assert!(function("system.tag.read").is_deprecated());
}

#[test]
fn constants_and_members() {
    let c = system_constant("system.db.INTEGER").unwrap();
    assert_eq!(c.value(Scope::Gateway), Some(&ConstValue::Int(4)));
    let names: Vec<_> = system_members("system.db").map(|m| m.path()).collect();
    assert!(names.contains(&"system.db.runPrepQuery"));
    assert!(names.contains(&"system.db.INTEGER"));
    assert!(
        !names
            .iter()
            .any(|n| n.strip_prefix("system.db.").unwrap().contains('.'))
    );

    let top: Vec<_> = system_members("system").map(|m| m.path()).collect();
    for p in [
        "system.db",
        "system.tag",
        "system.perspective",
        "system.vision",
    ] {
        assert!(top.contains(&p), "{p}");
    }
    assert!(matches!(
        system_member("system.tag"),
        Some(Member::Package(_))
    ));
    assert!(matches!(
        system_member("system.net.httpClient"),
        Some(Member::Object(_))
    ));
    assert!(system_member("system.nope").is_none());
    assert!(system_package("system.perspective").is_some());
}

#[test]
fn expression_functions_lookup_is_case_insensitive() {
    let f = expression_function("DATEFORMAT").unwrap();
    assert_eq!(f.name, "dateFormat");
    assert!(f.accepts_arg_count(2));
    assert!(!f.accepts_arg_count(1));
    assert!(expression_function("dateformat").is_some());
    assert!(expression_function("noSuchFunction").is_none());
    assert_eq!(
        f.docs_url(),
        "https://www.docs.inductiveautomation.com/docs/8.3/appendix/expression-functions/date-and-time/dateFormat"
    );
}

#[test]
fn expression_arity() {
    let case = expression_function("case").unwrap();
    assert!(case.accepts_arg_count(4));
    assert!(!case.accepts_arg_count(5));
    assert!(case.accepts_arg_count(100));
    let coalesce = expression_function("coalesce").unwrap();
    assert_eq!(
        coalesce.arity,
        Some(Arity {
            min: 0,
            max: None,
            step: 1
        })
    );
    let round = expression_function("round").unwrap();
    assert_eq!(
        round.arity,
        Some(Arity {
            min: 1,
            max: Some(2),
            step: 1
        })
    );
    let if_ = expression_function("if").unwrap();
    assert!(if_.accepts_arg_count(3));
    assert!(!if_.accepts_arg_count(2));
    assert!(
        expression_function("runScript")
            .unwrap()
            .accepts_arg_count(1)
    );
}

#[test]
fn component_type_lookup() {
    let label = component_type("ia.display.label").unwrap();
    assert_eq!(label.name, "Label");
    let text = label.prop("text").unwrap();
    assert!(text.types.contains(TypeSet::STRING));
    assert_eq!(text.default, Some("\"\""));
    let align = label.prop("alignVertical").unwrap();
    assert!(align.enum_values.contains(&"\"center\""));
    // Through a shared schema reference.
    assert!(label.prop("style.classes").is_some());
    assert!(label.prop("nope").is_none());

    let table = component_type("ia.display.table").unwrap();
    assert!(table.prop("columns[0].field").is_some());
    assert!(table.prop("columns.0.field").is_some());
    assert!(table.event("onRowClick").is_some());

    let flex = component_type("ia.container.flex").unwrap();
    assert!(flex.child_position_prop("grow").is_some());
    assert!(component_type("ia.display.nope").is_none());
    assert!(meta_prop("name").is_some());
}

#[test]
fn all_references_resolve() {
    fn visit(root: &'static Schema, node: &'static Schema, path: &str) {
        if let Some(r) = node.reference {
            let resolved = if let Some(rest) = r.strip_prefix(SHARED_SCHEMA_PREFIX) {
                shared_schema(rest.split('#').next().unwrap()).is_some()
            } else {
                // Same-document pointers are resolved by prop lookups; make
                // sure the definition exists for the common form.
                match r.strip_prefix("#/definitions/") {
                    Some(name) => root.definition(name).is_some(),
                    None => true,
                }
            };
            assert!(resolved, "{path}: unresolved {r}");
        }
        for (_, child) in node.properties.iter().chain(node.definitions) {
            visit(root, child, path);
        }
        if let Some(items) = node.items {
            visit(root, items, path);
        }
        for alt in node.one_of.iter().chain(node.any_of).chain(node.all_of) {
            visit(root, alt, path);
        }
    }
    for c in component_types() {
        visit(c.props, c.props, c.id);
    }
    for (name, schema) in shared_schemas() {
        visit(schema, schema, name);
    }
}

#[test]
fn builtin_lookup() {
    assert!(is_jython_builtin("len"));
    assert!(is_jython_builtin("unicode"));
    assert!(is_jython_builtin("xrange"));
    assert!(!is_jython_builtin("system"));
    let b = jython_builtins()
        .iter()
        .find(|b| b.name == "ValueError")
        .unwrap();
    assert_eq!(b.kind, BuiltinKind::Exception);
}

#[test]
fn slugs() {
    assert_eq!(slug("Date and Time"), "date-and-time");
    assert_eq!(slug("Type Casting"), "type-casting");
}
