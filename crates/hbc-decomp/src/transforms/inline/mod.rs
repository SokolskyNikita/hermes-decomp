mod arguments;
mod captured;
mod cleanup;
mod counting;
mod declarations;
mod esm_cleanup;
mod folding;
mod inline_named;
mod reserved_words;
mod strip_this;

pub use arguments::simplify_arguments_copy;
pub use captured::names_used_by_descendants;
pub use cleanup::cleanup_noise;
pub use declarations::{
    extra_writes_from_nested_bodies, insert_declarations, insert_declarations_with_extra_writes,
    insert_declarations_with_outer, insert_declarations_with_slots,
};
pub use folding::{fold_array_literals, fold_object_literals};
pub use inline_named::{
    eliminate_immutable_aliases, inline_named_variables, inline_named_variables_keeping,
};
pub use reserved_words::{make_sanitized_names_distinct, rename_reserved_words};
pub use strip_this::strip_hermes_this;

use crate::ir::{AssignTarget, Binding, Expression, MutVisitor, Statement, Value, Visitor};
use std::collections::{BTreeMap, HashSet};

pub fn inline_expressions(mut stmts: Vec<Statement>) -> Vec<Statement> {
    let mut counter = UseCounter::new();
    counter.count_uses(&stmts);

    let mut inliner = ExpressionInliner::new(counter.use_count, counter.def_count);
    inliner.visit_statement_list(&mut stmts);

    stmts
}

struct UseCounter {
    use_count: BTreeMap<u32, usize>,
    def_count: BTreeMap<u32, usize>,
}

impl UseCounter {
    fn new() -> Self {
        Self {
            use_count: BTreeMap::new(),
            def_count: BTreeMap::new(),
        }
    }

    fn count_uses(&mut self, stmts: &[Statement]) {
        for stmt in stmts {
            self.visit_statement(stmt);
        }
    }
}

impl<'a> Visitor<'a> for UseCounter {
    fn visit_expression(&mut self, expr: &'a Expression) {
        if let Expression::Value(Value::Binding(Binding::Register(r))) = expr {
            *self.use_count.entry(*r).or_insert(0) += 1;
        }
        self.walk_expression(expr);
    }

    fn visit_assign_target(&mut self, target: &'a AssignTarget) {
        if let AssignTarget::Binding(Binding::Register(r)) = target {
            *self.def_count.entry(*r).or_insert(0) += 1;
        }
        self.walk_assign_target(target);
    }
}

struct ExpressionInliner {
    // Map from register to its defining expression
    definitions: BTreeMap<u32, Expression>,
    // Count of uses for each register
    use_count: BTreeMap<u32, usize>,
    // Count of definitions for each register
    def_count: BTreeMap<u32, usize>,
}

impl ExpressionInliner {
    fn new(use_count: BTreeMap<u32, usize>, def_count: BTreeMap<u32, usize>) -> Self {
        Self {
            definitions: BTreeMap::new(),
            use_count,
            def_count,
        }
    }
}

