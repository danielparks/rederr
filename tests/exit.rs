//! Test handling of child processes exiting various ways.
use assert2::check;
use assert_cmd::prelude::*;
use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;
use std::os::unix::process::ExitStatusExt;
use std::process::Command;
use std::time::{Duration, Instant};

// child.id() returns u32; nix expects i32.
fn to_pid(id: u32) -> Pid {
    Pid::from_raw(id.try_into().unwrap())
}

#[test]
fn child_success() {
    let mut command = Command::cargo_bin("rederr").unwrap();
    let output = command.args(["true"]).output().unwrap();

    check!(output.status.success());
    check!(output.stdout.is_empty());
    check!(output.stderr.is_empty());
}

#[test]
fn child_failure() {
    let mut command = Command::cargo_bin("rederr").unwrap();
    let output = command.args(["false"]).output().unwrap();

    check!(output.status.code() == Some(1));
    check!(output.stdout.is_empty());
    check!(output.stderr.is_empty());
}

#[test]
fn child_sigterm() {
    let start = Instant::now();
    let mut command = Command::cargo_bin("rederr").unwrap();
    let child = command.args(["sleep", "60"]).spawn().unwrap();
    kill(to_pid(child.id()), Signal::SIGTERM).unwrap();
    let output = child.wait_with_output().unwrap();

    check!(output.status.signal() == Some(15), "Expected SIGTERM (15)");
    check!(output.stdout.is_empty());
    check!(output.stderr.is_empty());
    check!(start.elapsed() < Duration::from_secs(1));
}
