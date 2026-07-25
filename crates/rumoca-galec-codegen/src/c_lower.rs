//! GALEC AST → structured C codegen IR for the `galec-c` export (SPEC_0034
//! GAL-024).
//!
//! Scope: exactly the AST shape [`crate::lower`] emits today — nested
//! [`Statement::Assignment`] and Boolean [`Statement::If`] trees over
//! single-part `self.` state references,
//! with expressions built from literals, references, the emittable §3.2.6
//! builtin calls ([`crate::lower::emittable_builtin_targets`]),
//! parentheses, if-expressions, `not`, unary minus over references, binary
//! operations, and whole-array start literals. Anything outside that shape
//! fails with a typed `ET023` — never silently dropped (GAL-007).
//!
//! Semantics represented for the C templates:
//!
//! - `and`/`or`/`not` → `&&`/`||`/`!`; `<>` → `!=`; `^` → `pow(…)`
//!   (GALEC `^` returns Real for numeric operands);
//! - every composite subexpression is parenthesized, so the AST shape — the
//!   normative GALEC evaluation order (trap T6) — survives verbatim and
//!   nested unary/binary forms can never re-associate;
//! - Real literals reuse the strict GALEC formatter (trap T7); its output
//!   (`1.0e+5`) is a valid C `double` literal;
//! - GALEC subscripts are 1-based, C subscripts 0-based: literal indices
//!   shift at print time, expression indices print as `(… - 1)`;
//! - Integer is `int32_t`, so literals outside its range are rejected
//!   rather than truncated;
//! - builtin identity and arguments stay semantic in the context; the
//!   templates own C99 names and helper selection.

use rumoca_ir_galec::ast::{
    BinaryOp, Condition, Expression, IfExpression, Name, Reference, Spanned, Statement,
};

use crate::c_mangle::CNameTable;
use crate::diagnostic::GalecTargetError;

/// GALEC-statement/-expression → structured C codegen IR lowerer over one
/// package's collision-checked C name table.
pub struct CContextLowerer<'a> {
    names: &'a CNameTable,
}

impl<'a> CContextLowerer<'a> {
    /// Lowerer over the package's C name table.
    #[must_use]
    pub fn new(names: &'a CNameTable) -> Self {
        Self { names }
    }

    /// Lower one GALEC statement to structured, serializable C codegen IR.
    ///
    /// This is the production codegen boundary: array-valued assignments are
    /// normalized here, but no C source text is produced. The minijinja
    /// templates own all C spelling and punctuation.
    pub fn statement_contexts(
        &self,
        statement: &Statement,
    ) -> Result<Vec<serde_json::Value>, GalecTargetError> {
        match statement {
            Statement::Assignment { target, value } => self.assignment_contexts(target, value),
            Statement::If(if_statement) => self.if_statement_context(if_statement),
            Statement::MultiAssignment { .. } => {
                Err(unsupported_statement("a multi-assignment statement"))
            }
            Statement::Call(_) => Err(unsupported_statement("a bare call statement")),
            Statement::For(_) => Err(unsupported_statement("a for loop")),
            Statement::Limit(_) => Err(unsupported_statement("a limit statement")),
            Statement::Signal(_) => Err(unsupported_statement("a signal statement")),
        }
    }

    /// Lower an ordered GALEC statement block.
    ///
    /// Complete multi-assignment numerical idioms are recognized here,
    /// before their component assignments lose their shared operation
    /// structure. An incomplete or non-exact pattern falls back to ordinary
    /// statement lowering without changing its evaluation order.
    pub fn statements_contexts(
        &self,
        statements: &[Spanned<Statement>],
    ) -> Result<Vec<serde_json::Value>, GalecTargetError> {
        let mut contexts = Vec::new();
        let mut index = 0;
        while index < statements.len() {
            if let Some((context, consumed)) =
                self.multi_assignment_helper_context(&statements[index..])?
            {
                contexts.push(context);
                index += consumed;
            } else {
                contexts.extend(self.statement_contexts(&statements[index].node)?);
                index += 1;
            }
        }
        Ok(contexts)
    }

    fn if_statement_context(
        &self,
        statement: &rumoca_ir_galec::ast::IfStatement,
    ) -> Result<Vec<serde_json::Value>, GalecTargetError> {
        let branches = statement
            .branches
            .iter()
            .map(|branch| {
                let Condition::Expression(condition) = &branch.condition else {
                    return Err(unsupported_statement(
                        "an if statement with an error-signal condition",
                    ));
                };
                Ok(serde_json::json!({
                    "condition": self.expression_context(condition)?,
                    "body": self.body_contexts(&branch.body)?,
                }))
            })
            .collect::<Result<Vec<_>, GalecTargetError>>()?;
        let else_body = statement
            .else_body
            .as_ref()
            .map(|body| self.body_contexts(body))
            .transpose()?;
        Ok(vec![serde_json::json!({
            "kind": "if",
            "branches": branches,
            "else_body": else_body,
        })])
    }

    fn body_contexts(
        &self,
        statements: &[Spanned<Statement>],
    ) -> Result<Vec<serde_json::Value>, GalecTargetError> {
        self.statements_contexts(statements)
    }

    fn assignment_contexts(
        &self,
        target: &Reference,
        value: &Expression,
    ) -> Result<Vec<serde_json::Value>, GalecTargetError> {
        if let Expression::Array(elements) = value {
            let mut assignments = Vec::new();
            self.array_assignment_context(target, elements, &mut Vec::new(), &mut assignments)?;
            return Ok(assignments);
        }
        if let Expression::Ref(source) = value
            && self.is_whole_array_reference(target)
            && self.is_whole_array_reference(source)
        {
            return Ok(vec![serde_json::json!({
                "kind": "copy",
                "target": self.reference_context(target)?,
                "source": self.reference_context(source)?,
            })]);
        }
        if let Some(dimensions) = self.whole_array_dimensions(target) {
            if let Some(context) = self.special_array_context(target, dimensions, value)? {
                return Ok(vec![context]);
            }
            if let Some(context) = self.simple_array_binary_context(target, dimensions, value)? {
                return Ok(vec![context]);
            }
            let mut assignments = Vec::new();
            self.array_expression_context(
                target,
                dimensions,
                value,
                &mut Vec::new(),
                &mut assignments,
            )?;
            return Ok(assignments);
        }
        Ok(vec![self.assignment_context(target, value)?])
    }

    fn array_assignment_context(
        &self,
        target: &Reference,
        elements: &[Expression],
        indices: &mut Vec<i64>,
        assignments: &mut Vec<serde_json::Value>,
    ) -> Result<(), GalecTargetError> {
        for (index, element) in elements.iter().enumerate() {
            let one_based =
                i64::try_from(index + 1).map_err(|_| GalecTargetError::LoweringInternal {
                    detail: "C export array index exceeds i64".to_owned(),
                })?;
            indices.push(one_based);
            match element {
                Expression::Array(nested) => {
                    self.array_assignment_context(target, nested, indices, assignments)?;
                }
                scalar => assignments.push(self.assignment_context(
                    &self.reference_with_static_subscripts(target, indices)?,
                    scalar,
                )?),
            }
            indices.pop();
        }
        Ok(())
    }

