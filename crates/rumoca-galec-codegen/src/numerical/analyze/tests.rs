use super::*;
use crate::numerical::facts::ValueSource;
use rumoca_ir_galec::ast::{
    Block, FunctionCall, InterfaceVariable, Name, ProtectedEntity, RangeAttributes, Spanned,
    StateCompartment,
};

#[test]
fn registers_all_entity_roles_in_declaration_order() {
    let mut block = Block::new(Name::ident("Roles"));
    block.interface = vec![
        interface(InterfaceKind::Input, "input"),
        interface(InterfaceKind::Output, "output"),
        interface(InterfaceKind::TunableParameter, "tunable"),
    ];
    block.protected = vec![
        protected(ProtectedKind::DependentParameter, "dependent"),
        protected(ProtectedKind::Constant, "constant"),
        protected(ProtectedKind::State, "state"),
    ];

    let facts = analyze(&block).expect("valid declarations should be analyzed");
    let roles: Vec<_> = facts
        .entities()
        .iter()
        .map(|entity| entity.role())
        .collect();

    assert_eq!(
        roles,
        vec![
            EntityRole::Input,
            EntityRole::Output,
            EntityRole::TunableParameter,
            EntityRole::DependentParameter,
            EntityRole::Constant,
            EntityRole::State,
        ]
    );
}

#[test]
fn registers_scalar_vector_and_matrix_shapes() {
    let mut block = Block::new(Name::ident("Shapes"));
    block.interface = vec![interface(InterfaceKind::Input, "scalar")];
    block.protected = vec![
        protected_with_dimensions(ProtectedKind::State, "vector", &[4]),
        protected_with_dimensions(ProtectedKind::State, "matrix", &[3, 3]),
    ];

    let facts = analyze(&block).expect("rank-two-and-lower shapes should be analyzed");
    let entities = facts.entities();

    assert_eq!(entities[0].name(), "scalar");
    assert!(entities[0].shape().dimensions().is_empty());
    assert_eq!(entities[1].shape().dimensions(), &[4]);
    assert_eq!(entities[2].shape().dimensions(), &[3, 3]);
}

#[test]
fn registers_ekf_innovation_covariance_profile() {
    let mut block = Block::new(Name::ident("EkfInnovation"));
    block.interface = vec![
        interface_with_dimensions(InterfaceKind::TunableParameter, "H", &[2, 4]),
        interface_with_dimensions(InterfaceKind::TunableParameter, "R", &[2, 2]),
    ];
    block.protected = vec![
        protected_with_dimensions(ProtectedKind::State, "P", &[4, 4]),
        protected_with_dimensions(ProtectedKind::State, "S", &[2, 2]),
    ];

    let facts = analyze(&block).expect("EKF matrix declarations should be analyzed");
    let profile: Vec<_> = facts
        .entities()
        .iter()
        .map(|entity| {
            (
                entity.name(),
                entity.scalar_kind(),
                entity.shape().dimensions(),
                entity.role(),
            )
        })
        .collect();

    assert_eq!(
        profile,
        vec![
            (
                "H",
                ScalarType::Real,
                &[2, 4][..],
                EntityRole::TunableParameter,
            ),
            (
                "R",
                ScalarType::Real,
                &[2, 2][..],
                EntityRole::TunableParameter,
            ),
            ("P", ScalarType::Real, &[4, 4][..], EntityRole::State),
            ("S", ScalarType::Real, &[2, 2][..], EntityRole::State),
        ]
    );
}

#[test]
fn records_ekf_linear_solve_then_assignment_in_do_step() {
    let mut block = Block::new(Name::ident("EkfSolve"));
    block.interface = vec![
        interface_with_dimensions(InterfaceKind::Input, "residual", &[2]),
        interface_with_dimensions(InterfaceKind::TunableParameter, "S", &[2, 2]),
    ];
    block.protected = vec![protected_with_dimensions(
        ProtectedKind::State,
        "correction",
        &[2],
    )];
    block
        .do_step
        .statements
        .push(Spanned::dummy(Statement::Assignment {
            target: Reference::state(Name::ident("correction")),
            value: Expression::Call(FunctionCall {
                function: Name::ident("solveLinearEquations"),
                arguments: vec![state_expression("S"), state_expression("residual")],
            }),
        }));

    let facts = analyze(&block).expect("valid EKF solve should be analyzed");
    let operations = facts.operations();

    assert_eq!(operations.len(), 2);
    assert_eq!(operations[0].kind(), OperationKind::LinearSolve);
    assert_eq!(operations[1].kind(), OperationKind::Assign);
    assert_eq!(operations[0].phase(), BlockMethodKind::DoStep);
    assert_eq!(operations[1].phase(), BlockMethodKind::DoStep);
    assert_eq!(operations[0].inputs().len(), 2);
    assert_eq!(operations[1].inputs(), operations[0].outputs());

    let correction = facts
        .entities()
        .iter()
        .find(|entity| entity.name() == "correction")
        .expect("correction entity")
        .id();
    let assigned = facts.value(operations[1].outputs()[0]);
    assert_eq!(assigned.stored_in(), Some(correction));
    assert!(matches!(assigned.source(), ValueSource::Operation(_)));
}

