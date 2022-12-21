use assert_cmd::prelude::*;
use std::ffi::OsStr;
use std::process::Command;

pub fn rederr<I, S>(args: I) -> Command
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::cargo_bin("rederr").unwrap();
    command.args(args);
    command
}
