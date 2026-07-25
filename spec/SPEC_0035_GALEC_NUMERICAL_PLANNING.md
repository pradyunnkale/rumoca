# SPEC_0035: GALEC Numerical Planning

## Status

DRAFT

## Summary

Embedded GALEC backends derive sound numerical facts and an explicit algorithm
plan without changing GALEC semantics or the canonical Rumoca IR pipeline.

## Motivation

- Fixed-size embedded models permit numerical choices unavailable to C/Rust compilers.
- EKF workloads benefit from structure-aware matrix kernels and linear solves.
- Proof failure must select a safe fallback, never an unsound specialized algorithm.
- C and future Rust emitters should consume the same target-neutral plan.

## Specification

### Pipeline Position

```text
AST -> Flat -> DAE -> Solve                         canonical pipeline
                 \
                  -> AlgorithmCodePackage (GALEC)  export artifact
                       -> NumericalFacts            derived analysis
                       -> NumericalPlan             backend-neutral choices
                       -> embedded C/Rust context   target rendering
```

| Rule | Owner/Where | Brief Justification |
|---|---|---|
| `NumericalFacts` and `NumericalPlan` are not canonical IR stages | `rumoca-galec-codegen` | GALEC remains an export artifact |
| Plain GALEC and Algorithm Code emission do not require a numerical plan | GALEC emitters | Optimization must not restrict GALEC |
| Only embedded targets invoke numerical planning | embedded C/Rust facades | Limits belong to consuming targets |
| Planning never mutates the GALEC AST | numerical planner | Preserve validation and traceability |
| Generated C/Rust remains template-owned | `rumoca-phase-codegen` templates | Required by SPEC_0034 GAL-008 |

### Current Types

Located in `crates/rumoca-galec-codegen/src/numerical/facts.rs`:

```rust
pub struct NumericalFacts {
    entities: Vec<Entity>,
    values: Vec<Value>,
    operations: Vec<Operation>,
}

pub struct Entity {
    id: EntityId,
    name: String,
    scalar_kind: rumoca_ir_galec::ast::ScalarType,
    shape: Shape,
    role: EntityRole,
}
```

Located in `crates/rumoca-galec-codegen/src/numerical/analyze.rs`:

```rust
pub(super) fn analyze(
    block: &rumoca_ir_galec::ast::Block,
) -> Result<NumericalFacts, NumericalAnalysisError>
```

The initial declaration and operation pass is implemented for direct
assignments, arithmetic expressions, fixed rank-one/rank-two array
constructors, and `solveLinearEquations`. Initial triangularity inference and
linear-solve planning are implemented. Full GALEC statement/expression
coverage and production routing remain implementation gaps.

The initial embedded C lowering also selects helper-backed representations for
basic `Real` array arithmetic. This is a typed codegen selection at the GALEC
to C-context boundary; routing the general `NumericalPlan` through production
codegen remains future work. Rust supplies structured `array_binary` and `dot`
context data, while the Jinja template owns helper definitions and C syntax.

### Fact Ownership

| Rule | Owner/Where | Brief Justification |
|---|---|---|
| `Entity` records declaration/storage identity | `facts.rs` | Storage identity survives assignments |
| `Value` records one computed value and its numerical facts | `facts.rs` | Properties may change per assignment |
| `Operation` records inputs, outputs, and `BlockMethodKind` | `facts.rs` | Planning needs dataflow and frequency |
| Name lookup is analysis-local and deterministic | `analyze.rs` | Lookup caches are not output facts |
| Facts retain stable IDs instead of vector indices in callers | `facts.rs` | Prevent index coupling |

### Proof Contract

Boolean predicates use `ProofStatus::{ProvenTrue, ProvenFalse, Unknown}`.

| Rule | Owner/Where | Brief Justification |
|---|---|---|
| Specialized algorithms require every prerequisite `ProvenTrue` | `plan.rs` | Unknown is not evidence |
| `ProvenFalse` means a predicate was disproved | `analyze.rs` | Distinguish false from unknown |
| Conflicting proofs produce an internal phase error | fact update API | Contradictions indicate compiler bugs |
| Runtime samples never establish compile-time structure | `analyze.rs` | Observed zeros are not structural zeros |
| Structural sparsity marks guaranteed-zero positions | `SparsityPattern` | Skipped computation must be sound |
| Proofs eventually carry their derivation/evidence | numerical facts | Decisions must be explainable |

Branch joins retain only facts proven on every reachable path. Loops use their
statically bounded GALEC semantics; analysis must not guess a fixed point.

For a fixed, square matrix constructor:

- upper triangular is `ProvenTrue` only when every entry below the diagonal is
  proven zero, and `ProvenFalse` when any such entry is proven nonzero;
- lower triangular uses the corresponding entries above the diagonal;
- triangular invertibility is `ProvenTrue` only when triangularity and every
  diagonal entry being nonzero are both proven;
- any missing element proof produces `Unknown`, not a guessed result.

Assignments preserve the input value's facts. Exact zero/nonzero
classification of a finite source literal is a compile-time structural fact,
not a runtime tolerance test.

### Numerical Planning Contract

| Rule | Owner/Where | Brief Justification |
|---|---|---|
| Planning selects an implementation for an existing GALEC operation | `plan.rs` | Do not invent mathematics |
| Planning preserves GALEC evaluation order | `plan.rs` | GALEC order is normative |
| Planning preserves error-signal and NaN behavior | plan + kernels | Numerical speed cannot change semantics |
| General dense implementations are mandatory fallbacks | numerical library | Unknown facts remain executable |
| Kernel names are plan data, not emitted source strings | plan + template context | Keep rendering template-owned |

