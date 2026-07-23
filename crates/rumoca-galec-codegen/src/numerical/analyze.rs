use std::collections::BTreeMap;

use super::{
    diagnostic::NumericalAnalysisError,
    facts::{EntityId, EntityRole, LiteralValue, NumericalFacts, OperationKind, Shape, ValueId},
};
use rumoca_core::Span;
use rumoca_ir_galec::ast::{
    BinaryOp, Block, BlockMethod, BlockMethodKind, Dimension, Expression, FunctionCall,
    InterfaceKind, InterfaceVariable, ProtectedEntity, ProtectedKind, Reference, ScalarType,
    Statement, TypeRef, VariableDeclaration,
};

pub(super) fn analyze(block: &Block) -> Result<NumericalFacts, NumericalAnalysisError> {
    let mut analyzer = Analyzer::new();
    analyzer.analyze_block(block)?;
    Ok(analyzer.finish())
}

struct Analyzer {
    facts: NumericalFacts,
    entities_by_name: BTreeMap<String, EntityId>,
    current_values: BTreeMap<EntityId, ValueId>,
}

impl Analyzer {
    fn new() -> Self {
        Self {
            facts: NumericalFacts::new(),
            entities_by_name: BTreeMap::new(),
            current_values: BTreeMap::new(),
        }
    }

    fn finish(self) -> NumericalFacts {
        self.facts
    }

    fn analyze_block(&mut self, block: &Block) -> Result<(), NumericalAnalysisError> {
        if let Some(compartment) = block.compartments.first() {
            return Err(NumericalAnalysisError::UnsupportedStateCompartments {
                count: block.compartments.len(),
                span: compartment.span,
            });
        }

        for variable in &block.interface {
            self.analyze_interface_variable(variable)?;
        }
        for entity in &block.protected {
            self.analyze_protected_entity(entity)?;
        }

        self.analyze_method(&block.startup, BlockMethodKind::Startup)?;
        self.analyze_method(&block.recalibrate, BlockMethodKind::Recalibrate)?;
        self.analyze_method(&block.do_step, BlockMethodKind::DoStep)
    }

    fn analyze_interface_variable(
        &mut self,
        variable: &InterfaceVariable,
    ) -> Result<(), NumericalAnalysisError> {
        let role = match variable.kind {
            InterfaceKind::Input => EntityRole::Input,
            InterfaceKind::Output => EntityRole::Output,
            InterfaceKind::TunableParameter => EntityRole::TunableParameter,
        };
        self.register_declaration(&variable.decl, role)
    }

    fn analyze_protected_entity(
        &mut self,
        entity: &ProtectedEntity,
    ) -> Result<(), NumericalAnalysisError> {
        let role = match entity.kind {
            ProtectedKind::DependentParameter => EntityRole::DependentParameter,
            ProtectedKind::Constant => EntityRole::Constant,
            ProtectedKind::State => EntityRole::State,
        };
        self.register_declaration(&entity.decl, role)
    }

    fn register_declaration(
        &mut self,
        declaration: &VariableDeclaration,
        role: EntityRole,
    ) -> Result<(), NumericalAnalysisError> {
        let name = declaration.name.lexeme().to_owned();
        if self.entities_by_name.contains_key(&name) {
            return Err(NumericalAnalysisError::DuplicateEntity {
                entity: name,
                span: declaration.span,
            });
        }

        let scalar_kind = analyze_scalar_type(&declaration.ty, &name, declaration.span)?;
        let shape = analyze_shape(&declaration.dimensions, &name, declaration.span)?;
        let entity = self
            .facts
            .add_entity(name.clone(), scalar_kind, shape, role);
        let value = self.facts.add_entity_read(entity);
        self.entities_by_name.insert(name, entity);
        self.current_values.insert(entity, value);
        Ok(())
    }

    fn analyze_method(
        &mut self,
        method: &BlockMethod,
        phase: BlockMethodKind,
    ) -> Result<(), NumericalAnalysisError> {
        if let Some(local) = method.locals.first() {
            return Err(NumericalAnalysisError::UnsupportedMethodLocal {
                entity: local.name.lexeme().to_owned(),
                span: local.span,
            });
        }

        for statement in &method.statements {
            self.analyze_statement(&statement.node, phase, statement.span)?;
        }
        Ok(())
    }