    fn array_expression_context(
        &self,
        target: &Reference,
        dimensions: &[i64],
        value: &Expression,
        indices: &mut Vec<i64>,
        assignments: &mut Vec<serde_json::Value>,
    ) -> Result<(), GalecTargetError> {
        let Some((first, rest)) = dimensions.split_first() else {
            let indexed_target = self.reference_with_static_subscripts(target, indices)?;
            let indexed_value = self.indexed_expression(value, indices)?;
            assignments.push(self.assignment_context(&indexed_target, &indexed_value)?);
            return Ok(());
        };
        let size = usize::try_from(*first)
            .ok()
            .filter(|size| *size >= 1)
            .ok_or_else(|| GalecTargetError::LoweringInternal {
                detail: format!("C export saw non-positive array dimension {first}"),
            })?;
        for index in 0..size {
            let one_based =
                i64::try_from(index + 1).map_err(|_| GalecTargetError::LoweringInternal {
                    detail: "C export array index exceeds i64".to_owned(),
                })?;
            indices.push(one_based);
            self.array_expression_context(target, rest, value, indices, assignments)?;
            indices.pop();
        }
        Ok(())
    }

    fn assignment_context(
        &self,
        target: &Reference,
        value: &Expression,
    ) -> Result<serde_json::Value, GalecTargetError> {
        Ok(serde_json::json!({
            "kind": "assign",
            "target": self.reference_context(target)?,
            "value": self.expression_context(value)?,
        }))
    }

