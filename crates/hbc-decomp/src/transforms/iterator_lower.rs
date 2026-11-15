// Lower leftover Hermes iterator-protocol steps to correct plain JS.
//
// The IR builder turns `IteratorNext dst, iter, src` into `dst = iter.next()`,
// but the opcode really yields the *value* (not a {value, done} record) and
// sets `iter = undefined` once the iterator is exhausted. Pattern matchers
// (for-of, array destructuring) rely on the simple `iter.next()` form, so it is
// only rewritten here, after they ran, for whatever they could not match
// (destructuring with defaults / rest, nested destructuring in loops, ...):
//
//   dst = iter.next()   =>  dst = iter.next();
//                           if (dst.done) { iter = undefined; dst = undefined }
//                           else { dst = dst.value }
//   iter.return()       =>  if (iter.return) { iter.return() }   (array iterators have no `return`)
//
// An "iterator register" is one assigned from `src[Symbol.iterator]()`, or a
// register copied from one (fixpoint over the function).

use crate::ir::{
    AssignTarget, Binding, Constant, Expression, PropertyKey, Statement, UnaryOp, Value,
};
use std::collections::HashSet;

pub fn lower_iterator_protocol(stmts: Vec<Statement>) -> Vec<Statement> {
    let mut iters: HashSet<u32> = HashSet::new();
    collect_begin(&stmts, &mut iters);
    if iters.is_empty() {
        return stmts;
    }
    loop {
        let n = iters.len();
        collect_copies(&stmts, &mut iters);
        if iters.len() == n {
            break;
        }
    }
    lower(stmts, &iters)
}

fn each_child<'a>(s: &'a Statement, f: &mut dyn FnMut(&'a [Statement])) {
    match s {
        Statement::While { body, .. }
        | Statement::DoWhile { body, .. }
        | Statement::For { body, .. }
        | Statement::ForOf { body, .. }
        | Statement::ForIn { body, .. } => f(body),
        Statement::If {
            then_body,
            else_body,
            ..
        } => {
            f(then_body);
            f(else_body);
        }
        Statement::Block(b) => f(b),
        Statement::TryCatch {
            try_body,
            catch_body,
            finally_body,
            ..
        } => {
            f(try_body);
            f(catch_body);
            f(finally_body);
        }
        _ => {}
    }
}

fn reg_of(e: &Expression) -> Option<u32> {
    match e {
        Expression::Value(Value::Binding(Binding::Register(r))) => Some(*r),
        _ => None,
    }
}

fn collect_begin(stmts: &[Statement], out: &mut HashSet<u32>) {
    for s in stmts {
        if let Statement::Assign {
            target: AssignTarget::Binding(Binding::Register(r)),
            value,
        } = s
        {
            if is_symbol_iterator_call(value) {
                out.insert(*r);
            }
        }
        each_child(s, &mut |b| collect_begin(b, out));
    }
}

fn collect_copies(stmts: &[Statement], out: &mut HashSet<u32>) {
    for s in stmts {
        if let Statement::Assign {
            target: AssignTarget::Binding(Binding::Register(d)),
            value,
        } = s
        {
            if reg_of(value).is_some_and(|r| out.contains(&r)) {
                out.insert(*d);
            }
        }
        each_child(s, &mut |b| collect_copies(b, out));
    }
}

// `x[Symbol.iterator]()`
fn is_symbol_iterator_call(e: &Expression) -> bool {
    if let Expression::Call { callee, arguments } = e {
        if let (true, Expression::Member { property: PropertyKey::Computed(c), .. }) =
            (arguments.is_empty(), callee.as_ref())
        {
            if let Expression::Member {
                object,
                property: PropertyKey::Ident(p),
                ..
            } = c.as_ref()
            {
                return p == "iterator"
                    && matches!(object.as_ref(), Expression::Value(Value::Binding(Binding::Variable(n))) if n == "Symbol");
            }
        }
    }
    false
}

// `r.<name>()` -> r
fn method_call_on_reg(e: &Expression, name: &str) -> Option<u32> {
    if let Expression::Call { callee, arguments } = e {
        if arguments.is_empty() {
            if let Expression::Member {
                object,
                property: PropertyKey::Ident(p),
                ..
            } = callee.as_ref()
            {
                if p == name {
                    return reg_of(object);
                }
            }
        }
    }
    None
}

// `void 0`, not `Constant::Undefined`: the cleanup pass that drops Hermes'
// `reg = undefined` register clears is flow-insensitive and would delete these
// stores, which the loop's done-check depends on.
fn undef() -> Expression {
    Expression::Unary {
        op: UnaryOp::Void,
        operand: Box::new(Expression::Value(Value::Constant(Constant::Integer(0)))),
    }
}

