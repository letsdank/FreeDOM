//! Conservative identifier collection for function bodies.
//!
//! This is not free-variable analysis: locally declared names and names inside
//! nested functions are included. Property names are not identifier reads.

use std::collections::HashSet;

use super::{Expression, Statement, UserFunction};

pub(super) fn referenced_names(function: &UserFunction) -> HashSet<String> {
    let mut names = HashSet::new();

    visit_function(function, &mut names);

    names
}

fn visit_function(function: &UserFunction, names: &mut HashSet<String>) {
    for parameter in &function.parameters {
        if let Some(default) = &parameter.default {
            visit_expression(default, names);
        }
    }

    visit_statements(&function.body, names);
}

fn visit_statements(statements: &[Statement], names: &mut HashSet<String>) {
    for statement in statements {
        match statement {
            Statement::Declaration { value, .. }
            | Statement::Expression(value)
            | Statement::Return(value)
            | Statement::Throw(value) => {
                visit_expression(value, names);
            }

            Statement::Declarations(declarations) => {
                for (_, value) in declarations {
                    visit_expression(value, names);
                }
            }

            Statement::FunctionDeclaration { function, .. } => {
                visit_function(function, names);
            }

            Statement::If {
                condition,
                then_body,
                else_body,
            } => {
                visit_expression(condition, names);
                visit_statements(then_body, names);
                visit_statements(else_body, names);
            }

            Statement::Switch {
                discriminant,
                cases,
            } => {
                visit_expression(discriminant, names);

                for case in cases {
                    if let Some(test) = &case.test {
                        visit_expression(test, names);
                    }

                    visit_statements(&case.body, names);
                }
            }

            Statement::While { condition, body } => {
                visit_expression(condition, names);
                visit_statements(body, names);
            }

            Statement::For {
                initializer,
                condition,
                update,
                body,
            } => {
                if let Some(initializer) = initializer {
                    visit_statements(std::slice::from_ref(initializer.as_ref()), names);
                }

                if let Some(condition) = condition {
                    visit_expression(condition, names);
                }

                if let Some(update) = update {
                    visit_expression(update, names);
                }

                visit_statements(body, names);
            }

            Statement::ForOf { iterable, body, .. }
            | Statement::ForOfArray { iterable, body, .. } => {
                visit_expression(iterable, names);
                visit_statements(body, names);
            }

            Statement::ForIn { object, body, .. } => {
                visit_expression(object, names);
                visit_statements(body, names);
            }

            Statement::ForInExisting {
                binding,
                object,
                body,
            } => {
                names.insert(binding.clone());

                visit_expression(object, names);
                visit_statements(body, names);
            }

            Statement::Try {
                body,
                catch,
                finally,
            } => {
                visit_statements(body, names);

                if let Some(catch) = catch {
                    visit_statements(&catch.body, names);
                }

                if let Some(finally) = finally {
                    visit_statements(finally, names);
                }
            }

            Statement::Break | Statement::Continue => {}
        }
    }
}

fn visit_expression(expression: &Expression, names: &mut HashSet<String>) {
    match expression {
        Expression::Identifier(name) => {
            names.insert(name.clone());
        }

        Expression::Template { parts, .. } => {
            for (expression, _) in parts {
                visit_expression(expression, names);
            }
        }

        Expression::Array(elements) => {
            for expression in elements.iter().flatten() {
                visit_expression(expression, names);
            }
        }

        Expression::Object(properties) => {
            for (_, expression) in properties {
                visit_expression(expression, names);
            }
        }

        Expression::New {
            constructor,
            arguments,
        } => {
            names.insert(constructor.clone());

            for argument in arguments {
                visit_expression(&argument, names);
            }
        }

        Expression::Member { object, .. } => {
            visit_expression(object, names);
        }

        Expression::Call { callee, arguments } => {
            visit_expression(callee, names);

            for argument in arguments {
                visit_expression(&argument, names);
            }
        }

        Expression::Assignment { target, value }
        | Expression::CompoundAssignment { target, value, .. }
        | Expression::Binary {
            left: target,
            right: value,
            ..
        }
        | Expression::Index {
            object: target,
            index: value,
        } => {
            visit_expression(target, names);
            visit_expression(value, names);
        }

        Expression::Function(function) => {
            visit_function(function, names);
        }

        Expression::Not(value)
        | Expression::Await(value)
        | Expression::UnaryPlus(value)
        | Expression::UnaryMinus(value)
        | Expression::TypeOf(value)
        | Expression::Update { target: value, .. } => {
            visit_expression(value, names);
        }

        Expression::Conditional {
            condition,
            consequent,
            alternate,
        } => {
            visit_expression(condition, names);
            visit_expression(consequent, names);
            visit_expression(alternate, names);
        }

        Expression::Literal(_) | Expression::RegExpLiteral { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use super::referenced_names;
    use crate::javascript::{Statement, parse_program};

    #[test]
    fn collects_references_in_defaults_nested_functions_and_expressions() {
        let program = parse_program(
            r#"
                function sample(a = fallback) {
                    let local = obj.member;

                    function nested() {
                        return hidden[index];
                    }

                    for (key in source) {
                        sink = value;
                    }

                    return new Maker(local);
                }
            "#,
        )
        .unwrap();

        let [Statement::FunctionDeclaration { function, .. }] = program.as_slice() else {
            panic!("expected function declaration");
        };

        let names = referenced_names(function);

        for name in [
            "fallback", "obj", "hidden", "index", "key", "source", "sink", "value", "Maker",
            "local",
        ] {
            assert!(names.contains(name), "missing {name}");
        }

        assert!(!names.contains("member"));
        assert!(!names.contains("sample"));
        assert!(!names.contains("nested"));
    }
}