    fn reference_context(
        &self,
        reference: &Reference,
    ) -> Result<serde_json::Value, GalecTargetError> {
        let Reference::State(parts) = reference else {
            return Err(GalecTargetError::CExportUnsupported {
                construct: "a local (non-`self.`) reference",
                detail: "the current DAE lowering emits no method locals or loop iterators"
                    .to_owned(),
            });
        };
        let [part] = parts.as_slice() else {
            return Err(GalecTargetError::CExportUnsupported {
                construct: "a multi-part state reference",
                detail: "the current DAE lowering emits no state compartments".to_owned(),
            });
        };
        let indices = part
            .subscripts
            .iter()
            .map(|subscript| match subscript {
                Expression::Integer(value) if *value >= 1 => Ok(serde_json::json!({
                    "kind": "literal",
                    "value": value - 1,
                })),
                Expression::Integer(value) => Err(GalecTargetError::LoweringInternal {
                    detail: format!(
                        "C export met the GALEC subscript {value}; valid subscripts are 1-based positive integers"
                    ),
                }),
                other => Ok(serde_json::json!({
                    "kind": "expression",
                    "value": self.expression_context(other)?,
                })),
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(serde_json::json!({
            "name": self.names.c_name(&part.name)?,
            "indices": indices,
        }))
    }

    fn expression_context(
        &self,
        expression: &Expression,
    ) -> Result<serde_json::Value, GalecTargetError> {
        if let Some(dot) = self.dot_product_context(expression)? {
            return Ok(dot);
        }
        Ok(match expression {
            Expression::Bool(value) => serde_json::json!({"kind": "bool", "value": value}),
            Expression::Integer(value) => {
                validate_integer_literal(*value)?;
                serde_json::json!({"kind": "integer", "value": value})
            }
            Expression::Real(value) => {
                let literal = rumoca_ir_galec::format_real_literal(*value).map_err(|error| {
                    GalecTargetError::LoweringInternal {
                        detail: format!("C export met an unprintable Real literal: {error}"),
                    }
                })?;
                serde_json::json!({"kind": "real", "literal": literal, "negative": value.is_sign_negative()})
            }
            Expression::Ref(reference) => {
                serde_json::json!({"kind": "ref", "reference": self.reference_context(reference)?})
            }
            Expression::Call(call) => {
                let Name::Ident(function, _) = &call.function else {
                    return Err(GalecTargetError::LoweringInternal {
                        detail: "C export met a call to a quoted function name".to_owned(),
                    });
                };
                let Some((_, arity)) = crate::lower::emittable_builtin_targets()
                    .into_iter()
                    .find(|(name, _)| *name == function.as_str())
                else {
                    return Err(GalecTargetError::LoweringInternal {
                        detail: format!("C export has no mapping for `{}`", function.as_str()),
                    });
                };
                if call.arguments.len() != arity {
                    return Err(GalecTargetError::LoweringInternal {
                        detail: format!(
                            "C export met `{}` with {} argument(s), expected {arity}",
                            function.as_str(),
                            call.arguments.len()
                        ),
                    });
                }
                serde_json::json!({
                    "kind": "call",
                    "function": function.as_str(),
                    "arguments": call.arguments.iter()
                        .map(|arg| self.expression_context(arg))
                        .collect::<Result<Vec<_>, _>>()?,
                })
            }
            Expression::Paren(inner) => {
                serde_json::json!({"kind": "paren", "value": self.expression_context(inner)?})
            }
            Expression::If(if_expression) => serde_json::json!({
                "kind": "if",
                "branches": if_expression.branches.iter().map(|(condition, value)| {
                    Ok(serde_json::json!({
                        "condition": self.expression_context(condition)?,
                        "value": self.expression_context(value)?,
                    }))
                }).collect::<Result<Vec<_>, GalecTargetError>>()?,
                "else_value": self.expression_context(&if_expression.else_value)?,
            }),
            Expression::Neg(reference) => {
                serde_json::json!({"kind": "neg", "reference": self.reference_context(reference)?})
            }
            Expression::Not(inner) => {
                serde_json::json!({"kind": "not", "value": self.expression_context(inner)?})
            }
            Expression::Binary { op, lhs, rhs } => serde_json::json!({
                "kind": "binary",
                "operator": binary_op_name(*op),
                "lhs": self.expression_context(lhs)?,
                "rhs": self.expression_context(rhs)?,
            }),
            Expression::Size { .. } => {
                return Err(GalecTargetError::CExportUnsupported {
                    construct: "a `size(…)` expression",
                    detail: "the current DAE lowering never emits dimension queries".to_owned(),
                });
            }
            Expression::Array(_) => {
                return Err(GalecTargetError::CExportUnsupported {
                    construct: "an array constructor outside a whole-array assignment",
                    detail: "C has no array-valued expressions".to_owned(),
                });
            }
        })
    }

    fn multi_assignment_helper_context(
        &self,
        statements: &[Spanned<Statement>],
    ) -> Result<Option<(serde_json::Value, usize)>, GalecTargetError> {
        if let Some(context) = self.quaternion_integrate_context(statements)? {
            return Ok(Some((context, 4)));
        }
        if let Some(context) = self.quaternion_gravity_context(statements)? {
            return Ok(Some((context, 3)));
        }
        if let Some(context) = self.cross3_context(statements)? {
            return Ok(Some((context, 3)));
        }
        Ok(None)
    }

    fn cross3_context(
        &self,
        statements: &[Spanned<Statement>],
    ) -> Result<Option<serde_json::Value>, GalecTargetError> {
        let Some((target, values)) = component_assignment_group(statements, 3) else {
            return Ok(None);
        };
        if !self.is_real_vector(&target, 3) {
            return Ok(None);
        }
        let Expression::Binary {
            op: BinaryOp::Sub,
            lhs,
            ..
        } = unparen(values[0])
        else {
            return Ok(None);
        };
        let Expression::Binary {
            op: BinaryOp::Mul,
            lhs,
            rhs,
        } = unparen(lhs)
        else {
            return Ok(None);
        };
        let Some((a, 2)) = indexed_vector_reference(unparen(lhs)) else {
            return Ok(None);
        };
        let Some((b, 3)) = indexed_vector_reference(unparen(rhs)) else {
            return Ok(None);
        };
        if !self.is_real_vector(&a, 3) || !self.is_real_vector(&b, 3) {
            return Ok(None);
        }
        let expected = [
            sub(
                mul(element(&a, 2), element(&b, 3)),
                mul(element(&a, 3), element(&b, 2)),
            ),
            sub(
                mul(element(&a, 3), element(&b, 1)),
                mul(element(&a, 1), element(&b, 3)),
            ),
            sub(
                mul(element(&a, 1), element(&b, 2)),
                mul(element(&a, 2), element(&b, 1)),
            ),
        ];
        if !values
            .iter()
            .zip(expected.iter())
            .all(|(actual, expected)| expressions_match(actual, expected))
        {
            return Ok(None);
        }
        Ok(Some(serde_json::json!({
            "kind": "cross3",
            "target": self.reference_context(&target)?,
            "lhs": self.reference_context(&a)?,
            "rhs": self.reference_context(&b)?,
            "dimensions": [3],
        })))
    }

    fn quaternion_gravity_context(
        &self,
        statements: &[Spanned<Statement>],
    ) -> Result<Option<serde_json::Value>, GalecTargetError> {
        let Some((target, values)) = component_assignment_group(statements, 3) else {
            return Ok(None);
        };
        if !self.is_real_vector(&target, 3) {
            return Ok(None);
        }
        let Some(quaternion) = first_indexed_real_vector(values[0], self.names, 4) else {
            return Ok(None);
        };
        let q = |index| element(&quaternion, index);
        let expected = [
            mul(Expression::Real(2.0), sub(mul(q(2), q(4)), mul(q(1), q(3)))),
            mul(Expression::Real(2.0), add(mul(q(1), q(2)), mul(q(3), q(4)))),
            add(
                sub(sub(mul(q(1), q(1)), mul(q(2), q(2))), mul(q(3), q(3))),
                mul(q(4), q(4)),
            ),
        ];
        if !values
            .iter()
            .zip(expected.iter())
            .all(|(actual, expected)| expressions_match(actual, expected))
        {
            return Ok(None);
        }
        Ok(Some(serde_json::json!({
            "kind": "quat_gravity",
            "target": self.reference_context(&target)?,
            "quaternion": self.reference_context(&quaternion)?,
            "target_dimensions": [3],
            "quaternion_dimensions": [4],
        })))
    }

    fn quaternion_integrate_context(
        &self,
        statements: &[Spanned<Statement>],
    ) -> Result<Option<serde_json::Value>, GalecTargetError> {
        let Some((target, values)) = component_assignment_group(statements, 4) else {
            return Ok(None);
        };
        if !self.is_real_vector(&target, 4) {
            return Ok(None);
        }
        let Expression::Binary {
            op: BinaryOp::Add,
            lhs,
            rhs,
        } = unparen(values[0])
        else {
            return Ok(None);
        };
        let Some((quaternion, 1)) = indexed_vector_reference(unparen(lhs)) else {
            return Ok(None);
        };
        if !self.is_real_vector(&quaternion, 4) {
            return Ok(None);
        }
        let Expression::Binary {
            op: BinaryOp::Mul,
            lhs: scaled_rate,
            rhs: sample_period,
        } = unparen(rhs)
        else {
            return Ok(None);
        };
        if !self.is_array_free_scalar_expression(sample_period) {
            return Ok(None);
        }
        let Some(angular_rate) = first_indexed_real_vector(scaled_rate, self.names, 3) else {
            return Ok(None);
        };

        let q = |index| element(&quaternion, index);
        let w = |index| element(&angular_rate, index);
        let half = Expression::Real(0.5);
        let zero = Expression::Real(0.0);
        let scaled = |value| mul(mul(half.clone(), value), unparen(sample_period).clone());
        let expected = [
            add(
                q(1),
                scaled(sub(
                    sub(sub(zero, mul(q(2), w(1))), mul(q(3), w(2))),
                    mul(q(4), w(3)),
                )),
            ),
            add(
                q(2),
                scaled(sub(add(mul(q(1), w(1)), mul(q(3), w(3))), mul(q(4), w(2)))),
            ),
            add(
                q(3),
                scaled(add(sub(mul(q(1), w(2)), mul(q(2), w(3))), mul(q(4), w(1)))),
            ),
            add(
                q(4),
                scaled(sub(add(mul(q(1), w(3)), mul(q(2), w(2))), mul(q(3), w(1)))),
            ),
        ];
        if !values
            .iter()
            .zip(expected.iter())
            .all(|(actual, expected)| expressions_match(actual, expected))
        {
            return Ok(None);
        }
        Ok(Some(serde_json::json!({
            "kind": "quat_integrate",
            "target": self.reference_context(&target)?,
            "quaternion": self.reference_context(&quaternion)?,
            "angular_rate": self.reference_context(&angular_rate)?,
            "sample_period": self.expression_context(sample_period)?,
            "target_dimensions": [4],
            "quaternion_dimensions": [4],
            "angular_rate_dimensions": [3],
        })))
    }

    fn is_real_vector(&self, reference: &Reference, length: i64) -> bool {
        let Some(name) = single_part_name(reference) else {
            return false;
        };
        self.names.scalar_type(name) == Some(rumoca_ir_galec::ast::ScalarType::Real)
            && self.names.array_dimensions(name) == Some([length].as_slice())
    }

    fn special_array_context(
        &self,
        target: &Reference,
        dimensions: &[i64],
        value: &Expression,
    ) -> Result<Option<serde_json::Value>, GalecTargetError> {
        if let Some(context) = self.array_add_add_scaled_context(target, dimensions, value)? {
            return Ok(Some(context));
        }
        if let Some(context) = self.array_add_scaled2_context(target, dimensions, value)? {
            return Ok(Some(context));
        }
        self.quaternion_normalize_context(target, dimensions, value)
    }

    fn array_add_add_scaled_context(
        &self,
        target: &Reference,
        dimensions: &[i64],
        value: &Expression,
    ) -> Result<Option<serde_json::Value>, GalecTargetError> {
        let Expression::Binary {
            op: BinaryOp::Add,
            lhs: sum,
            rhs: scaled,
        } = unparen(value)
        else {
            return Ok(None);
        };
        let Expression::Binary {
            op: BinaryOp::Add,
            lhs,
            rhs,
        } = unparen(sum)
        else {
            return Ok(None);
        };
        let Some(lhs) = self.whole_real_array_operand(lhs, dimensions) else {
            return Ok(None);
        };
        let Some(rhs) = self.whole_real_array_operand(rhs, dimensions) else {
            return Ok(None);
        };
        let Some((scale, scaled_array)) = self.scalar_times_array(scaled, dimensions) else {
            return Ok(None);
        };
        Ok(Some(serde_json::json!({
            "kind": "array_add_add_scaled",
            "target": self.reference_context(target)?,
            "lhs": self.reference_context(&lhs)?,
            "rhs": self.reference_context(&rhs)?,
            "scale": self.expression_context(scale)?,
            "scaled": self.reference_context(&scaled_array)?,
            "dimensions": dimensions,
            "element_count": element_count(dimensions)?,
        })))
    }

    fn array_add_scaled2_context(
        &self,
        target: &Reference,
        dimensions: &[i64],
        value: &Expression,
    ) -> Result<Option<serde_json::Value>, GalecTargetError> {
        let Expression::Binary {
            op: BinaryOp::Add,
            lhs: base,
            rhs: scaled,
        } = unparen(value)
        else {
            return Ok(None);
        };
        let Some(base) = self.whole_real_array_operand(base, dimensions) else {
            return Ok(None);
        };
        let Expression::Binary {
            op: BinaryOp::Mul,
            lhs: first_product,
            rhs: scale2,
        } = unparen(scaled)
        else {
            return Ok(None);
        };
        if self.expression_needs_indexing(scale2) {
            return Ok(None);
        }
        let Some((scale1, scaled_array)) = self.scalar_times_array(first_product, dimensions)
        else {
            return Ok(None);
        };
        Ok(Some(serde_json::json!({
            "kind": "array_add_scaled2",
            "target": self.reference_context(target)?,
            "base": self.reference_context(&base)?,
            "scale1": self.expression_context(scale1)?,
            "scaled": self.reference_context(&scaled_array)?,
            "scale2": self.expression_context(scale2)?,
            "dimensions": dimensions,
            "element_count": element_count(dimensions)?,
        })))
    }

    fn quaternion_normalize_context(
        &self,
        target: &Reference,
        dimensions: &[i64],
        value: &Expression,
    ) -> Result<Option<serde_json::Value>, GalecTargetError> {
        if dimensions != [4] {
            return Ok(None);
        }
        let Expression::If(if_expression) = unparen(value) else {
            return Ok(None);
        };
        let [(condition, normalized)] = if_expression.branches.as_slice() else {
            return Ok(None);
        };
        let Expression::Binary {
            op: BinaryOp::Div,
            lhs: quaternion,
            rhs: norm,
        } = unparen(normalized)
        else {
            return Ok(None);
        };
        let Some(quaternion) = self.whole_real_array_operand(quaternion, dimensions) else {
            return Ok(None);
        };
        if !self.is_array_free_scalar_expression(norm)
            || !self.is_array_free_scalar_expression(condition)
            || !is_quaternion_identity_literal(&if_expression.else_value)
        {
            return Ok(None);
        }
        Ok(Some(serde_json::json!({
            "kind": "quat_normalize_if",
            "target": self.reference_context(target)?,
            "quaternion": self.reference_context(&quaternion)?,
            "norm": self.expression_context(norm)?,
            "condition": self.expression_context(condition)?,
            "dimensions": dimensions,
        })))
    }

    fn scalar_times_array<'b>(
        &self,
        expression: &'b Expression,
        dimensions: &[i64],
    ) -> Option<(&'b Expression, Reference)> {
        let Expression::Binary {
            op: BinaryOp::Mul,
            lhs,
            rhs,
        } = unparen(expression)
        else {
            return None;
        };
        if let Some(array) = self.whole_real_array_operand(rhs, dimensions)
            && self.is_array_free_scalar_expression(lhs)
        {
            return Some((lhs, array));
        }
        if let Some(array) = self.whole_real_array_operand(lhs, dimensions)
            && self.is_array_free_scalar_expression(rhs)
        {
            return Some((rhs, array));
        }
        None
    }

    fn whole_real_array_operand(
        &self,
        expression: &Expression,
        dimensions: &[i64],
    ) -> Option<Reference> {
        let Expression::Ref(reference) = unparen(expression) else {
            return None;
        };
        let name = single_part_name(reference)?;
        (self.whole_array_dimensions(reference) == Some(dimensions)
            && self.names.scalar_type(name) == Some(rumoca_ir_galec::ast::ScalarType::Real))
        .then(|| reference.clone())
    }

    fn is_array_free_scalar_expression(&self, expression: &Expression) -> bool {
        match unparen(expression) {
            Expression::Bool(_) | Expression::Integer(_) | Expression::Real(_) => true,
            Expression::Ref(reference) | Expression::Neg(reference) => single_part_name(reference)
                .is_some_and(|name| self.names.array_dimensions(name).is_none()),
            Expression::Call(call) => call
                .arguments
                .iter()
                .all(|argument| self.is_array_free_scalar_expression(argument)),
            Expression::Not(inner) => self.is_array_free_scalar_expression(inner),
            Expression::If(if_expression) => {
                if_expression.branches.iter().all(|(condition, value)| {
                    self.is_array_free_scalar_expression(condition)
                        && self.is_array_free_scalar_expression(value)
                }) && self.is_array_free_scalar_expression(&if_expression.else_value)
            }
            Expression::Binary { lhs, rhs, .. } => {
                self.is_array_free_scalar_expression(lhs)
                    && self.is_array_free_scalar_expression(rhs)
            }
            Expression::Paren(inner) => self.is_array_free_scalar_expression(inner),
            Expression::Array(_) | Expression::Size { .. } => false,
        }
    }

    fn simple_array_binary_context(
        &self,
        target: &Reference,
        dimensions: &[i64],
        value: &Expression,
    ) -> Result<Option<serde_json::Value>, GalecTargetError> {
        let Some(target_name) = single_part_name(target) else {
            return Ok(None);
        };
        if self.names.scalar_type(target_name) != Some(rumoca_ir_galec::ast::ScalarType::Real) {
            return Ok(None);
        }
        let Expression::Binary { op, lhs, rhs } = value else {
            return Ok(None);
        };
        if !matches!(
            op,
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div
        ) {
            return Ok(None);
        }
        let Some(lhs) = self.array_binary_operand_context(lhs, dimensions)? else {
            return Ok(None);
        };
        let Some(rhs) = self.array_binary_operand_context(rhs, dimensions)? else {
            return Ok(None);
        };
        if lhs["kind"] == "scalar" && rhs["kind"] == "scalar" {
            return Ok(None);
        }
        let element_count = element_count(dimensions)?;
        Ok(Some(serde_json::json!({
            "kind": "array_binary",
            "operator": binary_op_name(*op),
            "target": self.reference_context(target)?,
            "dimensions": dimensions,
            "element_count": element_count,
            "lhs": lhs,
            "rhs": rhs,
        })))
    }

    fn array_binary_operand_context(
        &self,
        expression: &Expression,
        dimensions: &[i64],
    ) -> Result<Option<serde_json::Value>, GalecTargetError> {
        if let Expression::Ref(reference) = expression
            && self.whole_array_dimensions(reference) == Some(dimensions)
            && single_part_name(reference).and_then(|name| self.names.scalar_type(name))
                == Some(rumoca_ir_galec::ast::ScalarType::Real)
        {
            return Ok(Some(serde_json::json!({
                "kind": "array",
                "reference": self.reference_context(reference)?,
            })));
        }
        if self.is_array_free_scalar_expression(expression) {
            return Ok(Some(serde_json::json!({
                "kind": "scalar",
                "value": self.expression_context(expression)?,
            })));
        }
        Ok(None)
    }

    fn dot_product_context(
        &self,
        expression: &Expression,
    ) -> Result<Option<serde_json::Value>, GalecTargetError> {
        let mut terms = Vec::new();
        if !collect_left_associated_add_terms(expression, &mut terms) {
            return Ok(None);
        }
        let Some(first) = terms.first() else {
            return Ok(None);
        };
        let Some((lhs, rhs, first_index)) = dot_term(first) else {
            return Ok(None);
        };
        let Some(lhs_name) = single_part_name(&lhs) else {
            return Ok(None);
        };
        let Some(rhs_name) = single_part_name(&rhs) else {
            return Ok(None);
        };
        if self.names.scalar_type(lhs_name) != Some(rumoca_ir_galec::ast::ScalarType::Real)
            || self.names.scalar_type(rhs_name) != Some(rumoca_ir_galec::ast::ScalarType::Real)
        {
            return Ok(None);
        }
        let Some(lhs_dimensions @ [length]) = self.names.array_dimensions(lhs_name) else {
            return Ok(None);
        };
        if self.names.array_dimensions(rhs_name) != Some(lhs_dimensions)
            || terms.len() != usize::try_from(*length).unwrap_or(0)
            || first_index != 1
        {
            return Ok(None);
        }
        for (offset, term) in terms.iter().enumerate().skip(1) {
            let Some((term_lhs, term_rhs, index)) = dot_term(term) else {
                return Ok(None);
            };
            if term_lhs != lhs
                || term_rhs != rhs
                || index != i64::try_from(offset + 1).unwrap_or(i64::MAX)
            {
                return Ok(None);
            }
        }
        Ok(Some(serde_json::json!({
            "kind": "dot",
            "lhs": self.reference_context(&lhs)?,
            "rhs": self.reference_context(&rhs)?,
            "dimensions": [length],
            "element_count": length,
        })))
    }

    fn indexed_expression(
        &self,
        expression: &Expression,
        indices: &[i64],
    ) -> Result<Expression, GalecTargetError> {
        if indices.is_empty() {
            return Ok(expression.clone());
        }
        match expression {
            Expression::Ref(reference) if self.is_whole_array_reference(reference) => Ok(
                Expression::Ref(self.reference_with_static_subscripts(reference, indices)?),
            ),
            Expression::Ref(_) => Ok(expression.clone()),
            Expression::Neg(reference) if self.is_whole_array_reference(reference) => Ok(
                Expression::Neg(self.reference_with_static_subscripts(reference, indices)?),
            ),
            Expression::Neg(_) => Ok(expression.clone()),
            Expression::Array(elements) => self.indexed_array_element(elements, indices),
            Expression::If(if_expression) => Ok(Expression::If(IfExpression {
                branches: if_expression
                    .branches
                    .iter()
                    .map(|(condition, value)| {
                        Ok((
                            condition.clone(),
                            self.index_value_if_array(value, indices)?,
                        ))
                    })
                    .collect::<Result<Vec<_>, GalecTargetError>>()?,
                else_value: Box::new(
                    self.index_value_if_array(&if_expression.else_value, indices)?,
                ),
            })),
            Expression::Paren(inner) if self.expression_needs_indexing(inner) => Ok(
                Expression::Paren(Box::new(self.indexed_expression(inner, indices)?)),
            ),
            Expression::Binary { op, lhs, rhs } => Ok(Expression::Binary {
                op: *op,
                lhs: Box::new(self.index_value_if_array(lhs, indices)?),
                rhs: Box::new(self.index_value_if_array(rhs, indices)?),
            }),
            Expression::Bool(_)
            | Expression::Integer(_)
            | Expression::Real(_)
            | Expression::Call(_)
            | Expression::Paren(_)
            | Expression::Not(_)
            | Expression::Size { .. } => Ok(expression.clone()),
        }
    }

    fn index_value_if_array(
        &self,
        expression: &Expression,
        indices: &[i64],
    ) -> Result<Expression, GalecTargetError> {
        if self.expression_needs_indexing(expression) {
            self.indexed_expression(expression, indices)
        } else {
            Ok(expression.clone())
        }
    }

    fn indexed_array_element(
        &self,
        elements: &[Expression],
        indices: &[i64],
    ) -> Result<Expression, GalecTargetError> {
        let Some((first, rest)) = indices.split_first() else {
            return Err(GalecTargetError::LoweringInternal {
                detail: "C export array element selection called without indices".to_owned(),
            });
        };
        let index = usize::try_from(*first)
            .ok()
            .filter(|index| *index >= 1 && *index <= elements.len())
            .ok_or_else(|| GalecTargetError::LoweringInternal {
                detail: format!(
                    "C export array constructor index {first} is outside 1..{}",
                    elements.len()
                ),
            })?;
        let selected = &elements[index - 1];
        if rest.is_empty() {
            return Ok(selected.clone());
        }
        if matches!(selected, Expression::Array(_)) || self.expression_needs_indexing(selected) {
            return self.indexed_expression(selected, rest);
        }
        Err(GalecTargetError::LoweringInternal {
            detail: "C export array constructor rank does not match target dimensions".to_owned(),
        })
    }

    fn expression_needs_indexing(&self, expression: &Expression) -> bool {
        match expression {
            Expression::Ref(reference) | Expression::Neg(reference) => {
                self.is_whole_array_reference(reference)
            }
            Expression::Array(_) => true,
            Expression::If(if_expression) => {
                if_expression
                    .branches
                    .iter()
                    .any(|(_, value)| self.expression_needs_indexing(value))
                    || self.expression_needs_indexing(&if_expression.else_value)
            }
            Expression::Paren(inner) | Expression::Not(inner) => {
                self.expression_needs_indexing(inner)
            }
            Expression::Binary { lhs, rhs, .. } => {
                self.expression_needs_indexing(lhs) || self.expression_needs_indexing(rhs)
            }
            Expression::Bool(_)
            | Expression::Integer(_)
            | Expression::Real(_)
            | Expression::Call(_)
            | Expression::Size { .. } => false,
        }
    }

    fn reference_with_static_subscripts(
        &self,
        reference: &Reference,
        indices: &[i64],
    ) -> Result<Reference, GalecTargetError> {
        let Reference::State(parts) = reference else {
            return Err(GalecTargetError::LoweringInternal {
                detail: "C export can only index whole-array state references".to_owned(),
            });
        };
        let [part] = parts.as_slice() else {
            return Err(GalecTargetError::LoweringInternal {
                detail: "C export can only index single-part state references".to_owned(),
            });
        };
        let mut part = part.clone();
        part.subscripts
            .extend(indices.iter().copied().map(Expression::Integer));
        Ok(Reference::State(vec![part]))
    }

    /// Literal dimensions when `reference` is a whole-array state reference:
    /// a single-part `self.x` reference with NO subscripts whose declaration is
    /// an array. Indexed elements and scalars return `None`.
    fn whole_array_dimensions(&self, reference: &Reference) -> Option<&[i64]> {
        let Reference::State(parts) = reference else {
            return None;
        };
        let [part] = parts.as_slice() else {
            return None;
        };
        if part.subscripts.is_empty() {
            self.names.array_dimensions(&part.name)
        } else {
            None
        }
    }

    /// Whether `reference` is a whole-array state reference.
    fn is_whole_array_reference(&self, reference: &Reference) -> bool {
        self.whole_array_dimensions(reference).is_some()
    }
}

fn single_part_name(reference: &Reference) -> Option<&Name> {
    let Reference::State(parts) = reference else {
        return None;
    };
    let [part] = parts.as_slice() else {
        return None;
    };
    Some(&part.name)
}

fn element_count(dimensions: &[i64]) -> Result<i64, GalecTargetError> {
    dimensions.iter().try_fold(1_i64, |count, dimension| {
        count
            .checked_mul(*dimension)
            .ok_or_else(|| GalecTargetError::LoweringInternal {
                detail: "C export array element count overflowed i64".to_owned(),
            })
    })
}

fn is_quaternion_identity_literal(expression: &Expression) -> bool {
    let Expression::Array(elements) = unparen(expression) else {
        return false;
    };
    let expected = [1.0_f64, 0.0, 0.0, 0.0];
    elements.len() == expected.len()
        && elements
            .iter()
            .zip(expected)
            .all(|(element, expected)| {
                matches!(unparen(element), Expression::Real(value) if value.to_bits() == expected.to_bits())
            })
}

fn component_assignment_group<'a>(
    statements: &'a [Spanned<Statement>],
    count: usize,
) -> Option<(Reference, Vec<&'a Expression>)> {
    let selected = statements.get(..count)?;
    let mut target = None;
    let mut values = Vec::with_capacity(count);
    for (offset, statement) in selected.iter().enumerate() {
        let Statement::Assignment {
            target: indexed_target,
            value,
        } = &statement.node
        else {
            return None;
        };
        let (whole_target, index) =
            indexed_vector_reference(&Expression::Ref(indexed_target.clone()))?;
        if index != i64::try_from(offset + 1).ok()? {
            return None;
        }
        match &target {
            Some(existing) if existing != &whole_target => return None,
            None => target = Some(whole_target),
            _ => {}
        }
        values.push(value);
    }
    Some((target?, values))
}