impl MutVisitor for ExpressionInliner {
    fn visit_statement_list(&mut self, stmts: &mut Vec<Statement>) {
        let mut result = Vec::new();
        // Pending assignments with side effects: (register, statement, expression value)
        let mut pending: Vec<(u32, Statement, Expression)> = Vec::new();

        // Definitions activated by an enclosing statement stay active for this
        // whole list: the enclosing statement's single use may sit anywhere in
        // it, not only in its first statement.
        let outer = self.definitions.clone();

        // We drain the original statements and build them back
        let old_stmts = std::mem::take(stmts);

        for mut stmt in old_stmts {
            let has_side_effects = crate::ir::stmt_has_side_effects(&stmt);

            let mut kept_pending = Vec::new();
            for (r, s, expr) in pending {
                if let Some(site) = use_site(&stmt, r) {
                    if site == UseSite::Head
                        || (site == UseSite::Branch && !expr.has_side_effects())
                    {
                        // Activate inline for this statement!
                        self.definitions.insert(r, expr);
                    } else {
                        // The use is inside a loop body (evaluated again on every
                        // iteration) or the value has side effects and its use is
                        // inside a branch (it would run later, and only on that
                        // path). Keep the definition where it is.
                        result.push(s);
                    }
                } else if has_side_effects || stmt_redefines_source(&stmt, &expr) {
                    // Flush to output: either this statement has side effects
                    // (ordering must be preserved), or it redefines a register
                    // that the pending value reads, inlining the value at a
                    // later use would then capture the NEW value, not the value
                    // at the copy site (`tmp = sum; sum = undefined; print(tmp)`).
                    result.push(s);
                } else {
                    // Keep waiting
                    kept_pending.push((r, s, expr));
                }
            }
            pending = kept_pending;

            // Recurse to apply the activated definitions
            self.visit_statement(&mut stmt);

            // Drop the definitions this statement activated, so they cannot
            // leak into the next one. Restoring the enclosing ones rather than
            // clearing everything matters in a nested list: clearing there wiped
            // a definition the enclosing statement had already consumed (its
            // assignment dropped), so a use after the body's first statement,
            // or in an `else` arm, read a register nothing assigned.
            self.definitions.clone_from(&outer);

            // Process the resulting statement for potential new pending
            if let Statement::Assign {
                target: AssignTarget::Binding(Binding::Register(r)),
                value,
            } = &stmt
            {
                let uses = self.use_count.get(r).copied().unwrap_or(0);
                let defs = self.def_count.get(r).copied().unwrap_or(0);
                // Only inline registers defined exactly once. A register with
                // multiple defs is loop-carried or reassigned; inlining one of its
                // definitions into a use elsewhere (e.g. across a loop back-edge)
                // is unsound, this is what silently broke counting loops.
                if uses == 1 && defs == 1 {
                    // Candidate for pending (chaining)
                    pending.push((*r, stmt.clone(), value.clone()));
                    continue; // Do not emit yet
                }
            }
            result.push(stmt);
        }

        for (_, s, _) in pending {
            result.push(s);
        }

        *stmts = result;
    }

    fn visit_statement(&mut self, stmt: &mut Statement) {
        // For a do-while, `walk_statement` visits the body before the condition,
        // and processing the body clears `self.definitions`, so a definition
        // activated for this statement (e.g. an inlined loop bound) would be lost
        // before the condition is reached. Inline the condition FIRST.
        if let Statement::DoWhile { body, condition } = stmt {
            self.visit_expression(condition);
            self.visit_statement_list(body);
            return;
        }
        self.walk_statement(stmt);
    }

    fn visit_expression(&mut self, expr: &mut Expression) {
        // First recurse into children
        self.walk_expression(expr);

        // Then modify this expression if it's a register use
        if let Expression::Value(Value::Binding(Binding::Register(r))) = expr {
            if let Some(def) = self.definitions.get(r) {
                *expr = def.clone();
            }
        }
    }
}

// Where a statement reads a register, relative to its own nested bodies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UseSite {
    // In the statement's own expressions (an `if` condition, an assigned value).
    Head,
    // Inside a nested body that runs at most once: an `if` arm, a `switch`
    // case, a block, a `try`.
    Branch,
    // Inside a loop body, which may run any number of times.
    Loop,
}

fn use_site(stmt: &Statement, reg: u32) -> Option<UseSite> {
    if !stmt_uses(stmt, reg) {
        return None;
    }
    let (bodies, is_loop) = nested_bodies(stmt);
    for body in bodies {
        for s in body {
            if let Some(inner) = use_site(s, reg) {
                return Some(if is_loop || inner == UseSite::Loop {
                    UseSite::Loop
                } else {
                    UseSite::Branch
                });
            }
        }
    }
    Some(UseSite::Head)
}

