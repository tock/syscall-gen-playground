//! A tiny stand-in for the real Tock `kernel` crate: just enough of
//! `kernel::{syscall, grant, hil::gpio, ProcessId, ErrorCode, process}` for
//! `syscall_drivers/button.rs` (and the `capsule` crate's own smoke test) to
//! compile and run outside of a real Tock tree, so the `macros` crate's
//! generated code -- which refers to these by the same paths a real Tock
//! capsule would -- can be exercised end to end.
#![allow(unused, reason = "mock surface only implements what's exercised")]

pub struct ProcessId;

pub mod process {
    #[derive(Debug)]
    pub struct Error;
}

#[derive(Debug, Clone, Copy)]
#[allow(non_camel_case_types, reason = "matches real kernel::ErrorCode spelling")]
pub enum ErrorCode {
    INVAL,
    NOSUPPORT,
    FAIL,
}

impl From<process::Error> for ErrorCode {
    fn from(_: process::Error) -> Self {
        ErrorCode::FAIL
    }
}

pub mod syscall {
    use super::ErrorCode;

    #[derive(Debug, Clone, Copy)]
    pub enum CommandReturn {
        Success,
        SuccessU32(u32),
        Failure(ErrorCode),
    }

    impl CommandReturn {
        pub fn success() -> Self {
            CommandReturn::Success
        }
        pub fn success_u32(v: u32) -> Self {
            CommandReturn::SuccessU32(v)
        }
        pub fn failure(e: ErrorCode) -> Self {
            CommandReturn::Failure(e)
        }

        pub fn is_success(&self) -> bool {
            matches!(self, CommandReturn::Success)
        }
        pub fn is_success_u32(&self) -> bool {
            matches!(self, CommandReturn::SuccessU32(_))
        }
        pub fn is_success_2_u32(&self) -> bool {
            false
        }
        pub fn is_success_3_u32(&self) -> bool {
            false
        }
        pub fn is_success_u64(&self) -> bool {
            false
        }
        pub fn is_success_u32_u64(&self) -> bool {
            false
        }

        pub fn is_failure(&self) -> bool {
            matches!(self, CommandReturn::Failure(_))
        }
        pub fn is_failure_u32(&self) -> bool {
            false
        }
        pub fn is_failure_2_u32(&self) -> bool {
            false
        }
        pub fn is_failure_u64(&self) -> bool {
            false
        }
    }

    pub trait SyscallDriver {
        fn command(
            &self,
            command_num: usize,
            data: usize,
            data2: usize,
            processid: super::ProcessId,
        ) -> CommandReturn;

        fn allocate_grant(&self, _processid: super::ProcessId) -> Result<(), super::process::Error> {
            Ok(())
        }
    }
}

pub mod grant {
    use core::cell::RefCell;
    use core::marker::PhantomData;

    pub struct UpcallCount<const N: usize>;
    pub struct AllowRoCount<const N: usize>;
    pub struct AllowRwCount<const N: usize>;

    /// Stands in for the real per-app upcall handle passed into
    /// `Grant::enter`/`Grant::each`.
    pub struct Upcalls;

    impl Upcalls {
        pub fn schedule_upcall(
            &self,
            _which: usize,
            _args: (usize, usize, usize),
        ) -> Result<(), super::ErrorCode> {
            Ok(())
        }
    }

    pub struct Grant<A, U, R, W> {
        app: RefCell<A>,
        _upcalls: PhantomData<U>,
        _ro: PhantomData<R>,
        _rw: PhantomData<W>,
    }

    impl<A, U, R, W> Grant<A, U, R, W> {
        pub fn new(app: A) -> Self {
            Grant {
                app: RefCell::new(app),
                _upcalls: PhantomData,
                _ro: PhantomData,
                _rw: PhantomData,
            }
        }

        pub fn enter<F, T>(&self, _processid: super::ProcessId, f: F) -> Result<T, super::process::Error>
        where
            F: FnOnce(&mut A, &mut Upcalls) -> T,
        {
            Ok(f(&mut self.app.borrow_mut(), &mut Upcalls))
        }

        pub fn each<F>(&self, mut f: F)
        where
            F: FnMut(super::ProcessId, &mut A, &mut Upcalls),
        {
            f(super::ProcessId, &mut self.app.borrow_mut(), &mut Upcalls);
        }
    }
}

pub mod hil {
    pub mod gpio {
        use core::cell::Cell;
        use core::marker::PhantomData;

        #[derive(Clone, Copy)]
        pub enum ActivationMode {
            ActiveHigh,
            ActiveLow,
        }

        #[derive(Clone, Copy)]
        pub enum FloatingState {
            PullNone,
            PullUp,
            PullDown,
        }

        #[derive(Clone, Copy, PartialEq)]
        pub enum ActivationState {
            Inactive = 0,
            Active = 1,
        }

        #[derive(Clone, Copy)]
        pub enum InterruptEdge {
            RisingEdge,
            FallingEdge,
            EitherEdge,
        }

        pub trait Configure {
            fn make_input(&self);
            fn set_floating_state(&self, state: FloatingState);
        }

        pub trait Input {
            fn read_activation(&self, mode: ActivationMode) -> ActivationState;
        }

        pub trait InterruptWithValue {
            fn set_value(&self, value: u32);
            fn enable_interrupts(&self, edge: InterruptEdge) -> Result<(), super::super::ErrorCode>;
            fn disable_interrupts(&self);
        }

        pub trait InterruptPin<'a>: Configure + Input + InterruptWithValue {}
        impl<'a, T: Configure + Input + InterruptWithValue> InterruptPin<'a> for T {}

        pub trait ClientWithValue {
            fn fired(&self, pin_num: u32);
        }

        /// Stands in for the real `InterruptValueWrapper`: a pin plus its
        /// per-pin interrupt "value" (its index), with just enough state to
        /// exercise the capsule end to end.
        pub struct InterruptValueWrapper<'a, P> {
            value: Cell<u32>,
            activation: Cell<ActivationState>,
            enabled: Cell<bool>,
            _pin: PhantomData<&'a P>,
        }

        impl<'a, P> InterruptValueWrapper<'a, P> {
            pub fn new() -> Self {
                Self {
                    value: Cell::new(0),
                    activation: Cell::new(ActivationState::Inactive),
                    enabled: Cell::new(false),
                    _pin: PhantomData,
                }
            }

            pub fn set_activation(&self, state: ActivationState) {
                self.activation.set(state);
            }

            pub fn is_enabled(&self) -> bool {
                self.enabled.get()
            }
        }

        impl<'a, P> Configure for InterruptValueWrapper<'a, P> {
            fn make_input(&self) {}
            fn set_floating_state(&self, _state: FloatingState) {}
        }

        impl<'a, P> Input for InterruptValueWrapper<'a, P> {
            fn read_activation(&self, _mode: ActivationMode) -> ActivationState {
                self.activation.get()
            }
        }

        impl<'a, P> InterruptWithValue for InterruptValueWrapper<'a, P> {
            fn set_value(&self, value: u32) {
                self.value.set(value);
            }
            fn enable_interrupts(&self, _edge: InterruptEdge) -> Result<(), super::super::ErrorCode> {
                self.enabled.set(true);
                Ok(())
            }
            fn disable_interrupts(&self) {
                self.enabled.set(false);
            }
        }
    }
}