The initial planner rules are:

| Operation | Required facts | Selected implementation |
|---|---|---|
| Linear solve | square, invertible, lower and upper triangular | diagonal solve |
| Linear solve | square, invertible, lower triangular | forward substitution |
| Linear solve | square, invertible, upper triangular | backward substitution |
| Linear solve | otherwise | generic pivoted solve |

The planner should prefer a solve over materializing an inverse when the GALEC
operation permits it. Algebraic rewrites such as Woodbury are outside the first
milestone because they require stronger equivalence and error-semantics proofs.
Positive-definite Cholesky selection remains a later rule after
positive-definiteness inference is implemented.

### Initial Embedded Profile

| Capability | Initial support | Behavior outside support |
|---|---|---|
| Scalar, vector, matrix values | Required | — |
| Rank greater than two | Not planned initially | Target-specific diagnostic |
| Primitive GALEC scalar types | Required | — |
| State-compartment values | Not planned initially | Target-specific diagnostic |
| Array add/subtract/multiply/divide | Typed static C helpers for whole `Real` arrays and scalar broadcast | Existing scalar-expanded emission for expressions outside the helper pattern |
| Dot product | Complete ordered left-associated sum of pairwise products over equally-sized `Real` vectors | Preserve the original scalar expression |
| 3D cross product | Complete ordered three-component determinant pattern | Preserve the original component assignments |
| Quaternion gravity rotation | Complete ordered three-component unit-axis rotation pattern | Preserve the original component assignments |
| Quaternion angular-rate integration | Complete ordered four-component first-order integration pattern | Preserve the original component assignments |
| Linear solve | Required for EKF | Pivoted general fallback |
| Matrix product/transpose recognition | Deferred until a valid GALEC loop/function pattern exists | Existing generic emission |
| Sparse kernels | Deferred | Dense fallback |

Rank and compartment limits apply only when a backend requests this numerical
profile. They must not reject plain GALEC or eFMI Algorithm Code generation.

### Diagnostics

| Code | Meaning | Best source span |
|---|---|---|
| `ET024` | Component-typed entity unsupported by initial profile | Entity declaration |
| `ET025` | Invalid fixed dimension | Variable declaration |
| `ET026` | Rank exceeds initial profile | Variable declaration |
| `ET027` | State compartments unsupported by initial profile | Compartment declaration |
| `ET028` | Duplicate numerical entity | Entity declaration |
| `ET029` | Method-local entity unsupported by initial profile | Local declaration |
| `ET030` | Statement unsupported by initial operation pass | Statement |
| `ET031` | Expression unsupported by initial operation pass | Enclosing statement |
| `ET032` | Reference shape unsupported by initial profile | Reference |
| `ET033` | Reference does not resolve to a registered entity | Reference |
| `ET034` | Recognized operation has incompatible metadata | Enclosing statement |

Errors implement `rumoca_core::PhaseError`; missing required semantic data is
never defaulted.

### Required Tests

| Test | Enforces |
|---|---|
| All six GALEC entity roles map in declaration order | Declaration analysis |
| Scalar/vector/matrix shapes and EKF declarations profile correctly | Shape analysis |
| Invalid dimension, rank, component, and compartment cases return stable codes | Diagnostics |
| `x := solveLinearEquations(S, residual)` produces solve then assignment in `DoStep` | Operation analysis |
| Array `*` is never mislabeled as matrix multiplication | GALEC semantic preservation |
| Whole-array element-wise arithmetic and scalar broadcasts render typed helper calls | Readable helper-backed C |
| A complete ordered left-associated vector sum-of-products renders a dot helper | Safe dot recovery |
| Partial, reordered, or right-associated sum-of-products remains scalar C | Evaluation-order preservation |
| Complete cross-product and quaternion component groups render one typed helper call | Recover operations scalarized before C lowering |
| Incomplete or modified component groups remain scalar C | Never guess a higher-level operation |
| A fixed lower-triangular matrix proves lower true, upper false, and invertible true | Triangularity inference |
| Proven lower/upper triangular solves choose forward/backward substitution | Specialized planning |
| An unknown matrix chooses the generic pivoted solve | Sound fallback |
| Generated C compiles and matches the dense reference result | Backend integration |

## Rationale

GALEC array arithmetic is element-wise with scalar broadcast; it has no
matrix-product or transpose builtin. The planner MUST NOT infer matrix
multiplication from a rank-two `BinaryOp::Mul`. Future matrix-product recovery
requires a proven GALEC loop or user-function pattern.

This layer consumes GALEC rather than Solve because GALEC retains block
methods, named entities, and export-level structure needed by readable embedded
code. It remains target-owned derived data so it does not compete with the
canonical AST/Flat/DAE/Solve contracts in SPEC_0007.

## References

- [SPEC_0007](SPEC_0007_IR_PIPELINE.md) — canonical IR contracts
- [SPEC_0008](SPEC_0008_PHASE_ERRORS.md) — phase-local diagnostics
- [SPEC_0029](SPEC_0029_CRATE_BOUNDARIES.md) — crate and template ownership
- [SPEC_0034](SPEC_0034_GALEC_EFMI_EXPORT.md) — GALEC/eFMI export contract
