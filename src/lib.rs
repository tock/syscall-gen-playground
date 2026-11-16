#![allow(unused, reason = "Just for experimentation")]

use macros::syscall;

trait SyscallDriver {}
struct Buttons {}

#[syscall]
impl SyscallDriver for Buttons {
}
