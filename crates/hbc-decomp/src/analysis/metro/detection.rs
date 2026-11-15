use super::registry::{MetroModule, MetroRegistry};
use crate::ir::{Binding, Expression, Statement, Value};
use std::collections::HashMap;

pub struct MetroDetector;

impl MetroDetector {
    // Metro uses `__d(undefined, factory, moduleId, dependencyMap)` to register modules.
    // The __d function is typically stored in a variable like `r0 = __r.__d`.
    pub fn analyze_statements(
        statements: &[Statement],
        registry: &mut MetroRegistry,
        param_counts: &HashMap<u32, u32>,
    ) {
        let mut reg_functions: HashMap<String, u32> = HashMap::new();
        let mut reg_arrays: HashMap<String, Vec<u32>> = HashMap::new();
        let mut reg_integers: HashMap<String, u32> = HashMap::new();
        let mut reg_members: HashMap<String, String> = HashMap::new();

        for stmt in statements {
            Self::analyze_stmt(
                stmt,
                &mut reg_functions,
                &mut reg_arrays,
                &mut reg_integers,
                &mut reg_members,
                registry,
                param_counts,
            );
        }
    }

    fn analyze_stmt(
        stmt: &Statement,
        reg_functions: &mut HashMap<String, u32>,
        reg_arrays: &mut HashMap<String, Vec<u32>>,
        reg_integers: &mut HashMap<String, u32>,
        reg_members: &mut HashMap<String, String>,
        registry: &mut MetroRegistry,
        param_counts: &HashMap<u32, u32>,
    ) {
        match stmt {
            Statement::Assign { target, value } => {
                let var_name = match target {
                    crate::ir::AssignTarget::Binding(Binding::Register(r)) => Some(format!("r{r}")),
                    crate::ir::AssignTarget::Binding(Binding::Variable(n)) => Some(n.clone()),
                    _ => None,
                };

                if let Some(name) = var_name {
                    if let Expression::Function { id, .. } = value {
                        reg_functions.insert(name.clone(), id.0);
                    }
                    if let Some(deps) = extract_array_of_integers(value, Some(reg_integers)) {
                        reg_arrays.insert(name.clone(), deps);
                    }
                    match value {
                        Expression::Member { property, .. } => match member_name(property) {
                            Some(m) => {
                                reg_members.insert(name.clone(), m);
                            }
                            None => {
                                reg_members.remove(&name);
                            }
                        },
                        _ => {
                            reg_members.remove(&name);
                        }
                    }
                    if let Some(n) = extract_integer(value) {
                        reg_integers.insert(name.clone(), n);
                    }
                }

                Self::check_for_d_call(
                    value,
                    reg_functions,
                    reg_arrays,
                    reg_integers,
                    reg_members,
                    registry,
                    param_counts,
                );
            }
            Statement::Expr(expr) => {
                Self::check_for_d_call(
                    expr,
                    reg_functions,
                    reg_arrays,
                    reg_integers,
                    reg_members,
                    registry,
                    param_counts,
                );
            }
            Statement::If {
                then_body,
                else_body,
                ..
            } => {
                for s in then_body {
                    Self::analyze_stmt(
                        s,
                        reg_functions,
                        reg_arrays,
                        reg_integers,
                        reg_members,
                        registry,
                        param_counts,
                    );
                }
                for s in else_body {
                    Self::analyze_stmt(
                        s,
                        reg_functions,
                        reg_arrays,
                        reg_integers,
                        reg_members,
                        registry,
                        param_counts,
                    );
                }
            }
            Statement::While { body, .. } => {
                for s in body {
                    Self::analyze_stmt(
                        s,
                        reg_functions,
                        reg_arrays,
                        reg_integers,
                        reg_members,
                        registry,
                        param_counts,
                    );
                }
            }
            Statement::For { body, .. } => {
                for s in body {
                    Self::analyze_stmt(
                        s,
                        reg_functions,
                        reg_arrays,
                        reg_integers,
                        reg_members,
                        registry,
                        param_counts,
                    );
                }
            }
            Statement::Block(inner) => {
                for s in inner {
                    Self::analyze_stmt(
                        s,
                        reg_functions,
                        reg_arrays,
                        reg_integers,
                        reg_members,
                        registry,
                        param_counts,
                    );
                }
            }
            _ => {}
        }
    }

