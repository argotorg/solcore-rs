use super::hir_nameres;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) enum ExportResolutionMode {
    Lenient,
    Strict,
}

impl ExportResolutionMode {
    pub(super) fn is_strict(self) -> bool {
        matches!(self, Self::Strict)
    }

    pub(super) fn suppress_if(self, suppress: bool) -> Self {
        match (self, suppress) {
            (Self::Strict, false) => Self::Strict,
            _ => Self::Lenient,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) enum CtorInclusion {
    Exclude,
    #[allow(dead_code)]
    Include,
}

impl CtorInclusion {
    pub(super) fn includes_data_ctors(self) -> bool {
        matches!(self, Self::Include)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) enum BodyDiagnosticPolicy {
    Emit,
    SuppressForParseErrors,
}

impl BodyDiagnosticPolicy {
    pub(super) fn from_parse_errors(has_parse_errors: bool) -> Self {
        if has_parse_errors {
            Self::SuppressForParseErrors
        } else {
            Self::Emit
        }
    }

    pub(super) fn from_suppress_for_parse_errors(suppress_for_parse_errors: bool) -> Self {
        if suppress_for_parse_errors {
            Self::SuppressForParseErrors
        } else {
            Self::Emit
        }
    }

    pub(super) fn as_hir_policy(self) -> hir_nameres::NameresDiagnosticPolicy {
        match self {
            Self::Emit => hir_nameres::NameresDiagnosticPolicy::Emit,
            Self::SuppressForParseErrors => {
                hir_nameres::NameresDiagnosticPolicy::SuppressForParseErrors
            }
        }
    }

    pub(super) fn suppress_for_parse_errors(self) -> bool {
        matches!(self, Self::SuppressForParseErrors)
    }
}
