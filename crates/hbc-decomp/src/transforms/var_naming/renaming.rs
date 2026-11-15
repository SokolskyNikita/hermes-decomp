use super::state::VariableNamer;
use crate::ir::{AssignTarget, Binding, Expression, PropertyKey, Statement, Value};

pub fn rename_stmt(namer: &VariableNamer, stmt: Statement) -> Statement {
    match stmt {
        Statement::Assign { target, value } => Statement::Assign {
            target: rename_target(namer, target),
            value: rename_expr(namer, value),
        },
        Statement::Let { name, value, kind } => Statement::Let {
            name: maybe_rename_var(namer, &name),
            value: rename_expr(namer, value),
            kind,
        },
        Statement::Delete { target, result } => Statement::Delete {
            target: rename_expr(namer, target),
            result,
        },
        Statement::Expr(e) => Statement::Expr(rename_expr(namer, e)),
        Statement::Return(Some(e)) => Statement::Return(Some(rename_expr(namer, e))),
        Statement::Throw(e) => Statement::Throw(rename_expr(namer, e)),
        Statement::If {
            condition,
            then_body,
            else_body,
        } => Statement::If {
            condition: rename_expr(namer, condition),
            then_body: then_body
                .into_iter()
                .map(|s| rename_stmt(namer, s))
                .collect(),
            else_body: else_body
                .into_iter()
                .map(|s| rename_stmt(namer, s))
                .collect(),
        },
        Statement::While { condition, body } => Statement::While {
            condition: rename_expr(namer, condition),
            body: body.into_iter().map(|s| rename_stmt(namer, s)).collect(),
        },
        Statement::DoWhile { body, condition } => Statement::DoWhile {
            body: body.into_iter().map(|s| rename_stmt(namer, s)).collect(),
            condition: rename_expr(namer, condition),
        },
        Statement::For {
            init,
            condition,
            update,
            body,
        } => Statement::For {
            init: init.map(|s| Box::new(rename_stmt(namer, *s))),
            condition: condition.map(|e| rename_expr(namer, e)),
            update: update.map(|s| Box::new(rename_stmt(namer, *s))),
            body: body.into_iter().map(|s| rename_stmt(namer, s)).collect(),
        },
        Statement::ForOf {
            variable,
            iterable,
            body,
        } => Statement::ForOf {
            variable: maybe_rename_var(namer, &variable),
            iterable: rename_expr(namer, iterable),
            body: body.into_iter().map(|s| rename_stmt(namer, s)).collect(),
        },
        Statement::ForIn {
            variable,
            object,
            body,
        } => Statement::ForIn {
            variable: maybe_rename_var(namer, &variable),
            object: rename_expr(namer, object),
            body: body.into_iter().map(|s| rename_stmt(namer, s)).collect(),
        },
        Statement::TryCatch {
            try_body,
            catch_param,
            catch_body,
            finally_body,
        } => Statement::TryCatch {
            try_body: try_body
                .into_iter()
                .map(|s| rename_stmt(namer, s))
                .collect(),
            catch_param: catch_param.map(|p| maybe_rename_var(namer, &p)),
            catch_body: catch_body
                .into_iter()
                .map(|s| rename_stmt(namer, s))
                .collect(),
            finally_body: finally_body
                .into_iter()
                .map(|s| rename_stmt(namer, s))
                .collect(),
        },
        Statement::Switch {
            discriminant,
            cases,
            default,
        } => Statement::Switch {
            discriminant: rename_expr(namer, discriminant),
            cases: cases
                .into_iter()
                .map(|(e, stmts)| {
                    (
                        rename_expr(namer, e),
                        stmts.into_iter().map(|s| rename_stmt(namer, s)).collect(),
                    )
                })
                .collect(),
            default: default
                .map(|stmts| stmts.into_iter().map(|s| rename_stmt(namer, s)).collect()),
        },
        Statement::Block(stmts) => {
            Statement::Block(stmts.into_iter().map(|s| rename_stmt(namer, s)).collect())
        }
        // The `extends` expression reads the enclosing scope like any other
        // expression; left untouched, the base class kept its old name while
        // its definition was renamed, and `class X extends tmp2` extended a
        // name nothing defined any more. The class body is not renamed here.
        Statement::Class {
            name,
            super_class,
            constructor,
            methods,
        } => Statement::Class {
            name,
            super_class: super_class.map(|e| rename_expr(namer, e)),
            constructor,
            methods,
        },
        other => other,
    }
}

