// transformation.rs, does the conversion to GALEC after the analysis pass

use crate::analysis::AnalysisResult;
use rumoca_ir_dae::{Dae, VariableOrigin};
use crate::galec::{DataRole, Galec, GalecBinaryOp, GalecExpr, GalecGroup, GalecLiteral, GalecStatement, GalecVariable};
use rumoca_core::Expression;
use rumoca_core::{BuiltinFunction, OpBinary, OpUnary, Literal};
use std::collections::HashSet;

pub struct TransformationInput<'a> {
    pub dae: &'a Dae,
    pub analysis: AnalysisResult,
    pub model_name: String,
}

pub fn transform(input: TransformationInput) -> Galec {
    let variables = transform_variables(input.dae);
    let startup = build_startup(input.dae);
    let recalibrate = build_recalibrate(input.dae);
    let do_step = build_do_step(&input.analysis, input.dae);
    let period = input.dae.clocks.schedules[0].period_seconds;

    Galec {
        name: input.model_name,
        period,
        variables,
        startup,
        recalibrate,
        do_step,
    }
}

macro_rules! push_partition {
    ($vars:expr, $partition:expr, $role:expr, $ty:expr) => {
        for (_, v) in &$partition {
            if v.origin == VariableOrigin::Generated { continue; }
            $vars.push(GalecVariable {
                name: v.name.to_string(),
                role: $role,
                ty: $ty,
                dims: v.dims.iter().map(|&d| d as usize).collect(),
                start: v.start.as_ref().map(transform_expr)
            });
        }
    };
}

fn transform_variables(dae: &Dae) -> Vec<GalecVariable> {
    let mut vars: Vec<GalecVariable> = Vec::new();

    push_partition!(vars, dae.variables.inputs, DataRole::Input, GalecLiteral::Real(0.0));
    push_partition!(vars, dae.variables.outputs, DataRole::Output, GalecLiteral::Real(0.0));
    push_partition!(vars, dae.variables.constants, DataRole::Constant, GalecLiteral::Real(0.0));
    push_partition!(vars, dae.variables.discrete_reals, DataRole::State, GalecLiteral::Real(0.0));
    push_partition!(vars, dae.variables.discrete_valued, DataRole::State, GalecLiteral::Integer(0));

    for (_, v) in &dae.variables.parameters {
        if v.origin == VariableOrigin::Generated { continue; }
        let role = if v.is_tunable { DataRole::TunableParameter } else { DataRole::DependentParameter };
        vars.push(GalecVariable {
            name: v.name.to_string(),
            role,
            ty: GalecLiteral::Real(0.0),
            dims: v.dims.iter().map(|&d| d as usize).collect(),
            start: v.start.as_ref().map(transform_expr),
        });
    }

    vars
}

fn build_startup(dae: &Dae) -> Vec<GalecStatement> {
    let mut stmts: Vec<GalecStatement> = Vec::new();
    for (_, v) in &dae.variables.constants {
        if v.origin == VariableOrigin::Generated { continue; }
        if let Some(start) = &v.start {
            stmts.push(GalecStatement::Assign {
                lhs: v.name.to_string(),
                rhs: transform_expr(start),
            });
        }
    }
    for (_, v) in &dae.variables.parameters {
        if v.origin == VariableOrigin::Generated { continue; }
        if let Some(start) = &v.start {
            stmts.push(GalecStatement::Assign {
                lhs: v.name.to_string(),
                rhs: transform_expr(start),
            });
        }
    }
    for partition in [&dae.variables.discrete_reals, &dae.variables.discrete_valued, &dae.variables.outputs] {
        for (_, v) in partition {
            if v.origin == VariableOrigin::Generated { continue; }
            if let Some(start) = &v.start {
                stmts.push(GalecStatement::Assign {
                    lhs: v.name.to_string(),
                    rhs: transform_expr(start),
                });
            }
        }
    }
    stmts
}

fn build_recalibrate(dae: &Dae) -> Vec<GalecStatement> {
    let d_param: HashSet<String> = dae.variables.parameters.values()
        .filter(|v| !v.is_tunable)
        .map(|v| v.name.to_string())
        .collect();

    dae.discrete.real_updates.iter()
        .chain(dae.discrete.valued_updates.iter())
        .filter(|eq| eq.lhs.as_ref().map(|l| d_param.contains(&l.to_string())).unwrap_or(false))
        .map(|eq| GalecStatement::Assign {
            lhs: eq.lhs.as_ref().unwrap().to_string(),
            rhs: transform_expr(&eq.rhs),
        })
        .collect()
}

fn build_do_step(analysis: &AnalysisResult, dae: &Dae) -> Vec<GalecGroup> {
    let source_vars: std::collections::HashSet<String> = dae.variables.discrete_reals.values()
        .chain(dae.variables.discrete_valued.values())
        .chain(dae.variables.outputs.values())
        .filter(|v| v.origin == VariableOrigin::Source)
        .map(|v| v.name.to_string())
        .collect();
    let equations: Vec<_> = dae.discrete.real_updates.iter()
        .chain(dae.discrete.valued_updates.iter())
        .filter(|eq| eq.lhs.as_ref().map(|lhs| source_vars.contains(&lhs.to_string())).unwrap_or(false))
        .collect();

    analysis.groups.iter().map(|group| {
        let statements = group.statements.iter().map(|id| {
            let eq = equations[id.0];
            let rhs = extract_true_branch(&eq.rhs);
            GalecStatement::Assign {
                lhs: eq.lhs.as_ref().unwrap().to_string(),
                rhs: transform_expr(rhs),
            }
        }).collect();

        GalecGroup {
            condition: group.condition.as_ref().map(transform_expr),
            statements,
        }
    }).collect()
}