    fn check_for_d_call(
        expr: &Expression,
        reg_functions: &HashMap<String, u32>,
        reg_arrays: &HashMap<String, Vec<u32>>,
        reg_integers: &HashMap<String, u32>,
        reg_members: &HashMap<String, String>,
        registry: &mut MetroRegistry,
        param_counts: &HashMap<u32, u32>,
    ) {
        if let Expression::Call {
            callee,
            arguments,
        } = expr
        {
            // A method call such as `xs.reduce(fn, 0)` has the same
            // (this, function, integer) shape as a dependency-free `__d` call.
            // Reject callees known to be a member other than `__d`.
            let method = match &**callee {
                Expression::Member { property, .. } => member_name(property),
                Expression::Value(Value::Binding(Binding::Register(r))) => {
                    reg_members.get(&format!("r{r}")).cloned()
                }
                _ => None,
            };
            if matches!(method, Some(ref m) if m != "__d") {
                return;
            }
            // Debug print
            if arguments.len() >= 3 {
                // println!("DEBUG: checking call with {} args", arguments.len());
            }

            // Metro format: __d(undefined, factory, moduleId, deps)
            // Arguments: [0] = undefined/context, [1] = function, [2] = moduleId, [3] = deps
            // tooling-eval local patch: Metro emits __d(factory, id) with no deps
            // array for dependency-free modules (Call3: this, factory, id). Those
            // were skipped (802 of 4,738 modules in a production RN bundle, incl. React core).
            if arguments.len() == 4 || arguments.len() == 3 {
                // Get function ID - either directly or via register/variable lookup
                let function_id = match &arguments[1] {
                    Expression::Function { id, .. } => Some(id.0),
                    Expression::Value(Value::Binding(Binding::Register(r))) => {
                        reg_functions.get(&format!("r{r}")).copied()
                    }
                    Expression::Value(Value::Binding(Binding::Variable(n))) => {
                        reg_functions.get(n).copied()
                    }
                    _ => None,
                };

                // Get module ID - either directly or via register/variable lookup
                let module_id = match &arguments[2] {
                    Expression::Value(Value::Constant(crate::ir::Constant::Integer(n))) => {
                        Some(*n as u32)
                    }
                    Expression::Value(Value::Constant(crate::ir::Constant::Number(n))) => {
                        Some(*n as u32)
                    }
                    Expression::Value(Value::Binding(Binding::Register(r))) => {
                        reg_integers.get(&format!("r{r}")).copied()
                    }
                    Expression::Value(Value::Binding(Binding::Variable(n))) => {
                        reg_integers.get(n).copied()
                    }
                    _ => None,
                };

                // Get dependencies - either directly or via register/variable lookup
                let no_deps = Expression::Array { elements: Vec::new() };
                let deps_expr = arguments.get(3).unwrap_or(&no_deps);
                let dependencies = match deps_expr {
                    Expression::Array { .. } => {
                        extract_array_of_integers(deps_expr, Some(reg_integers))
                            .unwrap_or_default()
                    }
                    Expression::Value(Value::Binding(Binding::Register(r))) => reg_arrays
                        .get(&format!("r{r}"))
                        .cloned()
                        .unwrap_or_default(),
                    Expression::Value(Value::Binding(Binding::Variable(n))) => {
                        reg_arrays.get(n).cloned().unwrap_or_default()
                    }
                    _ => Vec::new(),
                };

                // Register the module if we have function and module IDs
                if let (Some(func_id), Some(mod_id)) = (function_id, module_id) {
                    let inferred_name = match &arguments[1] {
                        Expression::Function { name: Some(n), .. } if is_meaningful_name(n) => {
                            Some(n.clone())
                        }
                        _ => None,
                    };

                    let module = MetroModule {
                        module_id: mod_id,
                        function_id: func_id,
                        name: inferred_name,
                        dependencies,
                        exports: HashMap::new(),
                        // The factory's declared parameter count (this-excluded)
                        // determines the Metro convention (classic 4-param vs
                        // modern 7-param with importDefault/importAll).
                        roles: param_counts
                            .get(&func_id)
                            .map(|&n| {
                                crate::analysis::metro::registry::FactoryRoles::from_param_count(n)
                            })
                            .unwrap_or_else(
                                crate::analysis::metro::registry::FactoryRoles::standard,
                            ),
                        name_from_default_export: false,
                    };
                    registry.function_to_module.insert(func_id, mod_id);
                    registry.factories.insert(func_id, module.clone());
                    registry.modules.insert(mod_id, module);
                }
            }
        }
    }
}

