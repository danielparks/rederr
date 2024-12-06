//! General tests.
use assert2::check;
use bstr::{ByteSlice, B};
use std::time::{Duration, Instant};

mod helpers;

#[test]
fn simple_separate() {
    // This seems to work without --separate, but I don’t think we can rely on
    // the order of the lines without a sleep.
    let output = helpers::rederr(["--separate", "tests/fixtures/simple.sh"])
        .output()
        .unwrap();

    check!(output.status.success());
    check!(output.stdout.as_bstr() == "out\n");
    check!(output.stderr.as_bstr() == "err\n");
}

#[test]
fn simple_separate_long_idle_timeout() {
    // The maximum timeout for `poll()` is around 27 days.
    let start = Instant::now();
    let output = helpers::rederr([
        "--separate",
        "--idle-timeout",
        "2y",
        "tests/fixtures/simple.sh",
    ])
    .output()
    .unwrap();

    check!(output.status.success());
    check!(output.stdout.as_bstr() == "out\n");
    check!(output.stderr.as_bstr() == "err\n");
    check!(start.elapsed() < Duration::from_millis(200));
}

#[test]
fn midline_sleep_all() {
    let output = helpers::rederr(["tests/fixtures/midline_sleep.sh"])
        .output()
        .unwrap();

    check!(output.status.success());
    check!(output.stdout.as_bstr() == "111222333\n");
    check!(output.stderr.as_bstr() == "");
}

#[test]
fn midline_sleep_idle_timeout() {
    let output = helpers::rederr([
        "--idle-timeout",
        "50ms",
        "tests/fixtures/midline_sleep.sh",
    ])
    .output()
    .unwrap();

    check!(!output.status.success());
    check!(output.stdout.as_bstr() == "111");
    check!(output.stderr[..28].as_bstr() == "Timed out waiting for input ");
}

#[test]
fn midline_sleep_run_timeout() {
    let start = Instant::now();
    let output = helpers::rederr([
        "--idle-timeout",
        "150ms",
        "--run-timeout",
        "150ms",
        "tests/fixtures/midline_sleep.sh",
    ])
    .output()
    .unwrap();

    check!(!output.status.success());
    check!(output.stdout.as_bstr() == "111222");
    check!(output.stderr[..14].as_bstr() == "Run timed out ");
    check!(start.elapsed() < Duration::from_millis(200));
}

#[test]
fn midline_sleep_unused_timeouts() {
    let start = Instant::now();
    let output = helpers::rederr([
        "--idle-timeout",
        "150ms",
        "--run-timeout",
        "500ms",
        "tests/fixtures/midline_sleep.sh",
    ])
    .output()
    .unwrap();

    check!(output.status.success());
    check!(output.stdout.as_bstr() == "111222333\n");
    check!(output.stderr.as_bstr() == "");
    check!(start.elapsed() > Duration::from_millis(200));
}

#[test]
fn mixed_output_no_color_combined() {
    let output = helpers::rederr(["tests/fixtures/mixed_output.sh"])
        .output()
        .unwrap();

    check!(output.status.success());
    check!(output.stdout.as_bstr() == "111aaa333\nbbb\n");
    check!(output.stderr.as_bstr() == "");
}

#[test]
fn mixed_output_no_color_separate() {
    let output = helpers::rederr(["-s", "tests/fixtures/mixed_output.sh"])
        .output()
        .unwrap();

    check!(output.status.success());
    check!(output.stdout.as_bstr() == "111333\n");
    check!(output.stderr.as_bstr() == "aaabbb\n");
}

#[test]
fn mixed_output_color_combined() {
    let output = helpers::rederr(["-c", "tests/fixtures/mixed_output.sh"])
        .output()
        .unwrap();

    check!(output.status.success());
    check!(output.stdout.as_bstr() ==
        "111\u{1b}[0m\u{1b}[38;5;9maaa\u{1b}[0m333\n\u{1b}[0m\u{1b}[38;5;9mbbb\n\u{1b}[0m");
    check!(output.stderr.as_bstr() == "");
}

#[test]
fn mixed_output_color_separate() {
    let output = helpers::rederr(["-cs", "tests/fixtures/mixed_output.sh"])
        .output()
        .unwrap();

    check!(output.status.success());
    check!(output.stdout.as_bstr() == "111333\n");
    check!(output.stderr.as_bstr() ==
        "\u{1b}[0m\u{1b}[38;5;9maaa\u{1b}[0m\u{1b}[0m\u{1b}[38;5;9mbbb\n\u{1b}[0m");
}

#[test]
fn invalid_utf8() {
    let output = helpers::rederr(["tests/fixtures/invalid_utf8.sh"])
        .output()
        .unwrap();

    check!(output.status.success());
    check!(output.stdout.as_bstr() == B(b"bad \xE2(\xA1 bad\n"));
    check!(output.stderr.as_bstr() == "");
}

#[test]
fn invalid_utf8_debug() {
    let output = helpers::rederr(["--debug", "tests/fixtures/invalid_utf8.sh"])
        .output()
        .unwrap();

    check!(output.status.success());
    check!(output.stdout.contains_str("\"bad \\xe2(\\xa1 bad\\n\""));
    check!(output.stderr.as_bstr() == "");
}