fn first_indexed_real_vector(
    expression: &Expression,
    names: &CNameTable,
    length: i64,
) -> Option<Reference> {
    match unparen(expression) {
        Expression::Ref(_) => {
            let (reference, _) = indexed_vector_reference(unparen(expression))?;
            let name = single_part_name(&reference)?;
            (names.scalar_type(name) == Some(rumoca_ir_galec::ast::ScalarType::Real)
                && names.array_dimensions(name) == Some([length].as_slice()))
            .then_some(reference)
        }
        Expression::Neg(reference) => {
            let (reference, _) = indexed_vector_reference(&Expression::Ref(reference.clone()))?;
            let name = single_part_name(&reference)?;
            (names.scalar_type(name) == Some(rumoca_ir_galec::ast::ScalarType::Real)
                && names.array_dimensions(name) == Some([length].as_slice()))
            .then_some(reference)
        }
        Expression::Binary { lhs, rhs, .. } => first_indexed_real_vector(lhs, names, length)
            .or_else(|| first_indexed_real_vector(rhs, names, length)),
        Expression::Paren(inner) | Expression::Not(inner) => {
            first_indexed_real_vector(inner, names, length)
        }
        Expression::If(if_expression) => if_expression
            .branches
            .iter()
            .find_map(|(condition, value)| {
                first_indexed_real_vector(condition, names, length)
                    .or_else(|| first_indexed_real_vector(value, names, length))
            })
            .or_else(|| first_indexed_real_vector(&if_expression.else_value, names, length)),
        Expression::Call(call) => call
            .arguments
            .iter()
            .find_map(|argument| first_indexed_real_vector(argument, names, length)),
        Expression::Array(elements) => elements
            .iter()
            .find_map(|element| first_indexed_real_vector(element, names, length)),
        Expression::Size { dimension, .. } => first_indexed_real_vector(dimension, names, length),
        Expression::Bool(_) | Expression::Integer(_) | Expression::Real(_) => None,
    }
}

