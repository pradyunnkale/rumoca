use super::facts::{MatrixFacts, NumericalFacts, ProofStatus, ValueFacts, ValueId};

pub(super) fn matrix_constructor_facts(
    facts: &NumericalFacts,
    rows: usize,
    columns: usize,
    elements: &[ValueId],
) -> ValueFacts {
    assert_eq!(
        elements.len(),
        rows * columns,
        "matrix constructor shape must agree with its scalar elements"
    );

    if rows != columns {
        return ValueFacts::Matrix(MatrixFacts::from_triangularity(
            ProofStatus::ProvenFalse,
            ProofStatus::ProvenFalse,
            ProofStatus::ProvenFalse,
        ));
    }

    let upper = triangular_status(facts, columns, elements, Triangle::Upper);
    let lower = triangular_status(facts, columns, elements, Triangle::Lower);
    let invertible = triangular_invertibility(facts, columns, elements, upper, lower);
    ValueFacts::Matrix(MatrixFacts::from_triangularity(upper, lower, invertible))
}

#[derive(Clone, Copy)]
enum Triangle {
    Upper,
    Lower,
}

fn triangular_status(
    facts: &NumericalFacts,
    size: usize,
    elements: &[ValueId],
    triangle: Triangle,
) -> ProofStatus {
    let statuses = (0..size).flat_map(|row| {
        (0..size)
            .filter(move |column| outside_triangle(row, *column, triangle))
            .map(move |column| zero_status(facts.value(elements[row * size + column]).facts()))
    });
    prove_all(statuses)
}

fn outside_triangle(row: usize, column: usize, triangle: Triangle) -> bool {
    match triangle {
        Triangle::Upper => row > column,
        Triangle::Lower => row < column,
    }
}

fn triangular_invertibility(
    facts: &NumericalFacts,
    size: usize,
    elements: &[ValueId],
    upper: ProofStatus,
    lower: ProofStatus,
) -> ProofStatus {
    if upper != ProofStatus::ProvenTrue && lower != ProofStatus::ProvenTrue {
        return ProofStatus::Unknown;
    }

    prove_all(
        (0..size).map(|index| nonzero_status(facts.value(elements[index * size + index]).facts())),
    )
}

fn zero_status(facts: &ValueFacts) -> ProofStatus {
    match nonzero_status(facts) {
        ProofStatus::ProvenTrue => ProofStatus::ProvenFalse,
        ProofStatus::ProvenFalse => ProofStatus::ProvenTrue,
        ProofStatus::Unknown => ProofStatus::Unknown,
    }
}

fn nonzero_status(facts: &ValueFacts) -> ProofStatus {
    let ValueFacts::Scalar(scalar) = facts else {
        panic!("matrix constructor elements must be scalar values");
    };
    scalar.nonzero()
}

fn prove_all(statuses: impl Iterator<Item = ProofStatus>) -> ProofStatus {
    let mut result = ProofStatus::ProvenTrue;
    for status in statuses {
        match status {
            ProofStatus::ProvenFalse => return ProofStatus::ProvenFalse,
            ProofStatus::Unknown => result = ProofStatus::Unknown,
            ProofStatus::ProvenTrue => {}
        }
    }
    result
}
