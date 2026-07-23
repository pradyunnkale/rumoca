//! Phase-local errors for compile-time numerical analysis of GALEC blocks.

use rumoca_core::{Diagnostic, PhaseError, PrimaryLabel, Span};

/// Errors produced while building numerical facts from a validated GALEC block.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub(super) enum NumericalAnalysisError {
    /// The first embedded numerical profile handles only primitive values.
    #[error(
        "entity `{entity}` has state-compartment type `{compartment}`; state-compartment values are not yet supported by numerical analysis"
    )]
    UnsupportedComponentType {
        entity: String,
        compartment: String,
        span: Span,
    },

    /// A required fixed dimension was not a positive integer literal.
    #[error("dimension {dimension} of `{entity}` is invalid: {reason}")]
    InvalidDimension {
        entity: String,
        /// One-based dimension position as shown to the user.
        dimension: usize,
        reason: String,
        span: Span,
    },

    /// The first numerical-planner profile is intentionally limited to rank two.
    #[error(
        "entity `{entity}` has rank {rank}; numerical analysis currently supports only scalars, vectors, and matrices"
    )]
    UnsupportedRank {
        entity: String,
        rank: usize,
        span: Span,
    },

    /// State-compartment declarations need an explicit flattening policy.
    #[error(
        "block contains {count} state-compartment declaration(s); state compartments are not yet supported by numerical analysis"
    )]
    UnsupportedStateCompartments { count: usize, span: Span },

    /// Duplicate names would make reference-to-entity resolution ambiguous.
    #[error("entity `{entity}` is declared more than once in the numerical scope")]
    DuplicateEntity { entity: String, span: Span },

    /// Method locals require a separate lexical scope and value lifetime.
    #[error("method-local entity `{entity}` is not yet supported by numerical analysis")]
    UnsupportedMethodLocal { entity: String, span: Span },

    /// The first operation pass accepts only direct assignments.
    #[error("{statement} statements are not yet supported by numerical analysis")]
    UnsupportedStatement { statement: &'static str, span: Span },

    /// The first operation pass recognizes a deliberately small expression set.
    #[error("{expression} expressions are not yet supported by numerical analysis")]
    UnsupportedExpression {
        expression: &'static str,
        span: Span,
    },

    /// Only direct, unsubscripted block-state references are in the first slice.
    #[error("reference `{reference}` is not supported by the initial numerical profile")]
    UnsupportedReference { reference: String, span: Span },

    /// A validated GALEC reference did not resolve to a registered block entity.
    #[error("reference `{reference}` does not name a registered numerical entity")]
    UnknownEntityReference { reference: String, span: Span },

    /// Operand/target metadata did not satisfy the recognized operation contract.
    #[error("invalid numerical operation: {detail}")]
    InvalidOperation { detail: String, span: Span },
}

impl NumericalAnalysisError {
    /// Stable diagnostic code in the GALEC target range.
    #[must_use]
    pub(super) const fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedComponentType { .. } => "ET024",
            Self::InvalidDimension { .. } => "ET025",
            Self::UnsupportedRank { .. } => "ET026",
            Self::UnsupportedStateCompartments { .. } => "ET027",
            Self::DuplicateEntity { .. } => "ET028",
            Self::UnsupportedMethodLocal { .. } => "ET029",
            Self::UnsupportedStatement { .. } => "ET030",
            Self::UnsupportedExpression { .. } => "ET031",
            Self::UnsupportedReference { .. } => "ET032",
            Self::UnknownEntityReference { .. } => "ET033",
            Self::InvalidOperation { .. } => "ET034",
        }
    }

    #[must_use]
    const fn span(&self) -> Span {
        match self {
            Self::UnsupportedComponentType { span, .. }
            | Self::InvalidDimension { span, .. }
            | Self::UnsupportedRank { span, .. }
            | Self::UnsupportedStateCompartments { span, .. }
            | Self::DuplicateEntity { span, .. }
            | Self::UnsupportedMethodLocal { span, .. }
            | Self::UnsupportedStatement { span, .. }
            | Self::UnsupportedExpression { span, .. }
            | Self::UnsupportedReference { span, .. }
            | Self::UnknownEntityReference { span, .. }
            | Self::InvalidOperation { span, .. } => *span,
        }
    }

    #[must_use]
    const fn label(&self) -> &'static str {
        match self {
            Self::UnsupportedComponentType { .. } => "component-typed entity declared here",
            Self::InvalidDimension { .. } => "invalid fixed dimension",
            Self::UnsupportedRank { .. } => "rank exceeds the numerical target profile",
            Self::UnsupportedStateCompartments { .. } => {
                "block with unsupported state compartments"
            }
            Self::DuplicateEntity { .. } => "duplicate numerical entity",
            Self::UnsupportedMethodLocal { .. } => "method-local declaration",
            Self::UnsupportedStatement { .. } => "unsupported statement",
            Self::UnsupportedExpression { .. } => "unsupported expression",
            Self::UnsupportedReference { .. } => "unsupported reference shape",
            Self::UnknownEntityReference { .. } => "unresolved numerical reference",
            Self::InvalidOperation { .. } => "invalid operation metadata",
        }
    }
}

impl PhaseError for NumericalAnalysisError {
    fn to_diagnostic(&self) -> Diagnostic {
        let span = self.span();
        if span.is_dummy() {
            Diagnostic::global_error(self.code(), self.to_string())
        } else {
            Diagnostic::error(
                self.code(),
                self.to_string(),
                PrimaryLabel::new(span).with_message(self.label()),
            )
        }
    }
}
