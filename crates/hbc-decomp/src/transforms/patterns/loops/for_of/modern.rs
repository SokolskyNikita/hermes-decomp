use super::{is_iterator_call, unwrap_iterator_body};
use crate::analysis::rename_registers;
use crate::ir::utils::{expr_uses_register, for_each_nested_body, map_nested_bodies, stmt_uses_register};
use crate::ir::{AssignTarget, Binding, Expression, PropertyKey, Statement, Value};
use std::collections::BTreeMap;

// Detect for-of loop patterns and rebuild them as `for (item of source)`.
//
// Hermes lowers `for (x of src)` to the iterator protocol
// (IteratorBegin / IteratorNext / IteratorClose). After IR build + structure
// recovery the shape is:
//
//   iter = src[Symbol.iterator]()        // IteratorBegin
//   val  = iter.next()                    // IteratorNext (value)
//   copy = iter                           // Mov of the iterator/index
//   while (copy !== undefined) {          // done check (iter set to undefined)
//     try { <body using val> } catch (e) { iter.return(); throw e }
//   }
//
// We match that and emit `for (val of src) { <body> }`, dropping the iterator
// plumbing (the per-iteration fetch is reintroduced by for-of semantics).
pub fn detect_for_of_loops(stmts: Vec<Statement>) -> Vec<Statement> {
    let stmts = recurse(stmts);
    let mut result: Vec<Statement> = Vec::new();
    let mut i = 0;
    while i < stmts.len() {
        if let Some((consumed, emitted)) = try_match_for_of(&stmts[i..]) {
            result.extend(emitted);
            i += consumed;
            continue;
        }
        result.push(stmts[i].clone());
        i += 1;
    }
    result
}

// Recurse into nested blocks first so inner for-of loops are rebuilt too.
fn recurse(stmts: Vec<Statement>) -> Vec<Statement> {
    stmts
        .into_iter()
        .map(|stmt| match stmt {
            Statement::While { condition, body } => Statement::While {
                condition,
                body: detect_for_of_loops(body),
            },
            Statement::DoWhile { body, condition } => Statement::DoWhile {
                body: detect_for_of_loops(body),
                condition,
            },
            Statement::If {
                condition,
                then_body,
                else_body,
            } => Statement::If {
                condition,
                then_body: detect_for_of_loops(then_body),
                else_body: detect_for_of_loops(else_body),
            },
            Statement::For {
                init,
                condition,
                update,
                body,
            } => Statement::For {
                init,
                condition,
                update,
                body: detect_for_of_loops(body),
            },
            Statement::ForOf {
                variable,
                iterable,
                body,
            } => Statement::ForOf {
                variable,
                iterable,
                body: detect_for_of_loops(body),
            },
            Statement::ForIn {
                variable,
                object,
                body,
            } => Statement::ForIn {
                variable,
                object,
                body: detect_for_of_loops(body),
            },
            Statement::Block(inner) => Statement::Block(detect_for_of_loops(inner)),
            Statement::TryCatch {
                try_body,
                catch_param,
                catch_body,
                finally_body,
            } => Statement::TryCatch {
                try_body: detect_for_of_loops(try_body),
                catch_param,
                catch_body: detect_for_of_loops(catch_body),
                finally_body: detect_for_of_loops(finally_body),
            },
            other => other,
        })
        .collect()
}

