// errors.rs, specifies the different errors that can occur, this file will be big because a lot of
// modelica models will be rejected here

use std::fmt;

#[derive(Debug)]
pub enum AdmissibilityError {
    HasContinuousStates,
    HasContinuousEquations,
    HasAlgebraicVariables,
    NoClock,
    MultiRate { count: usize },
    HasDynamicClocks,
    HasRuntimeEvents,
    AlgebraicLoop,
}

impl fmt::Display for AdmissibilityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HasContinuousStates => write!(f, "model has continuous-time state variables; GALEC requires a purely discrete model"),
            Self::HasContinuousEquations => write!(f, "model has continuous-time equations; GALEC requires a purely discrete model"),
            Self::HasAlgebraicVariables => write!(f, "model has algebraic variables; GALEC does not support algebraic loops or index > 0"),
            Self::NoClock => write!(f, "model has no periodic clock; GALEC requires exactly one fixed-period sample clock"),
            Self::MultiRate { count } => write!(f, "model has {count} clocks; GALEC requires exactly one fixed-period sample clock"),
            Self::HasDynamicClocks => write!(f, "model has dynamically-triggered clock conditions; GALEC only supports static periodic clocks"),
            Self::HasRuntimeEvents => write!(f, "model has runtime event actions; GALEC does not support event handling"),
            Self::AlgebraicLoop => write!(f, "discrete equations form an algebraic loop and cannot be topologically sorted"),
        }
    }
}
