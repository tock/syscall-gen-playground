#![allow(unused, reason = "Just for experimentation")]

/// Stands in for a board crate's driver-number registry, which
/// `syscall_drivers/button.rs` looks up via `crate::driver`.
mod driver {
    #[derive(Clone, Copy)]
    pub enum NUM {
        Button = 3,
    }
}

#[path = "../syscall_drivers/button.rs"]
mod button;

use core::cell::Cell;

use kernel::grant::{AllowRoCount, AllowRwCount, Grant, UpcallCount};
use kernel::syscall::{CommandReturn, SyscallDriver};
use kernel::{ErrorCode, ProcessId};
use macros::{allow_read_only, allow_read_write, command, subscribe, syscall};

#[derive(Default)]
struct App;

type CounterGrant = Grant<App, UpcallCount<1>, AllowRoCount<0>, AllowRwCount<0>>;

struct Counter {
    value: Cell<u32>,
    apps: CounterGrant,
}

#[syscall(grant = apps)]
impl Counter {
    fn new(apps: CounterGrant) -> Self {
        Self {
            value: Cell::new(0),
            apps,
        }
    }

    #[command(0, SuccessU32, Failure)]
    fn get(&self) -> CommandReturn {
        CommandReturn::success_u32(self.value.get())
    }

    #[command(1, Success, Failure)]
    fn increment(&self, processid: ProcessId) -> CommandReturn {
        self.value.set(self.value.get() + 1);
        let _ = self.apps.enter(processid, |_, _| {});
        CommandReturn::success()
    }

    #[command(2, Success, Failure)]
    fn set(&self, value: usize) -> CommandReturn {
        self.value.set(value as u32);
        CommandReturn::success()
    }
}

#[subscribe]
enum Subscribe {
    /// Fired whenever the count changes.
    Changed { value: u32 } = 0,
}

#[allow_read_only]
enum ROAllow {}

#[allow_read_write]
enum RWAllow {}

#[cfg(test)]
mod tests {
    use super::*;

    fn counter() -> Counter {
        Counter::new(CounterGrant::new(App))
    }

    #[test]
    fn get_returns_current_value() {
        let c = counter();
        assert!(matches!(
            c.command(0, 0, 0, ProcessId),
            CommandReturn::SuccessU32(0)
        ));
    }

    #[test]
    fn increment_then_get() {
        let c = counter();
        assert!(c.command(1, 0, 0, ProcessId).is_success());
        assert!(matches!(
            c.command(0, 0, 0, ProcessId),
            CommandReturn::SuccessU32(1)
        ));
    }

    #[test]
    fn set_writes_the_data_argument() {
        let c = counter();
        assert!(c.command(2, 42, 0, ProcessId).is_success());
        assert!(matches!(
            c.command(0, 0, 0, ProcessId),
            CommandReturn::SuccessU32(42)
        ));
    }

    #[test]
    fn unknown_command_is_nosupport() {
        let c = counter();
        assert!(matches!(
            c.command(99, 0, 0, ProcessId),
            CommandReturn::Failure(ErrorCode::NOSUPPORT)
        ));
    }

    #[test]
    fn allocate_grant_was_generated() {
        let c = counter();
        assert!(c.allocate_grant(ProcessId).is_ok());
    }

    #[test]
    fn upcall_const_was_generated() {
        assert_eq!(UPCALL_CHANGED, 0);
    }
}

/// Exercises the *real* `syscall_drivers/button.rs` (included above as
/// `mod button`) through the macros, proving they generate a working
/// `SyscallDriver` for it -- not just for the simplified `Counter` above.
#[cfg(test)]
mod button_tests {
    use super::button::{App, Button, UPCALL_TOGGLED};
    use kernel::grant::Grant;
    use kernel::hil::gpio::{
        ActivationMode, ActivationState, Configure, FloatingState, Input, InterruptEdge,
        InterruptValueWrapper, InterruptWithValue,
    };
    use kernel::syscall::{CommandReturn, SyscallDriver};
    use kernel::{ErrorCode, ProcessId};

    // `Button<'a, P>` just needs *some* `P: InterruptPin<'a>`; it never
    // calls methods on `P` itself (only on the `InterruptValueWrapper` that
    // wraps it). `Configure`/`Input`/`InterruptWithValue` now live in the
    // `kernel` crate, so the orphan rule means we need a locally-defined
    // type to implement them on -- `()` would be foreign-on-foreign.
    struct DummyPin;
    impl Configure for DummyPin {
        fn make_input(&self) {}
        fn set_floating_state(&self, _state: FloatingState) {}
    }
    impl Input for DummyPin {
        fn read_activation(&self, _mode: ActivationMode) -> ActivationState {
            ActivationState::Inactive
        }
    }
    impl InterruptWithValue for DummyPin {
        fn set_value(&self, _value: u32) {}
        fn enable_interrupts(&self, _edge: InterruptEdge) -> Result<(), ErrorCode> {
            Ok(())
        }
        fn disable_interrupts(&self) {}
    }

    fn with_button<R>(f: impl FnOnce(&Button<'_, DummyPin>) -> R) -> R {
        let wrapper = InterruptValueWrapper::new();
        let pins = [(&wrapper, ActivationMode::ActiveHigh, FloatingState::PullNone)];
        let grant = Grant::new(App::default());
        let button = Button::new(&pins, grant);
        f(&button)
    }

    #[test]
    fn num_reports_pin_count() {
        with_button(|b| {
            assert!(matches!(
                b.command(0, 0, 0, ProcessId),
                CommandReturn::SuccessU32(1)
            ));
        });
    }

    #[test]
    fn read_is_inactive_by_default() {
        with_button(|b| {
            assert!(matches!(
                b.command(3, 0, 0, ProcessId),
                CommandReturn::SuccessU32(0)
            ));
        });
    }

    #[test]
    fn read_of_an_out_of_range_pin_is_inval() {
        with_button(|b| {
            assert!(matches!(
                b.command(3, 99, 0, ProcessId),
                CommandReturn::Failure(ErrorCode::INVAL)
            ));
        });
    }

    #[test]
    fn enable_then_disable_interrupt_succeed() {
        with_button(|b| {
            assert!(b.command(1, 0, 0, ProcessId).is_success());
            assert!(b.command(2, 0, 0, ProcessId).is_success());
        });
    }

    #[test]
    fn allocate_grant_was_generated() {
        with_button(|b| {
            assert!(b.allocate_grant(ProcessId).is_ok());
        });
    }

    #[test]
    fn upcall_const_was_generated() {
        assert_eq!(UPCALL_TOGGLED, 0);
    }
}