    fn analyze_statement(
        &mut self,
        statement: &Statement,
        phase: BlockMethodKind,
        span: Span,
    ) -> Result<(), NumericalAnalysisError> {
        let Statement::Assignment { target, value } = statement else {
            return Err(NumericalAnalysisError::UnsupportedStatement {
                statement: statement_kind(statement),
                span,
            });
        };

        let target_entity = self.resolve_reference(target, span)?;
        let input = self.analyze_expression(value, phase, span)?;
        let (target_kind, target_shape) = {
            let target = self.facts.entity(target_entity);
            (target.scalar_kind(), target.shape().clone())
        };
        let (input_kind, input_shape) = {
            let input = self.facts.value(input);
            (input.scalar_kind(), input.shape().clone())
        };
        if target_kind != input_kind || target_shape != input_shape {
            return Err(NumericalAnalysisError::InvalidOperation {
                detail: format!(
                    "assignment target has type {target_kind:?}{:?}, but value has type {input_kind:?}{:?}",
                    target_shape.dimensions(),
                    input_shape.dimensions()
                ),
                span,
            });
        }

        let output = self.facts.add_operation(
            OperationKind::Assign,
            vec![input],
            target_kind,
            target_shape,
            phase,
            Some(target_entity),
        );
        self.current_values.insert(target_entity, output);
        Ok(())
    }

    fn analyze_expression(
        &mut self,
        expression: &Expression,
        phase: BlockMethodKind,
        span: Span,
    ) -> Result<ValueId, NumericalAnalysisError> {
        match expression {
            Expression::Bool(value) => Ok(self.facts.add_literal(LiteralValue::Boolean(*value))),
            Expression::Integer(value) => Ok(self.facts.add_literal(LiteralValue::Integer(*value))),
            Expression::Real(value) => Ok(self.facts.add_literal(LiteralValue::Real(*value))),
            Expression::Ref(reference) => self.current_value(reference, span),
            Expression::Paren(inner) => self.analyze_expression(inner, phase, span),
            Expression::Binary { op, lhs, rhs } => self.analyze_binary(*op, lhs, rhs, phase, span),
            Expression::Call(call) => self.analyze_call(call, phase, span),
            other => Err(NumericalAnalysisError::UnsupportedExpression {
                expression: expression_kind(other),
                span,
            }),
        }
    }

    fn analyze_binary(
        &mut self,
        op: BinaryOp,
        lhs: &Expression,
        rhs: &Expression,
        phase: BlockMethodKind,
        span: Span,
    ) -> Result<ValueId, NumericalAnalysisError> {
        let kind = match op {
            BinaryOp::Add => OperationKind::Add,
            BinaryOp::Sub => OperationKind::Subtract,
            BinaryOp::Mul => OperationKind::ElementwiseMultiply,
            BinaryOp::Div => OperationKind::ElementwiseDivide,
            _ => {
                return Err(NumericalAnalysisError::UnsupportedExpression {
                    expression: "non-arithmetic binary",
                    span,
                });
            }
        };

        let lhs = self.analyze_expression(lhs, phase, span)?;
        let rhs = self.analyze_expression(rhs, phase, span)?;
        let (scalar_kind, shape) = self.binary_result(lhs, rhs, span)?;
        Ok(self
            .facts
            .add_operation(kind, vec![lhs, rhs], scalar_kind, shape, phase, None))
    }

    fn binary_result(
        &self,
        lhs: ValueId,
        rhs: ValueId,
        span: Span,
    ) -> Result<(ScalarType, Shape), NumericalAnalysisError> {
        let lhs = self.facts.value(lhs);
        let rhs = self.facts.value(rhs);
        if lhs.scalar_kind() != rhs.scalar_kind() {
            return Err(NumericalAnalysisError::InvalidOperation {
                detail: format!(
                    "binary operands have different scalar types: {:?} and {:?}",
                    lhs.scalar_kind(),
                    rhs.scalar_kind()
                ),
                span,
            });
        }

        let shape = if lhs.shape() == rhs.shape() {
            lhs.shape().clone()
        } else if lhs.shape().rank() == 0 {
            rhs.shape().clone()
        } else if rhs.shape().rank() == 0 {
            lhs.shape().clone()
        } else {
            return Err(NumericalAnalysisError::InvalidOperation {
                detail: format!(
                    "binary operand shapes {:?} and {:?} do not agree",
                    lhs.shape().dimensions(),
                    rhs.shape().dimensions()
                ),
                span,
            });
        };
        Ok((lhs.scalar_kind(), shape))
    }

