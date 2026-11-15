use crate::ir::{
    is_nan_check, is_undefined_expr, AssignTarget, Binding, Expression, Statement, Value,
};

use super::esm_cleanup::{inline_hoisted_aliases_and_trim, remove_esm_boilerplate};

fn is_self_assign_value(name: &str, value: &Expression) -> bool {
    match value {
        Expression::Value(Value::Binding(Binding::Variable(val_name))) => name == val_name,
        Expression::Value(Value::Parameter(idx)) => name == format!("arg{idx}"),
        Expression::Value(Value::Constant(crate::ir::Constant::Bool(b))) => {
            (*b && name == "true") || (!*b && name == "false")
        }
        Expression::Value(Value::Constant(crate::ir::Constant::Null)) => name == "null",
        Expression::Value(Value::Constant(crate::ir::Constant::Undefined)) => name == "undefined",
        Expression::Value(Value::Global) => name == "globalThis",
        // `Error = globalThis.Error`, the Babel pattern that captures a global
        // BUILTIN into a same-named local (redundant, since bare `Error` already
        // resolves to the global). Only safe for actual builtins: for a user-local
        // like `f`, `f = globalThis.f` reads the global property into the local and
        // must NOT be dropped (it is a real, possibly-conditional assignment).
        Expression::Member {
            object,
            property: crate::ir::PropertyKey::Ident(prop),
            ..
        } => {
            prop == name
                && crate::ir::expr::display::is_builtin_global(name)
                && match &**object {
                    Expression::Value(Value::Global) => true,
                    Expression::Value(Value::Binding(Binding::Variable(v))) => v == "globalThis",
                    _ => false,
                }
        }
        _ => false,
    }
}

pub fn cleanup_noise(stmts: Vec<Statement>) -> Vec<Statement> {
    cleanup_noise_at(stmts, true)
}

// `tail`: control falling off the end of `stmts` ends the function, so a final
// `return;` there is redundant. Inside a loop body, a switch case, a `finally`
// or an `if` that is followed by more code it is not: dropping it would fall
// through into the next iteration, case or statement.
fn cleanup_noise_at(stmts: Vec<Statement>, tail: bool) -> Vec<Statement> {
    let mut result = Vec::new();

    for (i, stmt) in stmts.iter().enumerate() {
        match stmt {
            // Remove self-assignments: `x = x;` and `x = globalThis.x;` (Babel global captures)
            Statement::Assign {
                target: AssignTarget::Binding(Binding::Variable(name)),
                value,
            } => {
                if is_self_assign_value(name, value) {
                    continue;
                }
                result.push(stmt.clone());
            }
            // Remove self-assignments in let declarations: `let Error = Error;` (Babel global captures)
            Statement::Let { name, value, .. } => {
                if is_self_assign_value(name, value) {
                    continue;
                }
                result.push(stmt.clone());
            }
            // `return undefined;` -> `return;` or remove if last statement
            Statement::Return(Some(expr)) => {
                if is_undefined_expr(expr) {
                    // If this is the last statement, drop it entirely
                    if tail && i == stmts.len() - 1 {
                        continue;
                    }
                    // Otherwise convert to bare return
                    result.push(Statement::Return(None));
                } else {
                    result.push(stmt.clone());
                }
            }
            // Remove trailing bare `return;` if it's the last statement
            Statement::Return(None) if tail && i == stmts.len() - 1 => {
                continue;
            }
            // Remove dead `while (x !== x)` loops (NaN check artifact from Hermes bytecode)
            // `x !== x` is only true when x is NaN, but these loops are always dead code
            Statement::While { condition, .. } if is_nan_check(condition) => {
                continue;
            }
            // Remove empty while/for loops (artifact of structure recovery)
            Statement::While { body, .. }
            | Statement::DoWhile { body, .. }
            | Statement::For { body, .. }
            | Statement::ForIn { body, .. }
            | Statement::ForOf { body, .. }
                if body.is_empty() =>
            {
                continue;
            }
            // Remove ESM interop guard: `if (!this) { BODY } else { ... }` -> BODY
            // In ESM, `this` is always undefined, so the then-branch always executes
            Statement::If {
                condition,
                then_body,
                ..
            } if is_not_this(condition) => {
                for s in then_body {
                    result.push(s.clone());
                }
                continue;
            }
            _ => result.push(stmt.clone()),
        }
    }

    // Remove ESM boilerplate patterns
    result = remove_esm_boilerplate(result);

    // Inline hoisted parameter aliases and remove dead code after return/throw
    result = inline_hoisted_aliases_and_trim(result);

    // Recurse into sub-blocks
    let last = result.len().saturating_sub(1);
    for (i, stmt) in result.iter_mut().enumerate() {
        cleanup_noise_recurse(stmt, tail && i == last);
    }

    // Post-recursion: remove empty loops, empty if blocks, and empty blocks
    result.retain(|stmt| match stmt {
        Statement::While { body, .. }
        | Statement::DoWhile { body, .. }
        | Statement::For { body, .. }
        | Statement::ForIn { body, .. }
        | Statement::ForOf { body, .. } => !is_effectively_empty_body(body),
        Statement::If {
            then_body,
            else_body,
            ..
        } => !is_effectively_empty_body(then_body) || !is_effectively_empty_body(else_body),
        Statement::Block(inner) => !is_effectively_empty_body(inner),
        _ => true,
    });

    result
}

