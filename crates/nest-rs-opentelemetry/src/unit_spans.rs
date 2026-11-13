//! The spans `operation_span!` opens, one per unit of work, admitted to the
//! trace export whatever the console's filter writes: `RUST_LOG` says what the
//! console and the log export write, and the sampler which traces are kept.

use tracing::Metadata;
use tracing::subscriber::Interest;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::{Context, Filter};

/// Admits every unit's span where traces export, and nothing else: where none
/// export, it neither opens a span nor raises the process's level.
pub(crate) struct UnitSpans {
    exported: bool,
}

impl UnitSpans {
    /// Every unit's span when `exported`, nothing otherwise.
    pub(crate) fn new(exported: bool) -> Self {
        Self { exported }
    }
}

/// Whether `metadata` is shaped as the span `operation_span!` opens, which
/// alone declares the parent's id among the framework's spans; a library may
/// declare it too, so only a unit pending as the span is created is one.
pub(crate) fn is_unit_span(metadata: &Metadata<'_>) -> bool {
    metadata.is_span()
        && metadata
            .fields()
            .field(nest_rs_core::__private::field::PARENT_SPAN_ID)
            .is_some()
}

impl<S> Filter<S> for UnitSpans {
    fn enabled(&self, metadata: &Metadata<'_>, _: &Context<'_, S>) -> bool {
        self.exported && is_unit_span(metadata) && nest_rs_core::__private::pending_unit().is_some()
    }

    fn callsite_enabled(&self, metadata: &'static Metadata<'static>) -> Interest {
        if self.exported && is_unit_span(metadata) {
            Interest::sometimes()
        } else {
            Interest::never()
        }
    }

    fn max_level_hint(&self) -> Option<LevelFilter> {
        Some(if self.exported {
            LevelFilter::INFO
        } else {
            LevelFilter::OFF
        })
    }
}