fn element(reference: &Reference, index: i64) -> Expression {
    let Reference::State(parts) = reference else {
        unreachable!("helper pattern references are state references");
    };
    let mut part = parts[0].clone();
    part.subscripts.push(Expression::Integer(index));
    Expression::Ref(Reference::State(vec![part]))
}

fn add(lhs: Expression, rhs: Expression) -> Expression {
    Expression::binary(BinaryOp::Add, lhs, rhs)
}

fn sub(lhs: Expression, rhs: Expression) -> Expression {
    Expression::binary(BinaryOp::Sub, lhs, rhs)
}

fn mul(lhs: Expression, rhs: Expression) -> Expression {
    Expression::binary(BinaryOp::Mul, lhs, rhs)
}

fn expressions_match(actual: &Expression, expected: &Expression) -> bool {
    match (unparen(actual), unparen(expected)) {
        (
            Expression::Binary {
                op: actual_op,
                lhs: actual_lhs,
                rhs: actual_rhs,
            },
            Expression::Binary {
                op: expected_op,
                lhs: expected_lhs,
                rhs: expected_rhs,
            },
        ) => {
            actual_op == expected_op
                && expressions_match(actual_lhs, expected_lhs)
                && expressions_match(actual_rhs, expected_rhs)
        }
        (Expression::Real(actual), Expression::Real(expected)) => {
            actual.to_bits() == expected.to_bits()
        }
        (actual, expected) => actual == expected,
    }
}

