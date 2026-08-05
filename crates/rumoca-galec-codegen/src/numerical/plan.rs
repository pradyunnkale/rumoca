use super::facts::{NumericalFacts, OperationId, OperationKind, ProofStatus, ValueFacts};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LinearSolveAlgorithm {
    Diagonal,
    ForwardSubstitution,
    BackwardSubstitution,
    GenericPivoted,
}

impl LinearSolveAlgorithm {
    pub(crate) const fn kernel_name(self) -> &'static str {
        match self {
            Self::Diagonal => "diagonal",
            Self::ForwardSubstitution => "forward",
            Self::BackwardSubstitution => "backward",
            Self::GenericPivoted => "generic_pivoted",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct LinearSolvePlan {
    operation: OperationId,
    algorithm: LinearSolveAlgorithm,
}

impl LinearSolvePlan {
    #[cfg(test)]
    pub(super) fn operation(&self) -> OperationId {
        self.operation
    }

    pub(super) fn algorithm(&self) -> LinearSolveAlgorithm {
        self.algorithm
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct NumericalPlan {
    linear_solves: Vec<LinearSolvePlan>,
}

impl NumericalPlan {
    #[cfg(test)]
    pub(super) fn linear_solves(&self) -> &[LinearSolvePlan] {
        &self.linear_solves
    }

    pub(crate) fn linear_solve_algorithms(
        &self,
    ) -> impl ExactSizeIterator<Item = LinearSolveAlgorithm> + '_ {
        self.linear_solves.iter().map(LinearSolvePlan::algorithm)
    }
}

pub(super) fn plan(facts: &NumericalFacts) -> NumericalPlan {
    let linear_solves = facts
        .operations()
        .iter()
        .filter(|operation| operation.kind() == OperationKind::LinearSolve)
        .map(|operation| {
            let matrix = operation
                .inputs()
                .first()
                .expect("analyzed linear solve must have a matrix input");
            LinearSolvePlan {
                operation: operation.id(),
                algorithm: select_linear_solve(facts.value(*matrix).facts()),
            }
        })
        .collect();
    NumericalPlan { linear_solves }
}

fn select_linear_solve(facts: &ValueFacts) -> LinearSolveAlgorithm {
    let ValueFacts::Matrix(matrix) = facts else {
        panic!("analyzed linear solve matrix input must have matrix facts");
    };
    let proven_invertible = matrix.invertible() == ProofStatus::ProvenTrue;
    let upper = matrix.upper_triangular() == ProofStatus::ProvenTrue;
    let lower = matrix.lower_triangular() == ProofStatus::ProvenTrue;

    match (proven_invertible, upper, lower) {
        (true, true, true) => LinearSolveAlgorithm::Diagonal,
        (true, false, true) => LinearSolveAlgorithm::ForwardSubstitution,
        (true, true, false) => LinearSolveAlgorithm::BackwardSubstitution,
        _ => LinearSolveAlgorithm::GenericPivoted,
    }
}
