use anyhow::anyhow;
use clap::Parser;
use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::os::unix::prelude::AsRawFd;
use std::os::unix::process::ExitStatusExt;
use std::process;
use std::time::Duration;
use termcolor::{Color, ColorChoice, ColorSpec, StandardStream, WriteColor};

#[derive(Debug, Parser)]
#[clap(version, about)]
pub(crate) struct Params {
    /// The executable to run
    #[clap()]
    command: OsString,

    /// Arguments to pass to the executable
    #[clap(allow_hyphen_values = true)]
    args: Vec<OsString>,

    /// Always output in color
    #[clap(long, short = 'c')]
    always_color: bool,

    /// Timeout on individual reads (e.g. "1s", "1h", or "30ms")
    #[clap(
        long,
        name = "duration",
        parse(try_from_str = parse_idle_timeout),
        allow_hyphen_values = true,
    )]
    idle_timeout: Option<Duration>,

    /// Don't combine stderr into stdout; keep them separate
    #[clap(long, short)]
    separate: bool,

    // Hidden: output debugging information rather than coloring stderr
    #[clap(long, hide = true)]
    debug: bool,

    /// Hidden: how large a buffer to use
    #[clap(
        long,
        default_value_t = 1024,
        hide = true,
        allow_hyphen_values = true
    )]
    buffer_size: usize,
}

fn parse_duration(input: &str) -> anyhow::Result<Duration> {
    let input = input.trim();

    if input.starts_with('-') {
        Err(anyhow!("duration cannot be negative"))
    } else if input.chars().all(|c| c.is_ascii_digit()) {
        // Input is all numbers, so assume it’s seconds.
        input
            .parse::<u64>()
            .map(|seconds| Duration::from_secs(seconds))
            .map_err(|e| e.into())
    } else {
        let duration = duration_str::parse(input)?;
        if duration.subsec_nanos() == duration.subsec_millis() * 1_000_000 {
            Ok(duration)
        } else {
            Err(anyhow!("duration cannot be more precise than milliseconds"))
        }
    }
}