fn rename_target(namer: &VariableNamer, target: AssignTarget) -> AssignTarget {
    match target {
        AssignTarget::Binding(Binding::Register(r)) => {
            let key = format!("r{r}");
            if let Some(name) = namer.inferred_names.get(&key) {
                AssignTarget::Binding(Binding::Variable(name.clone()))
            } else {
                AssignTarget::Binding(Binding::Register(r))
            }
        }
        AssignTarget::Binding(Binding::Variable(v)) => {
            if let Some(name) = namer.inferred_names.get(&v) {
                AssignTarget::Binding(Binding::Variable(name.clone()))
            } else {
                AssignTarget::Binding(Binding::Variable(v))
            }
        }
        AssignTarget::Member { object, property } => AssignTarget::Member {
            object: rename_expr(namer, object),
            property,
        },
        AssignTarget::Index { object, key } => AssignTarget::Index {
            object: rename_expr(namer, object),
            key: rename_expr(namer, key),
        },
        // The bindings a destructuring pattern writes, and its defaults.
        AssignTarget::DestructuringObject(props) => {
            AssignTarget::DestructuringObject(rename_object_pattern(namer, props))
        }
        AssignTarget::DestructuringObjectRest { properties, rest } => {
            AssignTarget::DestructuringObjectRest {
                properties: rename_object_pattern(namer, properties),
                rest: Box::new(rename_target(namer, *rest)),
            }
        }
        AssignTarget::DestructuringArray(elements) => {
            AssignTarget::DestructuringArray(rename_array_pattern(namer, elements))
        }
        AssignTarget::DestructuringArrayRest { elements, rest } => {
            AssignTarget::DestructuringArrayRest {
                elements: rename_array_pattern(namer, elements),
                rest: Box::new(rename_target(namer, *rest)),
            }
        }
        AssignTarget::Rest(inner) => AssignTarget::Rest(Box::new(rename_target(namer, *inner))),
        t @ AssignTarget::Binding(Binding::ClosureVar { .. }) => t,
    }
}

type ObjectPattern = Vec<(String, AssignTarget, Option<Expression>)>;
type ArrayPattern = Vec<Option<(AssignTarget, Option<Expression>)>>;

fn rename_object_pattern(namer: &VariableNamer, props: ObjectPattern) -> ObjectPattern {
    props
        .into_iter()
        .map(|(k, t, def)| {
            (
                k,
                rename_target(namer, t),
                def.map(|e| rename_expr(namer, e)),
            )
        })
        .collect()
}

fn rename_array_pattern(namer: &VariableNamer, elements: ArrayPattern) -> ArrayPattern {
    elements
        .into_iter()
        .map(|e| e.map(|(t, def)| (rename_target(namer, t), def.map(|d| rename_expr(namer, d)))))
        .collect()
}

