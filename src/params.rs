//! Manage parameters for `rederr`.

use anyhow::anyhow;
use clap::Parser;
use std::ffi::OsString;
use std::io::{self, IsTerminal};
use std::time::Duration;
use termcolor::{ColorChoice, StandardStream};

/// Parameters for `rederr`.
#[derive(Debug, Parser)]
#[clap(version, about)]
pub struct Params {
    /// The executable to run
    pub command: OsString,

    /// Arguments to pass to the executable
    #[clap(allow_hyphen_values = true)]
    pub args: Vec<OsString>,

    /// Always output in color
    #[clap(long, short = 'c')]
    pub always_color: bool,

    /// Timeout for entire run (e.g. "1s", "1h", or "30ms")
    #[clap(
        long,
        value_name = "DURATION",
        value_parser = parse_duration,
        allow_hyphen_values = true,
    )]
    pub run_timeout: Option<Duration>,

    /// Timeout for individual reads (e.g. "1s", "1h", or "30ms")
    #[clap(
        long,
        value_name = "DURATION",
        value_parser = parse_duration,
        allow_hyphen_values = true,
    )]
    pub idle_timeout: Option<Duration>,

    /// Don't combine stderr into stdout; keep them separate
    #[clap(long, short)]
    pub separate: bool,

    /// Hidden: output debugging information rather than coloring stderr
    #[clap(long, hide = true)]
    pub debug: bool,

    /// Hidden: how large a buffer to use
    #[clap(
        long,
        default_value_t = 1024,
        hide = true,
        allow_hyphen_values = true
    )]
    pub buffer_size: usize,
}

impl Params {
    /// Get the output stream for the child’s stdout.
    pub fn out_stream(&self) -> StandardStream {
        StandardStream::stdout(if self.always_color {
            ColorChoice::Always
        } else if io::stdout().is_terminal() {
            ColorChoice::Auto
        } else {
            ColorChoice::Never
        })
    }

    /// Get the output stream for the child’s stderr.
    pub fn err_stream(&self) -> StandardStream {
        if self.separate {
            StandardStream::stderr(if self.always_color {
                ColorChoice::Always
            } else if io::stderr().is_terminal() {
                ColorChoice::Auto
            } else {
                ColorChoice::Never
            })
        } else {
            self.out_stream()
        }
    }
}

/// Parse a duration parameter.
///
/// ```rust
/// assert_eq!(
///     parse_duration("5s 500ms").unwrap(),
///     Duration::from_millis(5_500),
/// );
/// ```
fn parse_duration(input: &str) -> anyhow::Result<Duration> {
    let input = input.trim();

    if input.starts_with('-') {
        Err(anyhow!("duration cannot be negative"))
    } else if input.chars().all(|c| c.is_ascii_digit()) {
        // Input is all numbers, so assume it’s seconds.
        input
            .parse::<u64>()
            .map(Duration::from_secs)
            .map_err(Into::into)
    } else {
        let duration = duration_str::parse(input).map_err(|s| anyhow!(s))?;
        // subsec_millis() will always return a value < 1000.
        #[allow(clippy::arithmetic_side_effects)]
        if duration.subsec_nanos() == duration.subsec_millis() * 1_000_000 {
            Ok(duration)
        } else {
            Err(anyhow!("duration cannot be more precise than milliseconds"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert2::{check, let_assert};
    use clap::error::{
        ContextKind::InvalidArg, ContextValue::String, ErrorKind,
    };
    use std::time::Duration;

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