fn reg(r: u32) -> Expression {
    Expression::Value(Value::Binding(Binding::Register(r)))
}

fn set(r: u32, value: Expression) -> Statement {
    Statement::Assign {
        target: AssignTarget::Binding(Binding::Register(r)),
        value,
    }
}

fn lower(stmts: Vec<Statement>, iters: &HashSet<u32>) -> Vec<Statement> {
    let mut out = Vec::with_capacity(stmts.len());
    for s in stmts {
        match s {
            Statement::Assign {
                target: AssignTarget::Binding(Binding::Register(dst)),
                value,
            } if method_call_on_reg(&value, "next").is_some_and(|r| iters.contains(&r)) => {
                let it = method_call_on_reg(&value, "next").unwrap();
                out.push(set(dst, value));
                out.push(Statement::If {
                    condition: Expression::member(reg(dst), "done"),
                    then_body: vec![
                        set(it, undef()),
                        set(dst, undef()),
                    ],
                    else_body: vec![set(dst, Expression::member(reg(dst), "value"))],
                });
            }
            Statement::Expr(e) if method_call_on_reg(&e, "return").is_some_and(|r| iters.contains(&r)) => {
                let it = method_call_on_reg(&e, "return").unwrap();
                out.push(Statement::If {
                    condition: Expression::member(reg(it), "return"),
                    then_body: vec![Statement::Expr(e)],
                    else_body: vec![],
                });
            }
            Statement::While { condition, body } => out.push(Statement::While {
                condition,
                body: lower(body, iters),
            }),
            Statement::DoWhile { body, condition } => out.push(Statement::DoWhile {
                body: lower(body, iters),
                condition,
            }),
            Statement::If {
                condition,
                then_body,
                else_body,
            } => out.push(Statement::If {
                condition,
                then_body: lower(then_body, iters),
                else_body: lower(else_body, iters),
            }),
            Statement::For {
                init,
                condition,
                update,
                body,
            } => out.push(Statement::For {
                init,
                condition,
                update,
                body: lower(body, iters),
            }),
            Statement::ForOf {
                variable,
                iterable,
                body,
            } => out.push(Statement::ForOf {
                variable,
                iterable,
                body: lower(body, iters),
            }),
            Statement::ForIn {
                variable,
                object,
                body,
            } => out.push(Statement::ForIn {
                variable,
                object,
                body: lower(body, iters),
            }),
            Statement::Block(b) => out.push(Statement::Block(lower(b, iters))),
            Statement::TryCatch {
                try_body,
                catch_param,
                catch_body,
                finally_body,
            } => out.push(Statement::TryCatch {
                try_body: lower(try_body, iters),
                catch_param,
                catch_body: lower(catch_body, iters),
                finally_body: lower(finally_body, iters),
            }),
            other => out.push(other),
        }
    }
    out
}


#[cfg(test)]
mod tests {
    use super::*;

    fn r(n: u32) -> Expression {
        reg(n)
    }

    #[test]
    fn next_and_return_on_iterator_registers_are_lowered() {
        let begin = Expression::Call {
            callee: Box::new(Expression::Member {
                object: Box::new(r(9)),
                property: PropertyKey::Computed(Box::new(Expression::member(
                    Expression::Value(Value::Binding(Binding::Variable("Symbol".into()))),
                    "iterator",
                ))),
                optional: false,
            }),
            arguments: vec![],
        };
        let next = |n| Expression::Call {
            callee: Box::new(Expression::member(r(n), "next")),
            arguments: vec![],
        };
        let other_next = set(7, next(8)); // r8 is not an iterator register
        let stmts = vec![
            set(1, begin),
            set(2, r(1)), // alias
            set(3, next(2)),
            Statement::Expr(Expression::Call {
                callee: Box::new(Expression::member(r(2), "return")),
                arguments: vec![],
            }),
            other_next.clone(),
        ];
        let out = lower_iterator_protocol(stmts);
        // begin, alias, next, done-if, guarded return, untouched foreign next
        assert_eq!(out.len(), 6);
        match &out[3] {
            Statement::If {
                condition,
                then_body,
                else_body,
            } => {
                assert_eq!(condition, &Expression::member(r(3), "done"));
                assert_eq!(then_body.len(), 2); // iter = void 0; dst = void 0
                assert_eq!(else_body, &vec![set(3, Expression::member(r(3), "value"))]);
            }
            other => panic!("expected done-if, got {other:?}"),
        }
        assert!(matches!(&out[4], Statement::If { .. }));
        assert_eq!(out[5], other_next);
    }
}