// The statement lists nested directly in `stmt`, and whether they are loop bodies.
fn nested_bodies(stmt: &Statement) -> (Vec<&[Statement]>, bool) {
    match stmt {
        Statement::If {
            then_body,
            else_body,
            ..
        } => (vec![then_body, else_body], false),
        Statement::Switch { cases, default, .. } => (
            cases
                .iter()
                .map(|(_, body)| body.as_slice())
                .chain(default.as_deref())
                .collect(),
            false,
        ),
        Statement::Block(body) => (vec![body], false),
        Statement::TryCatch {
            try_body,
            catch_body,
            finally_body,
            ..
        } => (vec![try_body, catch_body, finally_body], false),
        Statement::While { body, .. }
        | Statement::DoWhile { body, .. }
        | Statement::For { body, .. }
        | Statement::ForIn { body, .. }
        | Statement::ForOf { body, .. } => (vec![body], true),
        _ => (Vec::new(), false),
    }
}

// Helpers
fn stmt_uses(stmt: &Statement, reg: u32) -> bool {
    struct UsesRegister(u32, bool);
    impl<'a> Visitor<'a> for UsesRegister {
        fn visit_assign_target(&mut self, target: &'a AssignTarget) {
            if let AssignTarget::Binding(Binding::Register(r)) = target {
                if *r == self.0 {
                    self.1 = true;
                }
            }
            self.walk_assign_target(target);
        }
        fn visit_expression(&mut self, expr: &'a Expression) {
            if let Expression::Value(Value::Binding(Binding::Register(r))) = expr {
                if *r == self.0 {
                    self.1 = true;
                }
            }
            self.walk_expression(expr);
        }
    }
    let mut checker = UsesRegister(reg, false);
    checker.visit_statement(stmt);
    checker.1
}

// True if `stmt` assigns to any register that `value` reads. Such a write
// invalidates a pending copy of `value`, since inlining it later would capture
// the post-write register state.
fn stmt_redefines_source(stmt: &Statement, value: &Expression) -> bool {
    let mut reads: HashSet<u32> = HashSet::new();
    collect_read_regs(value, &mut reads);
    if reads.is_empty() {
        return false;
    }

    struct DefChecker<'a> {
        reads: &'a HashSet<u32>,
        found: bool,
    }
    impl<'a> Visitor<'a> for DefChecker<'a> {
        fn visit_assign_target(&mut self, target: &'a AssignTarget) {
            if let AssignTarget::Binding(Binding::Register(r)) = target {
                if self.reads.contains(r) {
                    self.found = true;
                }
            }
            self.walk_assign_target(target);
        }
        fn visit_expression(&mut self, expr: &'a Expression) {
            self.walk_expression(expr);
        }
    }
    let mut checker = DefChecker {
        reads: &reads,
        found: false,
    };
    checker.visit_statement(stmt);
    checker.found
}