fn rename_expr(namer: &VariableNamer, expr: Expression) -> Expression {
    match expr {
        Expression::Value(Value::Binding(Binding::Register(r))) => {
            let key = format!("r{r}");
            if let Some(name) = namer.inferred_names.get(&key) {
                Expression::Value(Value::Binding(Binding::Variable(name.clone())))
            } else {
                Expression::Value(Value::Binding(Binding::Register(r)))
            }
        }
        Expression::Value(Value::Binding(Binding::Variable(v))) => {
            if let Some(name) = namer.inferred_names.get(&v) {
                Expression::Value(Value::Binding(Binding::Variable(name.clone())))
            } else {
                Expression::Value(Value::Binding(Binding::Variable(v)))
            }
        }
        Expression::Binary { op, left, right } => Expression::Binary {
            op,
            left: Box::new(rename_expr(namer, *left)),
            right: Box::new(rename_expr(namer, *right)),
        },
        Expression::Unary { op, operand } => Expression::Unary {
            op,
            operand: Box::new(rename_expr(namer, *operand)),
        },
        Expression::Call { callee, arguments } => Expression::Call {
            callee: Box::new(rename_expr(namer, *callee)),
            arguments: arguments
                .into_iter()
                .map(|a| rename_expr(namer, a))
                .collect(),
        },
        Expression::New { callee, arguments } => Expression::New {
            callee: Box::new(rename_expr(namer, *callee)),
            arguments: arguments
                .into_iter()
                .map(|a| rename_expr(namer, a))
                .collect(),
        },
        Expression::Member {
            object,
            property,
            optional,
        } => Expression::Member {
            object: Box::new(rename_expr(namer, *object)),
            property: match property {
                PropertyKey::Computed(e) => PropertyKey::Computed(Box::new(rename_expr(namer, *e))),
                other => other,
            },
            optional,
        },
        Expression::Array { elements } => Expression::Array {
            elements: elements
                .into_iter()
                .map(|e| e.map(|ex| rename_expr(namer, ex)))
                .collect(),
        },
        Expression::Object { properties } => Expression::Object {
            properties: properties
                .into_iter()
                .map(|p| crate::ir::ObjectProperty {
                    key: match p.key {
                        PropertyKey::Computed(e) => {
                            PropertyKey::Computed(Box::new(rename_expr(namer, *e)))
                        }
                        other => other,
                    },
                    value: rename_expr(namer, p.value),
                })
                .collect(),
        },
        Expression::Conditional {
            condition,
            then_expr,
            else_expr,
        } => Expression::Conditional {
            condition: Box::new(rename_expr(namer, *condition)),
            then_expr: Box::new(rename_expr(namer, *then_expr)),
            else_expr: Box::new(rename_expr(namer, *else_expr)),
        },
        Expression::Assignment { target, value } => Expression::Assignment {
            target: Box::new(rename_target(namer, *target)),
            value: Box::new(rename_expr(namer, *value)),
        },
        Expression::Spread(inner) => Expression::Spread(Box::new(rename_expr(namer, *inner))),
        Expression::Await(inner) => Expression::Await(Box::new(rename_expr(namer, *inner))),
        Expression::Yield { value, delegate } => Expression::Yield {
            value: Box::new(rename_expr(namer, *value)),
            delegate,
        },
        // A template literal reads its interpolated values like any other
        // expression. Skipping it renamed a binding at every site except the
        // `${...}` holes, so a value merged across if/else arms (one SSA
        // class, one name) read an undefined name after the join:
        // `let text = `${str}B`` in the arms, then `${tmp}C` below.
        Expression::TemplateLiteral {
            quasis,
            expressions,
        } => Expression::TemplateLiteral {
            quasis,
            expressions: expressions
                .into_iter()
                .map(|e| rename_expr(namer, e))
                .collect(),
        },
        Expression::JSXElement {
            tag,
            attributes,
            children,
        } => Expression::JSXElement {
            tag,
            attributes: attributes
                .into_iter()
                .map(|(k, v)| (k, rename_expr(namer, v)))
                .collect(),
            children: children
                .into_iter()
                .map(|c| rename_expr(namer, c))
                .collect(),
        },
        // Listed rather than caught by a wildcard, so a new expression kind is
        // a compile error here instead of a silently half-renamed binding.
        // Function bodies are renamed per function, not from the outer scope.
        e @ (Expression::Value(_)
        | Expression::Function { .. }
        | Expression::RegExp { .. }
        | Expression::Unknown { .. }) => e,
    }
}

fn maybe_rename_var(namer: &VariableNamer, name: &str) -> String {
    if let Some(new_name) = namer.inferred_names.get(name) {
        return new_name.clone();
    }
    name.to_string()
}