/// Collect an addition chain only when it has the exact left-fold shape that
/// the generated helper loop evaluates. Accepting a right-nested addition
/// would silently reassociate floating-point operations.
fn collect_left_associated_add_terms<'a>(
    expression: &'a Expression,
    terms: &mut Vec<&'a Expression>,
) -> bool {
    match unparen(expression) {
        Expression::Binary {
            op: BinaryOp::Add,
            lhs,
            rhs,
        } => {
            if matches!(
                unparen(rhs),
                Expression::Binary {
                    op: BinaryOp::Add,
                    ..
                }
            ) || !collect_left_associated_add_terms(lhs, terms)
            {
                return false;
            }
            terms.push(unparen(rhs));
            true
        }
        term => {
            terms.push(term);
            true
        }
    }
}

fn dot_term(expression: &Expression) -> Option<(Reference, Reference, i64)> {
    let Expression::Binary {
        op: BinaryOp::Mul,
        lhs,
        rhs,
    } = unparen(expression)
    else {
        return None;
    };
    let (lhs, lhs_index) = indexed_vector_reference(unparen(lhs))?;
    let (rhs, rhs_index) = indexed_vector_reference(unparen(rhs))?;
    (lhs_index == rhs_index).then_some((lhs, rhs, lhs_index))
}