fn member_name(property: &crate::ir::PropertyKey) -> Option<String> {
    match property {
        crate::ir::PropertyKey::Ident(p) | crate::ir::PropertyKey::String(p) => Some(p.clone()),
        _ => None,
    }
}

fn extract_integer(expr: &Expression) -> Option<u32> {
    match expr {
        Expression::Value(Value::Constant(crate::ir::Constant::Integer(n))) => Some(*n as u32),
        Expression::Value(Value::Constant(crate::ir::Constant::Number(n))) => Some(*n as u32),
        _ => None,
    }
}

fn extract_array_of_integers(
    expr: &Expression,
    reg_integers: Option<&HashMap<String, u32>>,
) -> Option<Vec<u32>> {
    if let Expression::Array { elements } = expr {
        let values: Vec<u32> = elements
            .iter()
            .flatten()
            .filter_map(|e| {
                if let Some(n) = extract_integer(e) {
                    return Some(n);
                }
                // Try resolving register/variable
                if let Some(map) = reg_integers {
                    match e {
                        Expression::Value(Value::Binding(Binding::Register(r))) => {
                            let key = format!("r{r}");
                            let val = map.get(&key).copied();
                            val
                        }
                        Expression::Value(Value::Binding(Binding::Variable(n))) => {
                            map.get(n).copied()
                        }
                        _ => None,
                    }
                } else {
                    None
                }
            })
            .collect();
        // Return explicit empty vector if array is empty but valid (though Metro Usually has deps)
        // But the original logic returned None if parsed values were empty.
        // If the array had elements but we failed to parse them, we get empty.
        // If array was empty [] -> we get empty.
        // Metro deps [] is valid.
        return Some(values);
    }
    None
}

pub(crate) fn is_meaningful_name(name: &str) -> bool {
    // Reject purely numeric names
    if name.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    // Reject f1234 pattern (decompiler-generated function names)
    if name.starts_with('f') && name.len() > 1 && name[1..].chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    // Reject obviously generic names (shared core)
    if super::is_obviously_generic(name) {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{AssignTarget, Constant, FunctionId, PropertyKey};

    fn stmts(prop: &str) -> Vec<Statement> {
        let reg = |r: u32| Expression::Value(Value::Binding(Binding::Register(r)));
        vec![
            Statement::Assign {
                target: AssignTarget::Binding(Binding::Register(1)),
                value: Expression::Member {
                    object: Box::new(reg(0)),
                    property: PropertyKey::Ident(prop.into()),
                    optional: false,
                },
            },
            Statement::Expr(Expression::Call {
                callee: Box::new(reg(1)),
                arguments: vec![
                    reg(0),
                    Expression::Function { id: FunctionId(5), name: None, is_arrow: false, is_async: false, is_generator: false },
                    Expression::Value(Value::Constant(Constant::Integer(0))),
                ],
            }),
        ]
    }

    #[test]
    fn method_call_with_function_and_int_is_not_a_module() {
        let mut registry = MetroRegistry::default();
        MetroDetector::analyze_statements(&stmts("reduce"), &mut registry, &HashMap::new());
        assert!(registry.modules.is_empty());
    }

    #[test]
    fn dependency_free_define_call_is_still_a_module() {
        let mut registry = MetroRegistry::default();
        MetroDetector::analyze_statements(&stmts("__d"), &mut registry, &HashMap::new());
        assert_eq!(registry.modules.len(), 1);
    }
}