// Try to match the iterator sequence at the start of `stmts`. Returns the number
// of leading statements consumed and the statements to emit in their place
// (any non-iterator statements that were interleaved, e.g. an `undefined` load
// the trailing `return` still needs, are preserved, followed by the ForOf).
fn try_match_for_of(stmts: &[Statement]) -> Option<(usize, Vec<Statement>)> {
    // [0] iter = src[Symbol.iterator]()
    let (iter_reg, source) = match &stmts[0] {
        Statement::Assign {
            target: AssignTarget::Binding(Binding::Register(r)),
            value,
        } => (*r, is_iterator_call(value)?),
        _ => return None,
    };

    // Scan the iterator plumbing between IteratorBegin and the loop, in any order:
    //   val = iter.next()         (the per-iteration value, capture it)
    //   copy = iter               (alias of the iterator/index register)
    //   x = undefined             (the done sentinel)
    // Stop at the `while` (the iterator loop).
    let mut idx = 1;
    let mut iter_aliases = vec![iter_reg];
    let mut val_reg: Option<u32> = None;
    // Non-iterator statements interleaved with the plumbing that must survive
    // (e.g. an `undefined` constant load referenced after the loop).
    let mut kept: Vec<Statement> = Vec::new();
    while let Some(stmt) = stmts.get(idx) {
        match stmt {
            // val = <alias>.next()
            Statement::Assign {
                target: AssignTarget::Binding(Binding::Register(r)),
                value,
            } if iter_aliases.iter().any(|&a| is_next_call(value, a)) => {
                val_reg = Some(*r);
                idx += 1;
            }
            // copy = iter (alias)
            Statement::Assign {
                target: AssignTarget::Binding(Binding::Register(dst)),
                value: Expression::Value(Value::Binding(Binding::Register(src))),
            } if iter_aliases.contains(src) => {
                iter_aliases.push(*dst);
                idx += 1;
            }
            // Any other plain value load into a register: the `undefined` done
            // sentinel, a hoisted constant (`null`, a string, a number), or the
            // `Mov idx, src` that HBC >= 98 emits before every `IteratorNext`. At
            // -O copy propagation has often already turned that source into a
            // parameter or variable read (`r = arg0`), so any register-free value
            // counts. Keep it (it may be read later) and keep scanning so the
            // following `val = iter.next()` is still recognised. A load that
            // overwrites a register the iterable reads would change what the
            // for-of header evaluates, so it ends the scan.
            Statement::Assign {
                target: AssignTarget::Binding(Binding::Register(dst)),
                value: Expression::Value(v),
            } if !matches!(v, Value::Binding(Binding::Register(src)) if iter_aliases.contains(src))
                && !expr_uses_register(&source, *dst) =>
            {
                kept.push(stmt.clone());
                idx += 1;
            }
            // loop label / offset comments left by structure recovery
            Statement::Comment(_) => idx += 1,
            _ => break,
        }
    }
    let val_reg = val_reg?;

    // while (<iter-alias> !== <undefined>) { body }
    let body = match stmts.get(idx)? {
        Statement::While { condition, body } if is_iter_done_check(condition, &iter_aliases) => {
            body
        }
        _ => return None,
    };

    // Strip the iterator try/return plumbing and the trailing `// continue`.
    let mut loop_body = unwrap_iterator_body(body, iter_reg);
    // `IteratorClose` before an early `break`/`return` is implied by for-of.
    loop_body = strip_iterator_close(loop_body, &iter_aliases);
    // The back edge to the iterator step shows up as a final `continue;`.
    if matches!(loop_body.last(), Some(Statement::Continue(None))) {
        loop_body.pop();
    }
    // Anything else that still touches the iterator (or re-steps it) has no
    // for-of equivalent; leave the loop as it is.
    if iter_aliases
        .iter()
        .any(|&a| loop_body.iter().any(|s| stmt_reads_register_deep(s, a)))
    {
        return None;
    }
    // Rename the value register to a named loop variable.
    let var_name = format!("item{val_reg}");
    let mut map = BTreeMap::new();
    map.insert(val_reg, var_name.clone());
    loop_body = rename_registers(loop_body, &map);

    kept.push(Statement::ForOf {
        variable: var_name,
        iterable: source,
        body: detect_for_of_loops(loop_body),
    });
    Some((idx + 1, kept))
}

// Remove `alias.return()` (the lowered `IteratorClose` on an early exit) from a
// for-of body, at any depth except inside nested functions, together with the
// `copy = iter` moves that only fed it. for-of closes the iterator itself on
// `break`/`return`/`throw`.
fn strip_iterator_close(body: Vec<Statement>, iter_aliases: &[u32]) -> Vec<Statement> {
    // Body-local copies of the iterator (`r = iter` right before the close).
    let mut aliases = iter_aliases.to_vec();
    collect_local_aliases(&body, &mut aliases);
    let stripped = remove_close_calls(body, &aliases);
    let local: Vec<u32> = aliases[iter_aliases.len()..].to_vec();
    remove_dead_copies(stripped, &local)
}

fn collect_local_aliases(body: &[Statement], aliases: &mut Vec<u32>) {
    for stmt in body {
        if let Statement::Assign {
            target: AssignTarget::Binding(Binding::Register(dst)),
            value: Expression::Value(Value::Binding(Binding::Register(src))),
        } = stmt
        {
            if aliases.contains(src) && !aliases.contains(dst) {
                aliases.push(*dst);
            }
        }
        for_each_nested_body(stmt, &mut |b| collect_local_aliases(b, aliases));
    }
}

