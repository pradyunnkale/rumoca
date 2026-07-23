#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct EntityId(usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct OperationId(usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct ValueId(usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EntityRole {
    Input,
    Output,
    TunableParameter,
    DependentParameter,
    Constant,
    State,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OperationKind {
    Assign,
    Add,
    Subtract,
    ElementwiseMultiply,
    ElementwiseDivide,
    LinearSolve,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum LiteralValue {
    Boolean(bool),
    Integer(i64),
    Real(f64),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum ValueSource {
    EntityRead(EntityId),
    Literal(LiteralValue),
    Operation(OperationId),
}

#[derive(Debug, PartialEq, Eq)]
pub struct Entity {
    id: EntityId,
    name: String,
    scalar_kind: rumoca_ir_galec::ast::ScalarType,
    shape: Shape,
    role: EntityRole,
}

impl Entity {
    pub(super) fn id(&self) -> EntityId {
        self.id
    }

    pub(super) fn name(&self) -> &str {
        &self.name
    }

    pub(super) fn scalar_kind(&self) -> rumoca_ir_galec::ast::ScalarType {
        self.scalar_kind
    }

    pub(super) fn shape(&self) -> &Shape {
        &self.shape
    }

    pub(super) fn role(&self) -> EntityRole {
        self.role
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Value {
    id: ValueId,
    scalar_kind: rumoca_ir_galec::ast::ScalarType,
    shape: Shape,
    facts: ValueFacts,
    source: ValueSource,
    stored_in: Option<EntityId>,
}

impl Value {
    pub(super) fn id(&self) -> ValueId {
        self.id
    }

    pub(super) fn scalar_kind(&self) -> rumoca_ir_galec::ast::ScalarType {
        self.scalar_kind
    }

    pub(super) fn shape(&self) -> &Shape {
        &self.shape
    }

    pub(super) fn facts(&self) -> &ValueFacts {
        &self.facts
    }

    pub(super) fn source(&self) -> ValueSource {
        self.source
    }

    pub(super) fn stored_in(&self) -> Option<EntityId> {
        self.stored_in
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Operation {
    id: OperationId,
    kind: OperationKind,
    inputs: Vec<ValueId>,
    outputs: Vec<ValueId>,
    phase: rumoca_ir_galec::ast::BlockMethodKind,
}

impl Operation {
    pub(super) fn id(&self) -> OperationId {
        self.id
    }

    pub(super) fn kind(&self) -> OperationKind {
        self.kind
    }

    pub(super) fn inputs(&self) -> &[ValueId] {
        &self.inputs
    }

    pub(super) fn outputs(&self) -> &[ValueId] {
        &self.outputs
    }

    pub(super) fn phase(&self) -> rumoca_ir_galec::ast::BlockMethodKind {
        self.phase
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shape {
    dimensions: Vec<usize>,
}

impl Shape {
    pub(super) fn new(dimensions: Vec<usize>) -> Self {
        Self { dimensions }
    }

    pub(super) fn dimensions(&self) -> &[usize] {
        &self.dimensions
    }

    pub(super) fn rank(&self) -> usize {
        self.dimensions.len()
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct NumericalFacts {
    entities: Vec<Entity>,
    values: Vec<Value>,
    operations: Vec<Operation>,
}

impl NumericalFacts {
    pub(super) fn new() -> Self {
        Self {
            entities: Vec::new(),
            values: Vec::new(),
            operations: Vec::new(),
        }
    }

    pub(super) fn add_entity(
        &mut self,
        name: String,
        scalar_kind: rumoca_ir_galec::ast::ScalarType,
        shape: Shape,
        role: EntityRole,
    ) -> EntityId {
        let id = EntityId(self.entities.len());

        self.entities.push(Entity {
            id,
            name,
            scalar_kind,
            shape,
            role,
        });

        id
    }

    pub(super) fn add_entity_read(&mut self, entity: EntityId) -> ValueId {
        let declaration = &self.entities[entity.0];
        let id = ValueId(self.values.len());
        self.values.push(Value {
            id,
            scalar_kind: declaration.scalar_kind,
            shape: declaration.shape.clone(),
            facts: ValueFacts::for_shape(&declaration.shape),
            source: ValueSource::EntityRead(entity),
            stored_in: Some(entity),
        });
        id
    }

    pub(super) fn add_literal(&mut self, literal: LiteralValue) -> ValueId {
        let scalar_kind = match literal {
            LiteralValue::Boolean(_) => rumoca_ir_galec::ast::ScalarType::Boolean,
            LiteralValue::Integer(_) => rumoca_ir_galec::ast::ScalarType::Integer,
            LiteralValue::Real(_) => rumoca_ir_galec::ast::ScalarType::Real,
        };
        let shape = Shape::new(Vec::new());
        let id = ValueId(self.values.len());
        self.values.push(Value {
            id,
            scalar_kind,
            facts: ValueFacts::for_shape(&shape),
            shape,
            source: ValueSource::Literal(literal),
            stored_in: None,
        });
        id
    }

    pub(super) fn add_operation(
        &mut self,
        kind: OperationKind,
        inputs: Vec<ValueId>,
        scalar_kind: rumoca_ir_galec::ast::ScalarType,
        shape: Shape,
        phase: rumoca_ir_galec::ast::BlockMethodKind,
        stored_in: Option<EntityId>,
    ) -> ValueId {
        let operation_id = OperationId(self.operations.len());
        let value_id = ValueId(self.values.len());
        self.values.push(Value {
            id: value_id,
            scalar_kind,
            facts: ValueFacts::for_shape(&shape),
            shape,
            source: ValueSource::Operation(operation_id),
            stored_in,
        });
        self.operations.push(Operation {
            id: operation_id,
            kind,
            inputs,
            outputs: vec![value_id],
            phase,
        });
        value_id
    }

    pub(super) fn entity(&self, id: EntityId) -> &Entity {
        &self.entities[id.0]
    }

    pub(super) fn value(&self, id: ValueId) -> &Value {
        &self.values[id.0]
    }

    pub(super) fn operation(&self, id: OperationId) -> &Operation {
        &self.operations[id.0]
    }

    pub(super) fn entities(&self) -> &[Entity] {
        &self.entities
    }

    pub(super) fn values(&self) -> &[Value] {
        &self.values
    }

    pub(super) fn operations(&self) -> &[Operation] {
        &self.operations
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum ProofStatus {
    ProvenTrue,
    ProvenFalse,

    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(super) struct Bounds {
    lower: Option<f64>,
    upper: Option<f64>,
}

impl Bounds {
    pub(super) fn new(lower: Option<f64>, upper: Option<f64>) -> Self {
        Self { lower, upper }
    }

    pub(super) fn lower(&self) -> Option<f64> {
        self.lower
    }

    pub(super) fn upper(&self) -> Option<f64> {
        self.upper
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum ValueFacts {
    Scalar(ScalarFacts),
    Vector(VectorFacts),
    Matrix(MatrixFacts),
}

impl ValueFacts {
    pub(super) fn for_shape(shape: &Shape) -> Self {
        match shape.rank() {
            0 => Self::scalar(),
            1 => Self::vector(),
            2 => Self::matrix(),
            rank => unreachable!("validated numerical shape has rank {rank}"),
        }
    }

    pub(super) fn scalar() -> Self {
        Self::Scalar(ScalarFacts::default())
    }

    pub(super) fn vector() -> Self {
        Self::Vector(VectorFacts::default())
    }

    pub(super) fn matrix() -> Self {
        Self::Matrix(MatrixFacts::default())
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub(super) struct ScalarFacts {
    finite: ProofStatus,
    nonzero: ProofStatus,
    positive: ProofStatus,
    bounds: Bounds,
}

impl ScalarFacts {
    pub(super) fn finite(&self) -> ProofStatus {
        self.finite
    }

    pub(super) fn nonzero(&self) -> ProofStatus {
        self.nonzero
    }

    pub(super) fn positive(&self) -> ProofStatus {
        self.positive
    }

    pub(super) fn bounds(&self) -> Bounds {
        self.bounds
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub(super) struct VectorFacts {
    all_finite: ProofStatus,
    zero_vector: ProofStatus,
    unit_vector: ProofStatus,
    element_bounds: Bounds,
}

impl VectorFacts {
    pub(super) fn all_finite(&self) -> ProofStatus {
        self.all_finite
    }

    pub(super) fn zero_vector(&self) -> ProofStatus {
        self.zero_vector
    }

    pub(super) fn unit_vector(&self) -> ProofStatus {
        self.unit_vector
    }

    pub(super) fn element_bounds(&self) -> Bounds {
        self.element_bounds
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Bandwidth {
    lower: usize,
    upper: usize,
}

impl Bandwidth {
    pub(super) fn new(lower: usize, upper: usize) -> Self {
        Self { lower, upper }
    }

    pub(super) fn lower(&self) -> usize {
        self.lower
    }

    pub(super) fn upper(&self) -> usize {
        self.upper
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub(super) struct MatrixFacts {
    symmetric: ProofStatus,
    positive_definite: ProofStatus,
    upper_triangular: ProofStatus,
    lower_triangular: ProofStatus,
    invertible: ProofStatus,
    known_rank: Option<usize>,
    sparsity: Option<SparsityPattern>,
    bandwidth: Option<Bandwidth>,
}

impl MatrixFacts {
    pub(super) fn symmetric(&self) -> ProofStatus {
        self.symmetric
    }

    pub(super) fn positive_definite(&self) -> ProofStatus {
        self.positive_definite
    }

    pub(super) fn upper_triangular(&self) -> ProofStatus {
        self.upper_triangular
    }

    pub(super) fn lower_triangular(&self) -> ProofStatus {
        self.lower_triangular
    }

    pub(super) fn invertible(&self) -> ProofStatus {
        self.invertible
    }

    pub(super) fn known_rank(&self) -> Option<usize> {
        self.known_rank
    }

    pub(super) fn sparsity(&self) -> Option<&SparsityPattern> {
        self.sparsity.as_ref()
    }

    pub(super) fn bandwidth(&self) -> Option<Bandwidth> {
        self.bandwidth
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SparsityPattern {
    may_be_nonzero: Vec<bool>,
}

impl SparsityPattern {
    pub(super) fn new(may_be_nonzero: Vec<bool>) -> Self {
        Self { may_be_nonzero }
    }

    pub(super) fn may_be_nonzero(&self) -> &[bool] {
        &self.may_be_nonzero
    }
}