fn indexed_vector_reference(expression: &Expression) -> Option<(Reference, i64)> {
    let Expression::Ref(Reference::State(parts)) = expression else {
        return None;
    };
    let [part] = parts.as_slice() else {
        return None;
    };
    let [Expression::Integer(index)] = part.subscripts.as_slice() else {
        return None;
    };
    let mut whole = part.clone();
    whole.subscripts.clear();
    Some((Reference::State(vec![whole]), *index))
}

fn unparen(expression: &Expression) -> &Expression {
    if let Expression::Paren(inner) = expression {
        unparen(inner)
    } else {
        expression
    }
}

/// GALEC Integer is C `int32_t`: literals outside its range are rejected,
/// never truncated (SPEC_0008).
fn validate_integer_literal(value: i64) -> Result<(), GalecTargetError> {
    if i32::try_from(value).is_err() {
        return Err(GalecTargetError::CExportUnsupported {
            construct: "an Integer literal beyond int32_t",
            detail: format!("literal {value} does not fit the C Integer type int32_t"),
        });
    }
    Ok(())
}

pub(crate) fn binary_op_name(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Pow => "pow",
        BinaryOp::Mul => "mul",
        BinaryOp::Div => "div",
        BinaryOp::Add => "add",
        BinaryOp::Sub => "sub",
        BinaryOp::Lt => "lt",
        BinaryOp::Gt => "gt",
        BinaryOp::Le => "le",
        BinaryOp::Ge => "ge",
        BinaryOp::Eq => "eq",
        BinaryOp::Ne => "ne",
        BinaryOp::And => "and",
        BinaryOp::Or => "or",
    }
}