fn parse_idle_timeout(input: &str) -> anyhow::Result<Duration> {
    let duration = parse_duration(input)?;
    if duration > Duration::from_millis(i32::MAX as u64) {
        Err(anyhow!(
            "duration cannot be larger than {} milliseconds",
            i32::MAX
        ))
    } else {
        Ok(duration)
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
enum PollKey {
    Out,
    Err,
}

macro_rules! fail {
    ($($arg:tt)*) => {
        eprintln!($($arg)*);
        process::exit(1);
    };
}

fn main() {
    if let Err(error) = cli(Params::parse()) {
        fail!("Error: {:#}", error);
    }
}

fn cli(params: Params) -> anyhow::Result<()> {
    let mut child = process::Command::new(&params.command)
        .args(&params.args)
        .stdout(process::Stdio::piped())
        .stderr(process::Stdio::piped())
        .spawn()
        .unwrap_or_else(|err| {
            fail!("Could not run command {:?}: {}", params.command, err);
        });

    let mut sources = popol::Sources::with_capacity(2);
    let mut events = popol::Events::new();

    let mut child_out = child.stdout.take().expect("child.stdout is None");
    set_nonblocking(&child_out, true)
        .expect("child stdout cannot be set to non-blocking");
    sources.register(PollKey::Out, &child_out, popol::interest::READ);

    let mut child_err = child.stderr.take().expect("child.stderr is None");
    set_nonblocking(&child_err, true)
        .expect("child stderr cannot be set to non-blocking");
    sources.register(PollKey::Err, &child_err, popol::interest::READ);

    let mut out_out = color_stream(atty::Stream::Stdout, &params);
    let mut out_err = if params.separate {
        color_stream(atty::Stream::Stderr, &params)
    } else {
        color_stream(atty::Stream::Stdout, &params)
    };

    let mut err_color = ColorSpec::new();
    err_color.set_fg(Some(Color::Red));
    err_color.set_intense(true);

    let mut buffer = vec![0; params.buffer_size];

    // FIXME? this sometimes messes up the order if stderr and stdout are used
    // in the same line. Not sure this is possible to fix.
    while !sources.is_empty() {
        wait_on(&mut sources, &mut events, params.idle_timeout);

        for (key, event) in events.iter() {
            if params.debug {
                println!("{:?} {:?}", key, event);
            }

            if event.readable {
                loop {
                    let result = if *key == PollKey::Out {
                        child_out.read(&mut buffer)
                    } else {
                        child_err.read(&mut buffer)
                    };

                    let count = match result {
                        Ok(count) => count,
                        Err(err) => {
                            if err.kind() == io::ErrorKind::WouldBlock {
                                // Done reading.
                                if params.debug {
                                    println!("io::ErrorKind::WouldBlock");
                                }

                                break;
                            } else {
                                return Err(err.into());
                            }
                        }
                    };

                    if params.debug {
                        // FIXME don’t require UTF-8
                        println!(
                            "read {} bytes {:?}",
                            count,
                            std::str::from_utf8(&buffer[..count]).unwrap()
                        );
                    } else if count > 0 {
                        // Only output if there’s something to output.
                        if *key == PollKey::Out {
                            out_out.write_all(&buffer[..count])?;
                            out_out.flush()?; // If there wasn’t a newline.
                        } else {
                            out_err.set_color(&err_color)?;
                            out_err.write_all(&buffer[..count])?;
                            out_err.reset()?;
                            out_err.flush()?; // If there wasn’t a newline.
                        }
                    }

                    if count < buffer.len() {
                        // We could read again and get either 0 bytes or
                        // io::ErrorKind::WouldBlock, but I think this check
                        // makes it more likely the output ordering is correct.
                        // A partial read indicates that the stream had stopped,
                        // so we should check to see if another stream is ready.
                        break;
                    }
                }
            }

            if event.hangup {
                // Remove the stream from poll.
                sources.unregister(key);
            }
        }
    }

    let status = child.wait().expect("failed to wait on child");
    process::exit(
        wait_status_to_code(status).expect("no exit code or signal for child"),
    );
}

/// Set a stream to be non-blocking
pub fn set_nonblocking(fd: &dyn AsRawFd, nonblocking: bool) -> io::Result<i32> {
    let fd = fd.as_raw_fd();

    // SAFETY: required for FFI; shouldn’t break rust guarantees.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags == -1 {
        return Err(io::Error::last_os_error());
    }

    let flags = if nonblocking {
        flags | libc::O_NONBLOCK
    } else {
        flags & !libc::O_NONBLOCK
    };

    // SAFETY: required for FFI; shouldn’t break rust guarantees.
    match unsafe { libc::fcntl(fd, libc::F_SETFL, flags) } {
        -1 => Err(io::Error::last_os_error()),
        result => Ok(result),
    }
}

fn color_stream(stream: atty::Stream, params: &Params) -> StandardStream {
    let choice = if params.always_color {
        ColorChoice::Always
    } else if atty::is(stream) {
        ColorChoice::Auto
    } else {
        ColorChoice::Never
    };

    match stream {
        atty::Stream::Stdout => StandardStream::stdout(choice),
        atty::Stream::Stderr => StandardStream::stderr(choice),
        atty::Stream::Stdin => panic!("can't output to stdin"),
    }
}

fn wait_on(
    sources: &mut popol::Sources<PollKey>,
    events: &mut popol::Events<PollKey>,
    timeout: Option<Duration>,
) {
    // FIXME? handle EINTR? I don’t think it will come up unless we have a
    // signal handler set.
    match timeout {
        Some(timeout) => sources.wait_timeout(events, timeout),
        None => sources.wait(events),
    }
    .unwrap_or_else(|err| {
        if err.kind() == io::ErrorKind::TimedOut {
            if let Some(timeout) = timeout {
                fail!("Timed out waiting for input after {:?}", timeout);
            }
        }

        fail!("Error while waiting for input: {:#}", err);
    });
}

/// Get the actual exit code from a finished child process
fn wait_status_to_code(status: process::ExitStatus) -> Option<i32> {
    status.code().or_else(|| Some(128 + status.signal()?))
}

#[cfg(test)]
mod tests {
    use crate::*;
    use assertify::assertify;

    #[test]
    fn args_invalid_long_option() {
        let parse =
            Params::try_parse_from(["redder", "--foo", "-s", "command"]);
        assertify!(parse.is_err());
        let error = parse.unwrap_err();
        assertify!(error.kind() == clap::ErrorKind::UnknownArgument);
        assertify!(error.info == ["--foo"]);
    }

    #[test]
    fn args_invalid_short_option() {
        let parse = Params::try_parse_from(["redder", "-X", "-s", "command"]);
        assertify!(parse.is_err());
        let error = parse.unwrap_err();
        assertify!(error.kind() == clap::ErrorKind::UnknownArgument);
        assertify!(error.info == ["-X"]);
    }

    #[test]
    #[ignore] // FIXME broken by clap bug
    fn args_other_long_option_after_command() {
        let params = Params::try_parse_from([
            "redder",
            "--always-color",
            "command",
            "--foo",
        ])
        .unwrap();
        assertify!(params.command == "command");
        assertify!(params.args == ["--foo"]);
        assertify!(params.always_color == true);
        assertify!(params.separate == false);
    }

    #[test]
    fn args_other_short_option_after_command() {
        let params = Params::try_parse_from([
            "redder",
            "--always-color",
            "command",
            "-f",
        ])
        .unwrap();
        assertify!(params.command == "command");
        assertify!(params.args == ["-f"]);
        assertify!(params.always_color == true);
        assertify!(params.separate == false);
    }

    #[test]
    fn args_other_mixed_option_after_command() {
        let params = Params::try_parse_from([
            "redder",
            "--always-color",
            "command",
            "-f",
            "--foo",
        ])
        .unwrap();
        assertify!(params.command == "command");
        assertify!(params.args == ["-f", "--foo"]);
        assertify!(params.always_color == true);
        assertify!(params.separate == false);
    }

    #[test]
    #[ignore] // FIXME broken by clap bug
    fn args_our_long_option_after_command() {
        let params = Params::try_parse_from([
            "redder",
            "--always-color",
            "command",
            "--separate",
        ])
        .unwrap();
        assertify!(params.command == "command");
        assertify!(params.args == ["--separate"]);
        assertify!(params.always_color == true);
        assertify!(params.separate == false);
    }

    #[test]
    #[ignore] // FIXME broken by clap bug
    fn args_our_same_long_option_after_command() {
        let params = Params::try_parse_from([
            "redder",
            "--separate",
            "command",
            "--separate",
        ])
        .unwrap();
        assertify!(params.command == "command");
        assertify!(params.args == ["-s"]);
        assertify!(params.always_color == false);
        assertify!(params.separate == true);
    }

    #[test]
    fn args_our_short_option_after_command() {
        let params =
            Params::try_parse_from(["redder", "-c", "command", "-s"]).unwrap();
        assertify!(params.command == "command");
        assertify!(params.args == ["-s"]);
        assertify!(params.always_color == true);
        assertify!(params.separate == false);
    }

    #[test]
    fn args_our_same_short_option_after_command() {
        let params =
            Params::try_parse_from(["redder", "-s", "command", "-s"]).unwrap();
        assertify!(params.command == "command");
        assertify!(params.args == ["-s"]);
        assertify!(params.always_color == false);
        assertify!(params.separate == true);
    }

    #[test]
    fn args_command_with_args() {
        let params = Params::try_parse_from([
            "redder", "-s", "command", "-s", "-abc", "foo", "--", "--bar",
        ])
        .unwrap();
        assertify!(params.command == "command");
        assertify!(params.args == ["-s", "-abc", "foo", "--", "--bar"]);
        assertify!(params.always_color == false);
        assertify!(params.separate == true);
    }

    #[test]
    fn args_buffer_size_negative() {
        let parse = Params::try_parse_from([
            "redder",
            "--buffer-size",
            "-2",
            "command",
        ]);
        let error = parse.expect_err("expected parse to fail");
        assertify!(error.kind() == clap::ErrorKind::ValueValidation);
    }

    #[test]
    fn args_idle_timeout_2() {
        let params = Params::try_parse_from([
            "redder",
            "--idle-timeout",
            "2",
            "command",
        ])
        .unwrap();
        assertify!(params.idle_timeout == Some(Duration::from_secs(2)));
    }

    #[test]
    fn args_idle_timeout_2s() {
        let params = Params::try_parse_from([
            "redder",
            "--idle-timeout",
            "2s",
            "command",
        ])
        .unwrap();
        assertify!(params.idle_timeout == Some(Duration::from_secs(2)));
    }

    #[test]
    fn args_idle_timeout_2s_1ms() {
        let params = Params::try_parse_from([
            "redder",
            "--idle-timeout",
            "2s 1ms",
            "command",
        ])
        .unwrap();
        assertify!(params.idle_timeout == Some(Duration::from_millis(2001)));
    }

    #[test]
    fn args_idle_timeout_2h() {
        let params = Params::try_parse_from([
            "redder",
            "--idle-timeout",
            "2h",
            "command",
        ])
        .unwrap();
        assertify!(
            params.idle_timeout == Some(Duration::from_secs(2 * 60 * 60))
        );
    }

    #[test]
    fn args_idle_timeout_negative() {
        let parse = Params::try_parse_from([
            "redder",
            "--idle-timeout",
            "-2s",
            "command",
        ]);
        let error = parse.expect_err("expected parse to fail");
        assertify!(error.kind() == clap::ErrorKind::ValueValidation);
        assertify!(error.to_string().contains("negative"));
    }

    #[test]
    fn args_idle_timeout_zero() {
        let params = Params::try_parse_from([
            "redder",
            "--idle-timeout",
            "0",
            "command",
        ])
        .unwrap();
        assertify!(params.idle_timeout == Some(Duration::ZERO));
    }

    #[test]
    fn args_idle_timeout_maximum() {
        let params = Params::try_parse_from([
            "redder",
            "--idle-timeout",
            &format!("{}ms", i32::MAX),
            "command",
        ])
        .unwrap();
        assertify!(
            params.idle_timeout == Some(Duration::from_millis(i32::MAX as u64))
        );
    }

    #[test]
    fn args_idle_timeout_too_large() {
        let parse = Params::try_parse_from([
            "redder",
            "--idle-timeout",
            &format!("{}", i32::MAX as u64 + 1),
            "command",
        ]);
        let error = parse.expect_err("expected parse to fail");
        assertify!(error.kind() == clap::ErrorKind::ValueValidation);
        assertify!(error.to_string().contains("cannot be larger"));
    }

    #[test]
    fn args_idle_timeout_too_large_days() {
        let parse = Params::try_parse_from([
            "redder",
            "--idle-timeout",
            "26day",
            "command",
        ]);
        let error = parse.expect_err("expected parse to fail");
        assertify!(error.kind() == clap::ErrorKind::ValueValidation);
        assertify!(error.to_string().contains("cannot be larger"));
    }

    #[test]
    fn args_idle_timeout_overly_precise() {
        let parse = Params::try_parse_from([
            "redder",
            "--idle-timeout",
            "2s 2ms 2ns",
            "command",
        ]);
        let error = parse.expect_err("expected parse to fail");
        assertify!(error.kind() == clap::ErrorKind::ValueValidation);
        assertify!(error.to_string().contains("milliseconds"));
    }
}