fn extract_true_branch(rhs: &Expression) -> &Expression {
    if let Expression::If { branches, .. } = rhs {
        if let Some((_, true_val)) = branches.first() {
            return true_val;
        }
    }
    rhs
}

fn transform_expr(expr: &Expression) -> GalecExpr {
    match expr {
        Expression::VarRef { name, .. } => {
            let s = name.to_string();
            if let Some(base) = s.strip_prefix("__pre__.") {
                GalecExpr::PreviousRef { name: base.to_string() }
            } else {
                GalecExpr::SelfRef { name: s }
            }
        }

        Expression::BuiltinCall { function: BuiltinFunction::Pre, args, .. } => {
            match args.first() {
                Some(Expression::VarRef { name, .. }) => {
                    GalecExpr::PreviousRef { name: name.to_string() }
                }
                Some(_) => unimplemented!("pre() arg must be a variable reference"),
                None => unimplemented!("pre() requires exactly one argument"),
            }
        }
            

        Expression::BuiltinCall { function, args, .. } =>
            GalecExpr::Call {
                name: remap_builtin(function).to_string(),
                args: args.iter().map(transform_expr).collect(),
            },

        Expression::FunctionCall { name, args, .. } =>
            GalecExpr::Call {
                name: name.to_string(),
                args: args.iter().map(transform_expr).collect(),
            },

        Expression::Binary { op, lhs, rhs, .. } =>
            GalecExpr::Binary {
                op: remap_op(op),
                lhs: Box::new(transform_expr(lhs)),
                rhs: Box::new(transform_expr(rhs)),
            },

        Expression::Unary { op: OpUnary::Minus, rhs, .. } =>
            GalecExpr::Negate(Box::new(transform_expr(rhs))),

        Expression::Literal { value: Literal::Real(v), .. } =>
            GalecExpr::Literal(GalecLiteral::Real(*v)),
        Expression::Literal { value: Literal::Integer(v), .. } =>
            GalecExpr::Literal(GalecLiteral::Integer(*v)),
        Expression::Literal { value: Literal::Boolean(v), .. } =>
            GalecExpr::Literal(GalecLiteral::Boolean(*v)),

        Expression::If { branches, else_branch, .. } => {
            let mut result = transform_expr(else_branch);
            for (cond, then_val) in branches.iter().rev() {
                result = GalecExpr::If {
                    condition: Box::new(transform_expr(cond)),
                    then_branch: Box::new(transform_expr(then_val)),
                    else_branch: Box::new(result),
                };
            }
            result
        },

        Expression::Index { base, subscripts, .. } =>
            GalecExpr::Index {
                base: Box::new(transform_expr(base)),
                subscripts: subscripts.iter().filter_map(|s| {
                    if let rumoca_core::Subscript::Expr { expr, .. } = s {
                        Some(transform_expr(expr))
                    } else { None }
                }).collect(),
            },

        _ => unimplemented!("expression not supported in GALEC"),
    }
}

fn remap_builtin(f: &BuiltinFunction) -> &'static str {
    match f {
        BuiltinFunction::Abs  => "absolute",
        BuiltinFunction::Log  => "ln",
        BuiltinFunction::Log10 => "lg",
        BuiltinFunction::Floor => "roundDown",
        BuiltinFunction::Ceil  => "roundUp",
        BuiltinFunction::Min  => "min",
        BuiltinFunction::Max  => "max",
        BuiltinFunction::Sign => "sign",
        BuiltinFunction::Sqrt => "sqrt",
        BuiltinFunction::Sin  => "sin",
        BuiltinFunction::Cos  => "cos",
        BuiltinFunction::Tan  => "tan",
        BuiltinFunction::Asin => "asin",
        BuiltinFunction::Acos => "acos",
        BuiltinFunction::Atan => "atan",
        BuiltinFunction::Atan2 => "atan2",
        BuiltinFunction::Exp  => "exp",
        _ => unimplemented!("builtin not supported in GALEC"),
    }
}

fn remap_op(op: &OpBinary) -> GalecBinaryOp {
    match op {
        OpBinary::Add | OpBinary::AddElem => GalecBinaryOp::Add,
        OpBinary::Sub | OpBinary::SubElem => GalecBinaryOp::Sub,
        OpBinary::Mul | OpBinary::MulElem => GalecBinaryOp::Mul,
        OpBinary::Div | OpBinary::DivElem => GalecBinaryOp::Div,
        OpBinary::Exp | OpBinary::ExpElem => GalecBinaryOp::Pow,
        OpBinary::Eq  => GalecBinaryOp::Eq,
        OpBinary::Neq => GalecBinaryOp::Neq,
        OpBinary::Lt  => GalecBinaryOp::Lt,
        OpBinary::Le  => GalecBinaryOp::Lte,
        OpBinary::Gt  => GalecBinaryOp::Gt,
        OpBinary::Ge  => GalecBinaryOp::Gte,
        OpBinary::And => GalecBinaryOp::And,
        OpBinary::Or  => GalecBinaryOp::Or,
        _ => unimplemented!("operator not supported in GALEC"),
    }
}