fn collect_read_regs(expr: &Expression, out: &mut HashSet<u32>) {
    struct Collector<'a>(&'a mut HashSet<u32>);
    impl<'a> Visitor<'a> for Collector<'_> {
        fn visit_expression(&mut self, expr: &'a Expression) {
            if let Expression::Value(Value::Binding(Binding::Register(r))) = expr {
                self.0.insert(*r);
            }
            self.walk_expression(expr);
        }
    }
    let mut c = Collector(out);
    c.visit_expression(expr);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Constant, PropertyKey};

    #[test]
    fn test_inline_simple() {
        let stmts = vec![
            Statement::assign_reg(
                0,
                Expression::constant(Constant::String("test".to_string())),
            ),
            Statement::assign_reg(
                1,
                Expression::Member {
                    object: Box::new(Expression::Value(Value::Binding(Binding::Register(0)))),
                    property: PropertyKey::Ident("length".to_string()),
                    optional: false,
                },
            ),
        ];

        let result = inline_expressions(stmts);

        // r0 should be inlined into r1's expression
        assert!(result.len() <= 2);
    }

    fn reg(r: u32) -> Expression {
        Expression::Value(Value::Binding(Binding::Register(r)))
    }

    fn var(n: &str) -> Expression {
        Expression::Value(Value::Binding(Binding::Variable(n.into())))
    }

    fn call(callee: &str, arguments: Vec<Expression>) -> Statement {
        Statement::Expr(Expression::Call {
            callee: Box::new(var(callee)),
            arguments,
        })
    }

    fn member(object: Expression, property: &str) -> Expression {
        Expression::Member {
            object: Box::new(object),
            property: PropertyKey::Ident(property.into()),
            optional: false,
        }
    }

    fn render(stmts: &[Statement]) -> String {
        stmts.iter().map(|s| format!("{s:#}\n")).collect()
    }

    // Every register the statements read is assigned somewhere in them.
    fn assert_no_dangling_read(stmts: &[Statement]) {
        let mut counter = UseCounter::new();
        counter.count_uses(stmts);
        for r in counter.use_count.keys() {
            assert!(
                counter.def_count.contains_key(r),
                "r{r} is read but nothing assigns it:\n{}",
                render(stmts)
            );
        }
    }

    #[test]
    fn a_use_after_the_first_statement_of_a_branch_keeps_its_definition() {
        // r0 = o.load(); if (c) { print("first"); print(r0); }
        // The `if` uses r0, so r0's assignment was dropped to inline it, but the
        // inlining was undone by the then-body's first statement, and
        // `print(r0)` read a register nothing assigned (in HBC 99 bundles, a
        // value read inside a `typeof f === "function"` guard came out as an
        // undeclared temporary).
        let load = Expression::Call {
            callee: Box::new(member(var("o"), "load")),
            arguments: vec![],
        };
        let stmts = vec![
            Statement::assign_reg(0, load),
            Statement::If {
                condition: var("c"),
                then_body: vec![
                    call(
                        "print",
                        vec![Expression::constant(Constant::String("first".into()))],
                    ),
                    call("print", vec![reg(0)]),
                ],
                else_body: vec![],
            },
        ];
        let out = inline_expressions(stmts);
        assert_no_dangling_read(&out);
        // A call stays where it was: moved into the branch it would run later
        // (after `print("first")`) and only when `c` holds.
        assert_eq!(out.len(), 2, "{}", render(&out));
    }

    #[test]
    fn a_pure_value_reaches_its_use_in_an_else_arm() {
        // r0 = o.a.b; if (c) { f(); } else { f(); g(r0); }  ->  ... else { f(); g(o.a.b); }
        let stmts = vec![
            Statement::assign_reg(0, member(member(var("o"), "a"), "b")),
            Statement::If {
                condition: var("c"),
                then_body: vec![call("f", vec![])],
                else_body: vec![call("f", vec![]), call("g", vec![reg(0)])],
            },
        ];
        let out = inline_expressions(stmts);
        assert_no_dangling_read(&out);
        assert_eq!(out.len(), 1, "{}", render(&out));
        let Statement::If { else_body, .. } = &out[0] else {
            panic!("{}", render(&out));
        };
        assert!(
            render(else_body).contains("(o.a.b)"),
            "{}",
            render(else_body)
        );
    }

    #[test]
    fn a_value_used_in_a_loop_body_is_not_moved_into_it() {
        // r0 = o.a; while (c) { f(); g(r0); }: inlining would re-read `o.a` on
        // every iteration.
        let stmts = vec![
            Statement::assign_reg(0, member(var("o"), "a")),
            Statement::While {
                condition: var("c"),
                body: vec![call("f", vec![]), call("g", vec![reg(0)])],
            },
        ];
        let out = inline_expressions(stmts);
        assert_no_dangling_read(&out);
        assert_eq!(out.len(), 2, "{}", render(&out));
    }

    #[test]
    fn a_use_in_the_statement_itself_still_inlines() {
        // r0 = o.load(); if (r0) { f(); }  ->  if (o.load()) { f(); }
        let load = Expression::Call {
            callee: Box::new(member(var("o"), "load")),
            arguments: vec![],
        };
        let stmts = vec![
            Statement::assign_reg(0, load),
            Statement::If {
                condition: reg(0),
                then_body: vec![call("f", vec![])],
                else_body: vec![],
            },
        ];
        let out = inline_expressions(stmts);
        assert_eq!(out.len(), 1, "{}", render(&out));
    }
}