// Check if expression is `!this` (ESM interop guard condition)
fn is_not_this(expr: &Expression) -> bool {
    if let Expression::Unary {
        op: crate::ir::UnaryOp::Not,
        operand,
    } = expr
    {
        return matches!(&**operand, Expression::Value(Value::This));
    }
    false
}

// Check if a body is effectively empty (no meaningful statements).
fn is_effectively_empty_body(stmts: &[Statement]) -> bool {
    stmts.iter().all(|s| {
        matches!(s,
            Statement::Block(inner) if inner.is_empty() || is_effectively_empty_body(inner)
        ) || matches!(s, Statement::Continue(_))
            || matches!(s, Statement::Comment(_))
    })
}

fn cleanup_noise_recurse(stmt: &mut Statement, tail: bool) {
    let take = |b: &mut Vec<Statement>, t: bool| *b = cleanup_noise_at(std::mem::take(b), t);
    match &mut *stmt {
        // Falling off a branch of the function's last `if` (or of its last
        // block / try / catch) still ends the function.
        Statement::If {
            then_body,
            else_body,
            ..
        } => {
            take(then_body, tail);
            take(else_body, tail);
        }
        Statement::Block(inner) => take(inner, tail),
        Statement::TryCatch {
            try_body,
            catch_body,
            finally_body,
            ..
        } => {
            take(try_body, tail && finally_body.is_empty());
            take(catch_body, tail && finally_body.is_empty());
            take(finally_body, false);
        }
        other => crate::ir::map_nested_bodies_mut(other, |b| cleanup_noise_at(b, false)),
    }

    // Post-pass: invert empty then-body: `if (x) {} else { ... }` -> `if (!x) { ... }`
    if let Statement::If {
        condition,
        then_body,
        else_body,
    } = stmt
    {
        if then_body.is_empty() && !else_body.is_empty() {
            let mut temp_else = std::mem::take(else_body);
            std::mem::swap(then_body, &mut temp_else);
            let old_cond = std::mem::replace(
                condition,
                Expression::Value(Value::Constant(crate::ir::Constant::Undefined)),
            );
            *condition = crate::transforms::logic_simplify::negate_expr(old_cond);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Constant, VarKind};

    #[test]
    fn return_inside_a_loop_body_is_kept() {
        // `while (c) { if (x) { return; } }` falls into the next iteration
        // without the return; only a return at the function's tail is noise.
        let var = |n: &str| Expression::Value(Value::Binding(Binding::Variable(n.into())));
        let stmts = vec![
            Statement::While {
                condition: var("c"),
                body: vec![Statement::If {
                    condition: var("x"),
                    then_body: vec![Statement::Return(None)],
                    else_body: vec![],
                }],
            },
            Statement::Expr(var("after")),
            Statement::Return(None),
        ];
        let result = cleanup_noise(stmts);
        let Statement::While { body, .. } = &result[0] else {
            panic!("loop lost: {result:?}");
        };
        assert!(
            matches!(&body[0], Statement::If { then_body, .. } if then_body == &vec![Statement::Return(None)]),
            "return in the loop dropped: {result:?}"
        );
        // The function's own trailing `return;` still goes.
        assert_eq!(result.len(), 2, "{result:?}");
    }

    #[test]
    fn test_let_self_assign_variable_removed() {
        // `let Error = Error;` should be removed (Babel global capture)
        let stmts = vec![Statement::Let {
            name: "Error".to_string(),
            value: Expression::Value(Value::Binding(Binding::Variable("Error".to_string()))),
            kind: VarKind::Let,
        }];
        let result = cleanup_noise(stmts);
        assert!(
            result.is_empty(),
            "Self-assign `let Error = Error` should be removed"
        );
    }

    #[test]
    fn test_let_self_assign_constant_removed() {
        // `let null = null;` and `let undefined = undefined;` should be removed
        let stmts = vec![
            Statement::Let {
                name: "null".to_string(),
                value: Expression::Value(Value::Constant(Constant::Null)),
                kind: VarKind::Let,
            },
            Statement::Let {
                name: "undefined".to_string(),
                value: Expression::Value(Value::Constant(Constant::Undefined)),
                kind: VarKind::Let,
            },
        ];
        let result = cleanup_noise(stmts);
        assert!(result.is_empty(), "Self-assign constants should be removed");
    }

    #[test]
    fn test_let_non_self_assign_preserved() {
        // `let x = y;` should NOT be removed (different names)
        let stmts = vec![Statement::Let {
            name: "x".to_string(),
            value: Expression::Value(Value::Binding(Binding::Variable("y".to_string()))),
            kind: VarKind::Let,
        }];
        let result = cleanup_noise(stmts);
        assert_eq!(result.len(), 1, "Non-self-assign should be preserved");
    }

    #[test]
    fn test_let_global_member_self_assign_removed() {
        // `let Error = globalThis.Error;` should be removed
        let stmts = vec![Statement::Let {
            name: "Error".to_string(),
            value: Expression::Member {
                object: Box::new(Expression::Value(Value::Global)),
                property: crate::ir::PropertyKey::Ident("Error".to_string()),
                optional: false,
            },
            kind: VarKind::Let,
        }];
        let result = cleanup_noise(stmts);
        assert!(
            result.is_empty(),
            "Global member self-assign should be removed"
        );
    }
}
