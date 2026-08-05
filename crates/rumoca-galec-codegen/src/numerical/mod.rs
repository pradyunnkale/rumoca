mod analyze;
mod diagnostic;
mod facts;
mod infer;
mod plan;

pub use facts::{Entity, NumericalFacts, Operation, Shape, Value};
pub(crate) use plan::{LinearSolveAlgorithm, NumericalPlan};

use rumoca_ir_galec::ast::{Block, Condition, Expression, Statement};

pub(crate) fn plan_embedded_block(
    block: &Block,
) -> Result<NumericalPlan, diagnostic::NumericalAnalysisError> {
    if !block_methods_contain_linear_solve(block) {
        return Ok(NumericalPlan::default());
    }
    analyze::analyze(block).map(|facts| plan::plan(&facts))
}

fn block_methods_contain_linear_solve(block: &Block) -> bool {
    [&block.startup, &block.recalibrate, &block.do_step]
        .into_iter()
        .flat_map(|method| &method.statements)
        .any(|statement| statement_contains_linear_solve(&statement.node))
}

fn statement_contains_linear_solve(statement: &Statement) -> bool {
    match statement {
        Statement::Assignment { target: _, value } => expression_contains_linear_solve(value),
        Statement::MultiAssignment { call, .. } | Statement::Call(call) => {
            call.function.lexeme() == "solveLinearEquations"
                || call.arguments.iter().any(expression_contains_linear_solve)
        }
        Statement::If(statement) => {
            statement.branches.iter().any(|branch| {
                let condition_has_solve = match &branch.condition {
                    Condition::Expression(expression) => {
                        expression_contains_linear_solve(expression)
                    }
                    Condition::SignalCheck(check) => check
                        .fallback
                        .as_ref()
                        .is_some_and(expression_contains_linear_solve),
                };
                condition_has_solve
                    || branch
                        .body
                        .iter()
                        .any(|item| statement_contains_linear_solve(&item.node))
            }) || statement.else_body.as_ref().is_some_and(|body| {
                body.iter()
                    .any(|item| statement_contains_linear_solve(&item.node))
            })
        }
        Statement::For(loop_) => {
            expression_contains_linear_solve(&loop_.start)
                || loop_
                    .step
                    .as_ref()
                    .is_some_and(expression_contains_linear_solve)
                || expression_contains_linear_solve(&loop_.stop)
                || loop_
                    .body
                    .iter()
                    .any(|item| statement_contains_linear_solve(&item.node))
        }
        Statement::Limit(_) | Statement::Signal(_) => false,
    }
}

fn expression_contains_linear_solve(expression: &Expression) -> bool {
    match expression {
        Expression::Call(call) => {
            call.function.lexeme() == "solveLinearEquations"
                || call.arguments.iter().any(expression_contains_linear_solve)
        }
        Expression::Paren(inner) | Expression::Not(inner) => {
            expression_contains_linear_solve(inner)
        }
        Expression::If(if_expression) => {
            if_expression.branches.iter().any(|(condition, value)| {
                expression_contains_linear_solve(condition)
                    || expression_contains_linear_solve(value)
            }) || expression_contains_linear_solve(&if_expression.else_value)
        }
        Expression::Array(elements) => elements.iter().any(expression_contains_linear_solve),
        Expression::Binary { lhs, rhs, .. } => {
            expression_contains_linear_solve(lhs) || expression_contains_linear_solve(rhs)
        }
        Expression::Bool(_)
        | Expression::Integer(_)
        | Expression::Real(_)
        | Expression::Ref(_)
        | Expression::Size { .. }
        | Expression::Neg(_) => false,
    }
}
