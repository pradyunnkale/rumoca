use std::collections::BTreeMap;

use super::{
    diagnostic::NumericalAnalysisError,
    facts::{
        EntityId, EntityRole, LiteralValue, NumericalFacts, OperationKind, OperationOutput, Shape,
        ValueId,
    },
    infer,
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

        let input_facts = self.facts.value(input).facts().clone();
        let output = self.facts.add_operation(
            OperationKind::Assign,
            vec![input],
            phase,
            OperationOutput::assignment(target_kind, target_shape, input_facts, target_entity),
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
            Expression::Array(elements) => self.analyze_array(elements, phase, span),
            Expression::Binary { op, lhs, rhs } => self.analyze_binary(*op, lhs, rhs, phase, span),
            Expression::Call(call) => self.analyze_call(call, phase, span),
            other => Err(NumericalAnalysisError::UnsupportedExpression {
                expression: expression_kind(other),
                span,
            }),
        }
    }

    fn analyze_array(
        &mut self,
        elements: &[Expression],
        phase: BlockMethodKind,
        span: Span,
    ) -> Result<ValueId, NumericalAnalysisError> {
        let Some(first) = elements.first() else {
            return Err(invalid_operation("array constructor cannot be empty", span));
        };
        if matches!(first, Expression::Array(_)) {
            self.analyze_matrix_constructor(elements, phase, span)
        } else {
            self.analyze_vector_constructor(elements, phase, span)
        }
    }

    fn analyze_vector_constructor(
        &mut self,
        elements: &[Expression],
        phase: BlockMethodKind,
        span: Span,
    ) -> Result<ValueId, NumericalAnalysisError> {
        let (scalar_kind, inputs) = self.analyze_scalar_elements(elements, phase, span)?;
        let shape = Shape::new(vec![inputs.len()]);
        Ok(self.facts.add_operation(
            OperationKind::ArrayConstruct,
            inputs,
            phase,
            OperationOutput::unknown(scalar_kind, shape),
        ))
    }

    fn analyze_matrix_constructor(
        &mut self,
        rows: &[Expression],
        phase: BlockMethodKind,
        span: Span,
    ) -> Result<ValueId, NumericalAnalysisError> {
        let mut scalar_kind = None;
        let mut columns = None;
        let mut inputs = Vec::new();
        for row in rows {
            let Expression::Array(elements) = row else {
                return Err(invalid_operation(
                    "matrix constructor must contain only row constructors",
                    span,
                ));
            };
            if elements.is_empty() {
                return Err(invalid_operation(
                    "matrix constructor rows cannot be empty",
                    span,
                ));
            }
            if columns.is_some_and(|expected| expected != elements.len()) {
                return Err(invalid_operation(
                    "matrix constructor rows must have equal lengths",
                    span,
                ));
            }
            columns = Some(elements.len());

            let (row_kind, row_inputs) = self.analyze_scalar_elements(elements, phase, span)?;
            if scalar_kind.is_some_and(|expected| expected != row_kind) {
                return Err(invalid_operation(
                    "matrix constructor rows must have the same scalar type",
                    span,
                ));
            }
            scalar_kind = Some(row_kind);
            inputs.extend(row_inputs);
        }

        let columns = columns.expect("non-empty matrix constructor must have a row");
        let scalar_kind = scalar_kind.expect("non-empty matrix constructor must have a type");
        let shape = Shape::new(vec![rows.len(), columns]);
        let value_facts =
            infer::matrix_constructor_facts(&self.facts, rows.len(), columns, &inputs);
        Ok(self.facts.add_operation(
            OperationKind::ArrayConstruct,
            inputs,
            phase,
            OperationOutput::inferred(scalar_kind, shape, value_facts),
        ))
    }

    fn analyze_scalar_elements(
        &mut self,
        elements: &[Expression],
        phase: BlockMethodKind,
        span: Span,
    ) -> Result<(ScalarType, Vec<ValueId>), NumericalAnalysisError> {
        let mut scalar_kind = None;
        let mut values = Vec::with_capacity(elements.len());
        for element in elements {
            let value = self.analyze_expression(element, phase, span)?;
            let analyzed = self.facts.value(value);
            if analyzed.shape().rank() != 0 {
                return Err(invalid_operation(
                    "array constructor elements must be scalar",
                    span,
                ));
            }
            if scalar_kind.is_some_and(|expected| expected != analyzed.scalar_kind()) {
                return Err(invalid_operation(
                    "array constructor elements must have the same scalar type",
                    span,
                ));
            }
            scalar_kind = Some(analyzed.scalar_kind());
            values.push(value);
        }

        let scalar_kind = scalar_kind.expect("validated array constructor must be non-empty");
        Ok((scalar_kind, values))
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
        Ok(self.facts.add_operation(
            kind,
            vec![lhs, rhs],
            phase,
            OperationOutput::unknown(scalar_kind, shape),
        ))
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
            phase,
            OperationOutput::unknown(ScalarType::Real, output_shape),
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

fn invalid_operation(detail: impl Into<String>, span: Span) -> NumericalAnalysisError {
    NumericalAnalysisError::InvalidOperation {
        detail: detail.into(),
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