    fn analyze_call(
        &mut self,
        call: &FunctionCall,
        phase: BlockMethodKind,
        span: Span,
    ) -> Result<ValueId, NumericalAnalysisError> {
        if call.function.lexeme() != "solveLinearEquations" {
            return Err(NumericalAnalysisError::UnsupportedExpression {
                expression: "unrecognized function call",
                span,
            });
        }
        if call.arguments.len() != 2 {
            return Err(NumericalAnalysisError::InvalidOperation {
                detail: format!(
                    "solveLinearEquations expects 2 arguments, found {}",
                    call.arguments.len()
                ),
                span,
            });
        }

        let matrix = self.analyze_expression(&call.arguments[0], phase, span)?;
        let rhs = self.analyze_expression(&call.arguments[1], phase, span)?;
        let output_shape = self.linear_solve_shape(matrix, rhs, span)?;
        Ok(self.facts.add_operation(
            OperationKind::LinearSolve,
            vec![matrix, rhs],
            ScalarType::Real,
            output_shape,
            phase,
            None,
        ))
    }

    fn linear_solve_shape(
        &self,
        matrix: ValueId,
        rhs: ValueId,
        span: Span,
    ) -> Result<Shape, NumericalAnalysisError> {
        let matrix = self.facts.value(matrix);
        let rhs = self.facts.value(rhs);
        let matrix_dimensions = matrix.shape().dimensions();
        let rhs_dimensions = rhs.shape().dimensions();
        let valid = matrix.scalar_kind() == ScalarType::Real
            && rhs.scalar_kind() == ScalarType::Real
            && matches!(matrix_dimensions, [rows, columns] if rows == columns)
            && matches!(rhs_dimensions, [length] if Some(length) == matrix_dimensions.first());
        if !valid {
            return Err(NumericalAnalysisError::InvalidOperation {
                detail: format!(
                    "solveLinearEquations requires square Real A and matching Real b, found A{:?} and b{:?}",
                    matrix_dimensions, rhs_dimensions
                ),
                span,
            });
        }
        Ok(Shape::new(vec![matrix_dimensions[0]]))
    }

    fn current_value(
        &self,
        reference: &Reference,
        fallback_span: Span,
    ) -> Result<ValueId, NumericalAnalysisError> {
        let entity = self.resolve_reference(reference, fallback_span)?;
        self.current_values.get(&entity).copied().ok_or_else(|| {
            NumericalAnalysisError::InvalidOperation {
                detail: "registered entity has no current value".to_owned(),
                span: fallback_span,
            }
        })
    }

    fn resolve_reference(
        &self,
        reference: &Reference,
        fallback_span: Span,
    ) -> Result<EntityId, NumericalAnalysisError> {
        let Reference::State(parts) = reference else {
            return Err(NumericalAnalysisError::UnsupportedReference {
                reference: reference_description(reference),
                span: reference_span(reference, fallback_span),
            });
        };
        let [part] = parts.as_slice() else {
            return Err(NumericalAnalysisError::UnsupportedReference {
                reference: reference_description(reference),
                span: reference_span(reference, fallback_span),
            });
        };
        if !part.subscripts.is_empty() {
            return Err(NumericalAnalysisError::UnsupportedReference {
                reference: reference_description(reference),
                span: reference_span(reference, fallback_span),
            });
        }

        let name = part.name.lexeme();
        self.entities_by_name.get(name).copied().ok_or_else(|| {
            NumericalAnalysisError::UnknownEntityReference {
                reference: name.to_owned(),
                span: reference_span(reference, fallback_span),
            }
        })
    }
}