fn is_close_call(stmt: &Statement, aliases: &[u32]) -> bool {
    let Statement::Expr(Expression::Call { callee, arguments }) = stmt else {
        return false;
    };
    if !arguments.is_empty() {
        return false;
    }
    matches!(callee.as_ref(), Expression::Member {
        object,
        property: PropertyKey::Ident(p),
        ..
    } if p == "return"
        && matches!(object.as_ref(), Expression::Value(Value::Binding(Binding::Register(r))) if aliases.contains(r)))
}

fn remove_close_calls(body: Vec<Statement>, aliases: &[u32]) -> Vec<Statement> {
    body.into_iter()
        .filter(|s| !is_close_call(s, aliases))
        .map(|s| map_nested_bodies(s, |b| remove_close_calls(b, aliases)))
        .collect()
}

fn remove_dead_copies(body: Vec<Statement>, local: &[u32]) -> Vec<Statement> {
    if local.is_empty() {
        return body;
    }
    let dead: Vec<u32> = local
        .iter()
        .copied()
        .filter(|&r| !body.iter().any(|s| stmt_reads_register_deep(s, r)))
        .collect();
    fn go(body: Vec<Statement>, dead: &[u32]) -> Vec<Statement> {
        body.into_iter()
            .filter(|s| {
                !matches!(s, Statement::Assign {
                    target: AssignTarget::Binding(Binding::Register(dst)),
                    value: Expression::Value(Value::Binding(Binding::Register(_))),
                } if dead.contains(dst))
            })
            .map(|s| map_nested_bodies(s, |b| go(b, dead)))
            .collect()
    }
    go(body, &dead)
}

// Does `stmt` read register `reg` anywhere, including nested bodies and the
// headers of nested loops (`stmt_uses_register` only looks at the top level).
fn stmt_reads_register_deep(stmt: &Statement, reg: u32) -> bool {
    let here = match stmt {
        Statement::For {
            init,
            condition,
            update,
            ..
        } => {
            init.as_deref().is_some_and(|s| stmt_reads_register_deep(s, reg))
                || condition.as_ref().is_some_and(|c| expr_uses_register(c, reg))
                || update.as_deref().is_some_and(|s| stmt_reads_register_deep(s, reg))
        }
        Statement::ForOf { iterable, .. } => expr_uses_register(iterable, reg),
        Statement::ForIn { object, .. } => expr_uses_register(object, reg),
        Statement::Let { value, .. } => expr_uses_register(value, reg),
        Statement::DoWhile { condition, .. } => expr_uses_register(condition, reg),
        Statement::Switch { discriminant, cases, .. } => {
            expr_uses_register(discriminant, reg)
                || cases.iter().any(|(c, _)| expr_uses_register(c, reg))
        }
        other => stmt_uses_register(other, reg),
    };
    let mut nested = false;
    for_each_nested_body(stmt, &mut |b| {
        nested |= b.iter().any(|s| stmt_reads_register_deep(s, reg));
    });
    here || nested
}

// `iter_reg.next()`
fn is_next_call(expr: &Expression, iter_reg: u32) -> bool {
    if let Expression::Call { callee, arguments } = expr {
        if arguments.is_empty() {
            if let Expression::Member {
                object,
                property: PropertyKey::Ident(p),
                ..
            } = callee.as_ref()
            {
                if p == "next" {
                    if let Expression::Value(Value::Binding(Binding::Register(r))) = object.as_ref()
                    {
                        return *r == iter_reg;
                    }
                }
            }
        }
    }
    false
}

// `<iter-alias> !== <undefined>`, the iterator done-check. The right side may be
// a literal `undefined` or a register holding it; the iter side is one of the
// tracked aliases. We only require a `!==` touching an iterator alias, since the
// preceding `iter.next()` already established this is an iterator loop.
fn is_iter_done_check(expr: &Expression, iter_aliases: &[u32]) -> bool {
    use crate::ir::{BinaryOp, UnaryOp};
    let touches_iter = |e: &Expression| matches!(e, Expression::Value(Value::Binding(Binding::Register(r))) if iter_aliases.contains(r));
    match expr {
        // iter !== undefined
        Expression::Binary {
            op: BinaryOp::StrictNeq,
            left,
            right,
        } => touches_iter(left) || touches_iter(right),
        // !(iter === undefined)
        Expression::Unary {
            op: UnaryOp::Not,
            operand,
        } => {
            if let Expression::Binary {
                op: BinaryOp::StrictEq,
                left,
                right,
            } = operand.as_ref()
            {
                touches_iter(left) || touches_iter(right)
            } else {
                false
            }
        }
        _ => false,
    }
}
