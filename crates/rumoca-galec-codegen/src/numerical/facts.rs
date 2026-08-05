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
    ArrayConstruct,
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
    #[cfg(test)]
    pub(super) fn id(&self) -> EntityId {
        self.id
    }

    #[cfg(test)]
    pub(super) fn name(&self) -> &str {
        &self.name
    }

    pub(super) fn scalar_kind(&self) -> rumoca_ir_galec::ast::ScalarType {
        self.scalar_kind
    }

    pub(super) fn shape(&self) -> &Shape {
        &self.shape
    }

    #[cfg(test)]
    pub(super) fn role(&self) -> EntityRole {
        self.role
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Value {
    scalar_kind: rumoca_ir_galec::ast::ScalarType,
    shape: Shape,
    facts: ValueFacts,
    source: ValueSource,
    stored_in: Option<EntityId>,
}

impl Value {
    pub(super) fn scalar_kind(&self) -> rumoca_ir_galec::ast::ScalarType {
        self.scalar_kind
    }

    pub(super) fn shape(&self) -> &Shape {
        &self.shape
    }

    pub(super) fn facts(&self) -> &ValueFacts {
        &self.facts
    }

    #[cfg(test)]
    pub(super) fn source(&self) -> ValueSource {
        self.source
    }

    #[cfg(test)]
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

pub(super) struct OperationOutput {
    scalar_kind: rumoca_ir_galec::ast::ScalarType,
    shape: Shape,
    facts: ValueFacts,
    stored_in: Option<EntityId>,
}

impl OperationOutput {
    pub(super) fn unknown(scalar_kind: rumoca_ir_galec::ast::ScalarType, shape: Shape) -> Self {
        let facts = ValueFacts::for_shape(&shape);
        Self {
            scalar_kind,
            shape,
            facts,
            stored_in: None,
        }
    }

    pub(super) fn inferred(
        scalar_kind: rumoca_ir_galec::ast::ScalarType,
        shape: Shape,
        facts: ValueFacts,
    ) -> Self {
        Self {
            scalar_kind,
            shape,
            facts,
            stored_in: None,
        }
    }

    pub(super) fn assignment(
        scalar_kind: rumoca_ir_galec::ast::ScalarType,
        shape: Shape,
        facts: ValueFacts,
        stored_in: EntityId,
    ) -> Self {
        Self {
            scalar_kind,
            shape,
            facts,
            stored_in: Some(stored_in),
        }
    }
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

    #[cfg(test)]
    pub(super) fn outputs(&self) -> &[ValueId] {
        &self.outputs
    }

    #[cfg(test)]
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
            scalar_kind,
            facts: ValueFacts::for_literal(literal),
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
        phase: rumoca_ir_galec::ast::BlockMethodKind,
        output: OperationOutput,
    ) -> ValueId {
        let operation_id = OperationId(self.operations.len());
        let value_id = ValueId(self.values.len());
        self.values.push(Value {
            scalar_kind: output.scalar_kind,
            facts: output.facts,
            shape: output.shape,
            source: ValueSource::Operation(operation_id),
            stored_in: output.stored_in,
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

    #[cfg(test)]
    pub(super) fn entities(&self) -> &[Entity] {
        &self.entities
    }

    #[cfg(test)]
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
    #[allow(dead_code, reason = "reserved for later SPEC_0035 transfer functions")]
    lower: Option<f64>,
    #[allow(dead_code, reason = "reserved for later SPEC_0035 transfer functions")]
    upper: Option<f64>,
}

impl Bounds {
    pub(super) fn new(lower: Option<f64>, upper: Option<f64>) -> Self {
        Self { lower, upper }
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

    pub(super) fn for_literal(literal: LiteralValue) -> Self {
        Self::Scalar(ScalarFacts::for_literal(literal))
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
    #[allow(dead_code, reason = "reserved for later SPEC_0035 transfer functions")]
    finite: ProofStatus,
    nonzero: ProofStatus,
    #[allow(dead_code, reason = "reserved for later SPEC_0035 transfer functions")]
    positive: ProofStatus,
    #[allow(dead_code, reason = "reserved for later SPEC_0035 transfer functions")]
    bounds: Bounds,
}

impl ScalarFacts {
    fn for_literal(literal: LiteralValue) -> Self {
        match literal {
            LiteralValue::Boolean(_) => Self::default(),
            LiteralValue::Integer(value) => Self {
                finite: ProofStatus::ProvenTrue,
                nonzero: proof(value != 0),
                positive: proof(value > 0),
                bounds: Bounds::default(),
            },
            LiteralValue::Real(value) if value.is_finite() => Self {
                finite: ProofStatus::ProvenTrue,
                nonzero: proof(value != 0.0),
                positive: proof(value > 0.0),
                bounds: Bounds::new(Some(value), Some(value)),
            },
            LiteralValue::Real(_) => Self {
                finite: ProofStatus::ProvenFalse,
                ..Self::default()
            },
        }
    }

    pub(super) fn nonzero(&self) -> ProofStatus {
        self.nonzero
    }
}

#[allow(dead_code, reason = "reserved for later SPEC_0035 vector inference")]
#[derive(Debug, Clone, PartialEq, Default)]
pub(super) struct VectorFacts {
    all_finite: ProofStatus,
    zero_vector: ProofStatus,
    unit_vector: ProofStatus,
    element_bounds: Bounds,
}

#[allow(dead_code, reason = "reserved for later SPEC_0035 banded kernels")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Bandwidth {
    lower: usize,
    upper: usize,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub(super) struct MatrixFacts {
    #[allow(dead_code, reason = "reserved for later SPEC_0035 Cholesky selection")]
    symmetric: ProofStatus,
    #[allow(dead_code, reason = "reserved for later SPEC_0035 Cholesky selection")]
    positive_definite: ProofStatus,
    upper_triangular: ProofStatus,
    lower_triangular: ProofStatus,
    invertible: ProofStatus,
    #[allow(dead_code, reason = "reserved for later SPEC_0035 rank inference")]
    known_rank: Option<usize>,
    #[allow(dead_code, reason = "reserved for later SPEC_0035 sparse kernels")]
    sparsity: Option<SparsityPattern>,
    #[allow(dead_code, reason = "reserved for later SPEC_0035 banded kernels")]
    bandwidth: Option<Bandwidth>,
}

impl MatrixFacts {
    pub(super) fn from_triangularity(
        upper_triangular: ProofStatus,
        lower_triangular: ProofStatus,
        invertible: ProofStatus,
    ) -> Self {
        Self {
            upper_triangular,
            lower_triangular,
            invertible,
            ..Self::default()
        }
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
}

#[allow(dead_code, reason = "reserved for later SPEC_0035 sparse kernels")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SparsityPattern {
    may_be_nonzero: Vec<bool>,
}

fn proof(value: bool) -> ProofStatus {
    if value {
        ProofStatus::ProvenTrue
    } else {
        ProofStatus::ProvenFalse
    }
}