fn unsupported_statement(construct: &'static str) -> GalecTargetError {
    GalecTargetError::CExportUnsupported {
        construct,
        detail: "the current C context supports assignments and Boolean if statements only; \
                 this statement kind cannot come from the supported DAE lowering"
            .to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use rumoca_core::Span;
    use rumoca_ir_galec::ast::{
        BinaryOp, Block, Condition, Dimension, Expression, IfBranch, IfExpression, IfStatement,
        Name, ProtectedEntity, ProtectedKind, RangeAttributes, RefPart, Reference, ScalarType,
        Spanned, Statement, TypeRef, VariableDeclaration,
    };

    use super::{CContextLowerer, element, mul, sub};
    use crate::c_mangle::CNameTable;

    fn real_array(name: &str, dimensions: &[i64]) -> ProtectedEntity {
        ProtectedEntity {
            kind: ProtectedKind::State,
            decl: VariableDeclaration {
                ty: TypeRef::Primitive(ScalarType::Real),
                name: Name::ident(name),
                dimensions: dimensions
                    .iter()
                    .copied()
                    .map(|size| Dimension::Expr(Expression::Integer(size)))
                    .collect(),
                range: RangeAttributes::default(),
                span: Span::DUMMY,
            },
            start: None,
        }
    }

    fn state_ref(name: &str, index: Option<i64>) -> Reference {
        Reference::State(vec![RefPart {
            name: Name::ident(name),
            subscripts: index.into_iter().map(Expression::Integer).collect(),
            span: Span::DUMMY,
        }])
    }

    #[test]
    fn array_literal_expansion_preserves_existing_target_subscripts() {
        let name = Name::ident("m");
        let mut block = Block::new(Name::ident("SliceAssignment"));
        block.protected.push(ProtectedEntity {
            kind: ProtectedKind::State,
            decl: VariableDeclaration {
                ty: TypeRef::Primitive(ScalarType::Real),
                name: name.clone(),
                dimensions: vec![
                    Dimension::Expr(Expression::Integer(2)),
                    Dimension::Expr(Expression::Integer(2)),
                ],
                range: RangeAttributes::default(),
                span: Span::DUMMY,
            },
            start: None,
        });
        let names = CNameTable::build(&block).expect("build C names");
        let statement = Statement::Assignment {
            target: Reference::State(vec![RefPart {
                name,
                subscripts: vec![Expression::Integer(1)],
                span: Span::DUMMY,
            }]),
            value: Expression::Array(vec![Expression::Real(1.0), Expression::Real(2.0)]),
        };

        let assignments = CContextLowerer::new(&names)
            .statement_contexts(&statement)
            .expect("lower slice assignment");

        assert_eq!(assignments.len(), 2);
        assert_eq!(assignments[0]["target"]["indices"][0]["value"], 0);
        assert_eq!(assignments[0]["target"]["indices"][1]["value"], 0);
        assert_eq!(assignments[1]["target"]["indices"][0]["value"], 0);
        assert_eq!(assignments[1]["target"]["indices"][1]["value"], 1);
    }

    #[test]
    fn statement_if_and_expression_if_remain_distinct_context_nodes() {
        let name = Name::ident("x");
        let mut block = Block::new(Name::ident("StructuredIf"));
        block.protected.push(ProtectedEntity {
            kind: ProtectedKind::State,
            decl: VariableDeclaration {
                ty: TypeRef::Primitive(ScalarType::Real),
                name: name.clone(),
                dimensions: Vec::new(),
                range: RangeAttributes::default(),
                span: Span::DUMMY,
            },
            start: None,
        });
        let target = Reference::State(vec![RefPart {
            name,
            subscripts: Vec::new(),
            span: Span::DUMMY,
        }]);
        let statement = Statement::If(IfStatement {
            branches: vec![IfBranch {
                condition: Condition::Expression(Expression::Bool(true)),
                body: vec![Spanned::dummy(Statement::Assignment {
                    target: target.clone(),
                    value: Expression::If(IfExpression {
                        branches: vec![(Expression::Bool(false), Expression::Real(1.0))],
                        else_value: Box::new(Expression::Real(2.0)),
                    }),
                })],
                span: Span::DUMMY,
            }],
            else_body: Some(vec![Spanned::dummy(Statement::Assignment {
                target,
                value: Expression::Real(3.0),
            })]),
        });

        let names = CNameTable::build(&block).expect("build C names");
        let contexts = CContextLowerer::new(&names)
            .statement_contexts(&statement)
            .expect("lower structured if");

        assert_eq!(contexts.len(), 1);
        assert_eq!(contexts[0]["kind"], "if");
        assert_eq!(contexts[0]["branches"][0]["body"][0]["kind"], "assign");
        assert_eq!(contexts[0]["branches"][0]["body"][0]["value"]["kind"], "if");
        assert_eq!(contexts[0]["else_body"][0]["kind"], "assign");
    }

    #[test]
    fn whole_array_arithmetic_becomes_one_typed_helper_call() {
        let mut block = Block::new(Name::ident("ArrayHelper"));
        block.protected = vec![
            real_array("input", &[3]),
            real_array("bias", &[3]),
            real_array("correction", &[3]),
            real_array("output", &[3]),
            ProtectedEntity {
                kind: ProtectedKind::State,
                decl: VariableDeclaration::scalar(ScalarType::Real, Name::ident("scale")),
                start: None,
            },
        ];
        let names = CNameTable::build(&block).expect("build C names");
        let statement = Statement::Assignment {
            target: state_ref("output", None),
            value: Expression::Binary {
                op: BinaryOp::Div,
                lhs: Box::new(Expression::Ref(state_ref("input", None))),
                rhs: Box::new(Expression::Ref(state_ref("scale", None))),
            },
        };

        let contexts = CContextLowerer::new(&names)
            .statement_contexts(&statement)
            .expect("lower array helper");

        assert_eq!(contexts.len(), 1);
        assert_eq!(contexts[0]["kind"], "array_binary");
        assert_eq!(contexts[0]["operator"], "div");
        assert_eq!(contexts[0]["lhs"]["kind"], "array");
        assert_eq!(contexts[0]["rhs"]["kind"], "scalar");
        assert_eq!(contexts[0]["element_count"], 3);

        let affine = Statement::Assignment {
            target: state_ref("output", None),
            value: Expression::Binary {
                op: BinaryOp::Add,
                lhs: Box::new(Expression::Binary {
                    op: BinaryOp::Add,
                    lhs: Box::new(Expression::Ref(state_ref("input", None))),
                    rhs: Box::new(Expression::Ref(state_ref("bias", None))),
                }),
                rhs: Box::new(Expression::Binary {
                    op: BinaryOp::Mul,
                    lhs: Box::new(Expression::Ref(state_ref("scale", None))),
                    rhs: Box::new(Expression::Ref(state_ref("correction", None))),
                }),
            },
        };
        let contexts = CContextLowerer::new(&names)
            .statement_contexts(&affine)
            .expect("lower affine array helper");
        assert_eq!(contexts.len(), 1);
        assert_eq!(contexts[0]["kind"], "array_add_add_scaled");
    }

    #[test]
    fn complete_expanded_sum_of_products_becomes_dot_context() {
        let mut block = Block::new(Name::ident("DotHelper"));
        block.protected = vec![
            real_array("a", &[3]),
            real_array("b", &[3]),
            ProtectedEntity {
                kind: ProtectedKind::State,
                decl: VariableDeclaration::scalar(ScalarType::Real, Name::ident("result")),
                start: None,
            },
        ];
        let product = |index| Expression::Binary {
            op: BinaryOp::Mul,
            lhs: Box::new(Expression::Ref(state_ref("a", Some(index)))),
            rhs: Box::new(Expression::Ref(state_ref("b", Some(index)))),
        };
        let value = Expression::Binary {
            op: BinaryOp::Add,
            lhs: Box::new(Expression::Binary {
                op: BinaryOp::Add,
                lhs: Box::new(product(1)),
                rhs: Box::new(product(2)),
            }),
            rhs: Box::new(product(3)),
        };
        let names = CNameTable::build(&block).expect("build C names");
        let contexts = CContextLowerer::new(&names)
            .statement_contexts(&Statement::Assignment {
                target: state_ref("result", None),
                value,
            })
            .expect("lower dot helper");

        assert_eq!(contexts[0]["value"]["kind"], "dot");
        assert_eq!(contexts[0]["value"]["lhs"]["name"], "a");
        assert_eq!(contexts[0]["value"]["rhs"]["name"], "b");
        assert_eq!(contexts[0]["value"]["element_count"], 3);

        let right_associated = Expression::Binary {
            op: BinaryOp::Add,
            lhs: Box::new(product(1)),
            rhs: Box::new(Expression::Binary {
                op: BinaryOp::Add,
                lhs: Box::new(product(2)),
                rhs: Box::new(product(3)),
            }),
        };
        let contexts = CContextLowerer::new(&names)
            .statement_contexts(&Statement::Assignment {
                target: state_ref("result", None),
                value: right_associated,
            })
            .expect("preserve right-associated sum");

        assert_eq!(contexts[0]["value"]["kind"], "binary");
    }

    #[test]
    fn complete_cross_product_component_group_becomes_one_helper_context() {
        let mut block = Block::new(Name::ident("CrossHelper"));
        block.protected = vec![
            real_array("a", &[3]),
            real_array("b", &[3]),
            real_array("result", &[3]),
        ];
        let a = state_ref("a", None);
        let b = state_ref("b", None);
        let values = [
            sub(
                mul(element(&a, 2), element(&b, 3)),
                mul(element(&a, 3), element(&b, 2)),
            ),
            sub(
                mul(element(&a, 3), element(&b, 1)),
                mul(element(&a, 1), element(&b, 3)),
            ),
            sub(
                mul(element(&a, 1), element(&b, 2)),
                mul(element(&a, 2), element(&b, 1)),
            ),
        ];
        let statements = values
            .into_iter()
            .enumerate()
            .map(|(index, value)| {
                Spanned::dummy(Statement::Assignment {
                    target: state_ref("result", Some(i64::try_from(index + 1).unwrap())),
                    value,
                })
            })
            .collect::<Vec<_>>();
        let names = CNameTable::build(&block).expect("build C names");
        let lowerer = CContextLowerer::new(&names);

        let contexts = lowerer
            .statements_contexts(&statements)
            .expect("lower complete cross product");
        assert_eq!(contexts.len(), 1);
        assert_eq!(contexts[0]["kind"], "cross3");
        assert_eq!(contexts[0]["lhs"]["name"], "a");
        assert_eq!(contexts[0]["rhs"]["name"], "b");

        let incomplete = lowerer
            .statements_contexts(&statements[..2])
            .expect("preserve incomplete cross product");
        assert_eq!(incomplete.len(), 2);
        assert!(incomplete.iter().all(|context| context["kind"] == "assign"));
    }
}
