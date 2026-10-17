//! The canonical names of the units of work the kernel opens.

use crate::operation_log::Unit;

/// One lifecycle hook, init or shutdown: no caller, so no edge dispatches it.
pub const HOOK: Unit =
    crate::unit!("lifecycle.hook", target: crate::target::LIFECYCLE, kind: Internal);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operation_log::Kind;

    #[test]
    fn a_hook_is_the_kernels_unit_on_its_own_target_with_no_edge() {
        assert_eq!(
            (HOOK.name(), HOOK.target(), HOOK.kind(), HOOK.edge()),
            (
                "lifecycle.hook",
                crate::target::LIFECYCLE,
                Kind::Internal,
                None
            ),
        );
        assert!(crate::__private::unit_opened_by(&HOOK, "nest-rs-core"));
        assert!(!crate::__private::unit_opened_by(&HOOK, "nest-rs-database"));
    }
}
