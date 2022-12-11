use anyhow::anyhow;
use clap::Parser;
use popol::set_nonblocking;
use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::os::unix::process::ExitStatusExt;
use std::process;
use std::time::Duration;
use termcolor::{Color, ColorChoice, ColorSpec, StandardStream, WriteColor};

#[derive(Debug, Parser)]
#[clap(version, about)]
pub(crate) struct Params {
    /// The executable to run
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
        value_name = "DURATION",
        value_parser = parse_idle_timeout,
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

/// Maximum timeout that poll allows.
const POLL_MAX_TIMEOUT: Duration = Duration::from_millis(i32::MAX as u64);

fn parse_duration(input: &str) -> anyhow::Result<Duration> {
    let input = input.trim();

    if input.starts_with('-') {
        Err(anyhow!("duration cannot be negative"))
    } else if input.chars().all(|c| c.is_ascii_digit()) {
        // Input is all numbers, so assume it’s seconds.
        input
            .parse::<u64>()
            .map(Duration::from_secs)
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
    if duration > POLL_MAX_TIMEOUT {
        Err(anyhow!(
            "duration cannot be larger than {:?}",
            POLL_MAX_TIMEOUT
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
    let mut events = Vec::with_capacity(2);

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
        // FIXME? handle EINTR? I don’t think it will come up unless we have a
        // signal handler set.
        sources
            .poll(&mut events, params.idle_timeout)
            .unwrap_or_else(|err| {
                if err.kind() == io::ErrorKind::TimedOut {
                    if let Some(timeout) = params.idle_timeout {
                        fail!(
                            "Timed out waiting for input after {:?}",
                            timeout
                        );
                    }
                }

                fail!("Error while waiting for input: {:#}", err);
            });

        for event in events.drain(..) {
            if params.debug {
                println!("{:?}", event);
            }

            if event.is_readable() {
                loop {
                    let result = if event.key == PollKey::Out {
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
                        if event.key == PollKey::Out {
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

            if event.is_hangup() {
                // Remove the stream from poll.
                sources.unregister(&event.key);
            }
        }
    }

    let status = child.wait().expect("failed to wait on child");
    process::exit(
        wait_status_to_code(status).expect("no exit code or signal for child"),
    );
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

/// Get the actual exit code from a finished child process
fn wait_status_to_code(status: process::ExitStatus) -> Option<i32> {
    status.code().or_else(|| Some(128 + status.signal()?))
}

#[cfg(test)]
mod tests {
    use crate::*;
    use assert2::{check, let_assert};
    use clap::error::{
        ContextKind::InvalidArg, ContextValue::String, ErrorKind,
    };

    #[test]
    fn args_invalid_long_option() {
        let_assert!(
            Err(error) =
                Params::try_parse_from(["redder", "--foo", "-s", "command"])
        );
        check!(error.kind() == ErrorKind::UnknownArgument);
        check!(error.get(InvalidArg) == Some(&String("--foo".into())));
    }

    #[test]
    fn args_invalid_short_option() {
        let_assert!(
            Err(error) =
                Params::try_parse_from(["redder", "-X", "-s", "command"])
        );
        check!(error.kind() == ErrorKind::UnknownArgument);
        check!(error.get(InvalidArg) == Some(&String("-X".into())));
    }

    #[test]
    fn args_other_long_option_after_command() {
        let_assert!(
            Ok(params) = Params::try_parse_from([
                "redder",
                "--always-color",
                "command",
                "--foo",
            ])
        );
        check!(params.command == "command");
        check!(params.args == ["--foo"]);
        check!(params.always_color == true);
        check!(params.separate == false);
    }

    #[test]
    fn args_other_short_option_after_command() {
        let_assert!(
            Ok(params) = Params::try_parse_from([
                "redder",
                "--always-color",
                "command",
                "-f",
            ])
        );
        check!(params.command == "command");
        check!(params.args == ["-f"]);
        check!(params.always_color == true);
        check!(params.separate == false);
    }

    #[test]
    fn args_other_mixed_option_after_command() {
        let_assert!(
            Ok(params) = Params::try_parse_from([
                "redder",
                "--always-color",
                "command",
                "-f",
                "--foo",
            ])
        );
        check!(params.command == "command");
        check!(params.args == ["-f", "--foo"]);
        check!(params.always_color == true);
        check!(params.separate == false);
    }

    #[test]
    #[ignore] // FIXME clap doesn’t stop parsing after first non-flag.
    fn args_our_long_option_after_command() {
        let_assert!(
            Ok(params) = Params::try_parse_from([
                "redder",
                "--always-color",
                "command",
                "--separate",
            ])
        );
        check!(params.command == "command");
        check!(params.args == ["--separate"]);
        check!(params.always_color == true);
        check!(params.separate == false);
    }

    #[test]
    #[ignore] // FIXME clap doesn’t stop parsing after first non-flag.
    fn args_our_same_long_option_after_command() {
        let_assert!(
            Ok(params) = Params::try_parse_from([
                "redder",
                "--separate",
                "command",
                "--separate",
            ])
        );
        check!(params.command == "command");
        check!(params.args == ["-s"]);
        check!(params.always_color == false);
        check!(params.separate == true);
    }

    #[test]
    #[ignore] // FIXME clap doesn’t stop parsing after first non-flag.
    fn args_our_short_option_after_command() {
        let_assert!(
            Ok(params) =
                Params::try_parse_from(["redder", "-c", "command", "-s"])
        );
        check!(params.command == "command");
        check!(params.args == ["-s"]);
        check!(params.always_color == true);
        check!(params.separate == false);
    }

    #[test]
    #[ignore] // FIXME clap doesn’t stop parsing after first non-flag.
    fn args_our_same_short_option_after_command() {
        let_assert!(
            Ok(params) =
                Params::try_parse_from(["redder", "-s", "command", "-s"])
        );
        check!(params.command == "command");
        check!(params.args == ["-s"]);
        check!(params.always_color == false);
        check!(params.separate == true);
    }

    #[test]
    fn args_command_with_args() {
        let_assert!(
            Ok(params) = Params::try_parse_from([
                "redder", "-s", "command", "-abc", "foo", "--", "-s", "--bar",
            ])
        );
        check!(params.command == "command");
        check!(params.args == ["-abc", "foo", "--", "-s", "--bar"]);
        check!(params.always_color == false);
        check!(params.separate == true);
    }

    #[test]
    fn args_buffer_size_negative() {
        let_assert!(
            Err(error) = Params::try_parse_from([
                "redder",
                "--buffer-size",
                "-2",
                "command",
            ])
        );
        check!(error.kind() == ErrorKind::ValueValidation);
    }

    #[test]
    fn args_idle_timeout_2() {
        let_assert!(
            Ok(params) = Params::try_parse_from([
                "redder",
                "--idle-timeout",
                "2",
                "command",
            ])
        );
        check!(params.idle_timeout == Some(Duration::from_secs(2)));
    }

    #[test]
    fn args_idle_timeout_2s() {
        let_assert!(
            Ok(params) = Params::try_parse_from([
                "redder",
                "--idle-timeout",
                "2s",
                "command",
            ])
        );
        check!(params.idle_timeout == Some(Duration::from_secs(2)));
    }

    #[test]
    fn args_idle_timeout_2s_1ms() {
        let_assert!(
            Ok(params) = Params::try_parse_from([
                "redder",
                "--idle-timeout",
                "2s 1ms",
                "command",
            ])
        );
        check!(params.idle_timeout == Some(Duration::from_millis(2001)));
    }

    #[test]
    fn args_idle_timeout_2h() {
        let_assert!(
            Ok(params) = Params::try_parse_from([
                "redder",
                "--idle-timeout",
                "2h",
                "command",
            ])
        );
        check!(params.idle_timeout == Some(Duration::from_secs(2 * 60 * 60)));
    }

    #[test]
    fn args_idle_timeout_negative() {
        let_assert!(
            Err(error) = Params::try_parse_from([
                "redder",
                "--idle-timeout",
                "-2s",
                "command",
            ])
        );
        check!(error.kind() == ErrorKind::ValueValidation);
        check!(error.to_string().contains("negative"));
    }

    #[test]
    fn args_idle_timeout_zero() {
        let_assert!(
            Ok(params) = Params::try_parse_from([
                "redder",
                "--idle-timeout",
                "0",
                "command",
            ])
        );
        check!(params.idle_timeout == Some(Duration::ZERO));
    }

    #[test]
    fn args_idle_timeout_maximum() {
        let_assert!(
            Ok(params) = Params::try_parse_from([
                "redder",
                "--idle-timeout",
                &format!("{}ms", i32::MAX),
                "command",
            ])
        );
        check!(
            params.idle_timeout == Some(Duration::from_millis(i32::MAX as u64))
        );
    }

    #[test]
    fn args_idle_timeout_too_large() {
        let_assert!(
            Err(error) = Params::try_parse_from([
                "redder",
                "--idle-timeout",
                &format!("{}", i32::MAX as u64 + 1),
                "command",
            ])
        );
        check!(error.kind() == ErrorKind::ValueValidation);
        check!(error.to_string().contains("cannot be larger"));
    }

    #[test]
    fn args_idle_timeout_too_large_days() {
        let_assert!(
            Err(error) = Params::try_parse_from([
                "redder",
                "--idle-timeout",
                "26day",
                "command",
            ])
        );
        check!(error.kind() == ErrorKind::ValueValidation);
        check!(error.to_string().contains("cannot be larger"));
    }

    #[test]
    fn args_idle_timeout_overly_precise() {
        let_assert!(
            Err(error) = Params::try_parse_from([
                "redder",
                "--idle-timeout",
                "2s 2ms 2ns",
                "command",
            ])
        );
        check!(error.kind() == ErrorKind::ValueValidation);
        check!(error.to_string().contains("milliseconds"));
    }
}