#[test]
fn rank_two_multiply_remains_elementwise() {
    let mut block = Block::new(Name::ident("Elementwise"));
    block.protected = vec![
        protected_with_dimensions(ProtectedKind::State, "A", &[2, 2]),
        protected_with_dimensions(ProtectedKind::State, "B", &[2, 2]),
        protected_with_dimensions(ProtectedKind::State, "C", &[2, 2]),
    ];
    block
        .do_step
        .statements
        .push(Spanned::dummy(Statement::Assignment {
            target: Reference::state(Name::ident("C")),
            value: Expression::binary(BinaryOp::Mul, state_expression("A"), state_expression("B")),
        }));

    let facts = analyze(&block).expect("rank-two element-wise multiply should be analyzed");
    let kinds: Vec<_> = facts
        .operations()
        .iter()
        .map(|operation| operation.kind())
        .collect();

    assert_eq!(
        kinds,
        vec![OperationKind::ElementwiseMultiply, OperationKind::Assign]
    );
}

#[test]
fn rejects_rank_greater_than_two() {
    let mut block = Block::new(Name::ident("RankThree"));
    block.protected = vec![protected_with_dimensions(
        ProtectedKind::State,
        "tensor",
        &[2, 3, 4],
    )];

    let error = analyze(&block).expect_err("rank three must be rejected");

    assert_eq!(error.code(), "ET026");
    assert!(matches!(
        error,
        NumericalAnalysisError::UnsupportedRank { rank: 3, .. }
    ));
}

#[test]
fn rejects_component_typed_entities() {
    let mut block = Block::new(Name::ident("Component"));
    let mut entity = protected(ProtectedKind::State, "component");
    entity.decl.ty = TypeRef::Compartment(Name::ident("StateRecord"));
    block.protected.push(entity);

    let error = analyze(&block).expect_err("component types must be rejected");

    assert_eq!(error.code(), "ET024");
}

#[test]
fn rejects_invalid_dimensions() {
    let mut block = Block::new(Name::ident("InvalidDimension"));
    let mut entity = protected(ProtectedKind::State, "empty");
    entity.decl.dimensions = vec![Dimension::Expr(Expression::Integer(0))];
    block.protected.push(entity);

    let error = analyze(&block).expect_err("zero dimensions must be rejected");

    assert_eq!(error.code(), "ET025");
}

#[test]
fn rejects_state_compartments() {
    let mut block = Block::new(Name::ident("Compartments"));
    block.compartments.push(StateCompartment {
        name: Name::ident("StateRecord"),
        entities: Vec::new(),
        span: Span::DUMMY,
    });

    let error = analyze(&block).expect_err("state compartments must be rejected");

    assert_eq!(error.code(), "ET027");
}

fn interface(kind: InterfaceKind, name: &str) -> InterfaceVariable {
    interface_with_dimensions(kind, name, &[])
}

fn interface_with_dimensions(
    kind: InterfaceKind,
    name: &str,
    dimensions: &[i64],
) -> InterfaceVariable {
    InterfaceVariable {
        kind,
        decl: declaration(name, dimensions),
        start: None,
    }
}

fn protected(kind: ProtectedKind, name: &str) -> ProtectedEntity {
    protected_with_dimensions(kind, name, &[])
}

fn protected_with_dimensions(
    kind: ProtectedKind,
    name: &str,
    dimensions: &[i64],
) -> ProtectedEntity {
    ProtectedEntity {
        kind,
        decl: declaration(name, dimensions),
        start: None,
    }
}

fn declaration(name: &str, dimensions: &[i64]) -> VariableDeclaration {
    VariableDeclaration {
        ty: TypeRef::Primitive(ScalarType::Real),
        name: Name::ident(name),
        dimensions: dimensions
            .iter()
            .map(|size| Dimension::Expr(Expression::Integer(*size)))
            .collect(),
        range: RangeAttributes::default(),
        span: Span::DUMMY,
    }
}

fn state_expression(name: &str) -> Expression {
    Expression::Ref(Reference::state(Name::ident(name)))
}
