//! Test handling of child processes exiting various ways.
use assert2::check;
use bstr::ByteSlice;
use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;
use std::os::unix::process::ExitStatusExt;
use std::time::{Duration, Instant};

mod helpers;

// child.id() returns u32; nix expects i32.
fn to_pid(id: u32) -> Pid {
    Pid::from_raw(id.try_into().unwrap())
}

#[test]
fn child_success() {
    let output = helpers::rederr(["true"]).output().unwrap();

    check!(output.status.success());
    check!(output.stdout.as_bstr() == "");
    check!(output.stderr.as_bstr() == "");
}

#[test]
fn child_failure() {
    let output = helpers::rederr(["false"]).output().unwrap();

    check!(output.status.code() == Some(1));
    check!(output.stdout.as_bstr() == "");
    check!(output.stderr.as_bstr() == "");
}

#[test]
fn child_sigterm() {
    let start = Instant::now();
    let child = helpers::rederr(["sleep", "60"]).spawn().unwrap();
    kill(to_pid(child.id()), Signal::SIGTERM).unwrap();
    let output = child.wait_with_output().unwrap();

    check!(output.status.signal() == Some(15), "Expected SIGTERM (15)");
    check!(output.stdout.as_bstr() == "");
    check!(output.stderr.as_bstr() == "");
    check!(start.elapsed() < Duration::from_secs(1));
}