fn analyze_scalar_type(
    ty: &TypeRef,
    entity_name: &str,
    span: Span,
) -> Result<ScalarType, NumericalAnalysisError> {
    match ty {
        TypeRef::Primitive(scalar_type) => Ok(*scalar_type),
        TypeRef::Compartment(compartment) => {
            Err(NumericalAnalysisError::UnsupportedComponentType {
                entity: entity_name.to_owned(),
                compartment: compartment.lexeme().to_owned(),
                span,
            })
        }
    }
}

fn analyze_shape(
    dimensions: &[Dimension],
    entity_name: &str,
    span: Span,
) -> Result<Shape, NumericalAnalysisError> {
    if dimensions.len() > 2 {
        return Err(NumericalAnalysisError::UnsupportedRank {
            entity: entity_name.to_owned(),
            rank: dimensions.len(),
            span,
        });
    }

    let mut sizes = Vec::with_capacity(dimensions.len());
    for (index, dimension) in dimensions.iter().enumerate() {
        sizes.push(analyze_dimension(dimension, entity_name, index + 1, span)?);
    }
    Ok(Shape::new(sizes))
}

fn analyze_dimension(
    dimension: &Dimension,
    entity_name: &str,
    position: usize,
    span: Span,
) -> Result<usize, NumericalAnalysisError> {
    let size = match dimension {
        Dimension::Expr(Expression::Integer(size)) if *size >= 1 => *size,
        Dimension::Expr(Expression::Integer(size)) => {
            return Err(invalid_dimension(
                entity_name,
                position,
                format!("expected an integer >= 1, found {size}"),
                span,
            ));
        }
        Dimension::Expr(_) => {
            return Err(invalid_dimension(
                entity_name,
                position,
                "expected a positive integer literal".to_owned(),
                span,
            ));
        }
        Dimension::Derived => {
            return Err(invalid_dimension(
                entity_name,
                position,
                "derived dimensions (`:`) are not supported".to_owned(),
                span,
            ));
        }
    };

    usize::try_from(size).map_err(|_| {
        invalid_dimension(
            entity_name,
            position,
            format!("size {size} does not fit the target index type"),
            span,
        )
    })
}

fn invalid_dimension(
    entity_name: &str,
    dimension: usize,
    reason: String,
    span: Span,
) -> NumericalAnalysisError {
    NumericalAnalysisError::InvalidDimension {
        entity: entity_name.to_owned(),
        dimension,
        reason,
        span,
    }
}

fn statement_kind(statement: &Statement) -> &'static str {
    match statement {
        Statement::Assignment { .. } => "assignment",
        Statement::MultiAssignment { .. } => "multi-assignment",
        Statement::Call(_) => "call",
        Statement::If(_) => "if",
        Statement::For(_) => "for-loop",
        Statement::Limit(_) => "limit",
        Statement::Signal(_) => "signal",
    }
}

fn expression_kind(expression: &Expression) -> &'static str {
    match expression {
        Expression::Bool(_) => "Boolean literal",
        Expression::Integer(_) => "Integer literal",
        Expression::Real(_) => "Real literal",
        Expression::Ref(_) => "reference",
        Expression::Size { .. } => "size query",
        Expression::Call(_) => "function call",
        Expression::Paren(_) => "parenthesized",
        Expression::If(_) => "if",
        Expression::Array(_) => "array constructor",
        Expression::Neg(_) => "negation",
        Expression::Not(_) => "logical-not",
        Expression::Binary { .. } => "binary",
    }
}

fn reference_description(reference: &Reference) -> String {
    match reference {
        Reference::Local(part) => part.name.lexeme().to_owned(),
        Reference::State(parts) => format!(
            "self.{}",
            parts
                .iter()
                .map(|part| part.name.lexeme())
                .collect::<Vec<_>>()
                .join(".")
        ),
    }
}

fn reference_span(reference: &Reference, fallback: Span) -> Span {
    let span = match reference {
        Reference::Local(part) => part.span,
        Reference::State(parts) => parts.first().map_or(fallback, |part| part.span),
    };
    if span.is_dummy() { fallback } else { span }
}

#[cfg(test)]
mod tests;
