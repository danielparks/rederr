use clap::{Args, FromArgMatches};
use std::ffi::OsString;
use std::io::{Read, Write};
use std::process;
use std::time::Duration;
use termcolor::{Color, ColorChoice, ColorSpec, StandardStream, WriteColor};

// FIXME UNIX only (is popol UNIX only?)
use std::os::unix::process::ExitStatusExt;

#[derive(Debug, Args)]
#[clap(version, about)]
struct Params {
    /// Timeout on individual reads (e.g. "1s", "1h", or "30ms")
    #[clap(long, name="duration", parse(try_from_str = duration_str::parse))]
    idle_timeout: Option<Duration>,
    /// Don't combine stderr into stdout; keep it separate
    #[clap(long, short)]
    separate: bool,
    /// Always output in color
    #[clap(long, short = 'c')]
    always_color: bool,
}

#[derive(Clone, PartialEq, Eq, Debug)]
enum PollKey {
    Out,
    Err,
}

fn main() {
    // Use the builder so that we can accepts options and flags as part of ARGS
    // without having to use --. For example: rederr tar -xf -
    let clap_command = clap::Command::new("command")
        .trailing_var_arg(true)
        .arg(
            clap::Arg::with_name("COMMAND")
                .help("The executable to run")
                .takes_value(true)
                .allow_invalid_utf8(true)
                .required(true),
        )
        .arg(
            clap::Arg::with_name("ARGS")
                .help("Arguments to pass to the executable")
                .takes_value(true)
                .allow_invalid_utf8(true)
                .multiple(true)
                .allow_hyphen_values(true),
        );
    let matches = Params::augment_args(clap_command).get_matches();

    let command: &OsString = matches.get_one::<OsString>("COMMAND").unwrap();
    let args: Vec<&OsString> = matches
        .get_many::<OsString>("ARGS")
        .map(|vals| vals.collect::<Vec<_>>())
        .unwrap_or_default();

    let params = Params::from_arg_matches(&matches)
        .map_err(|err| err.exit())
        .unwrap();

    if let Err(error) = cli(command, args, params) {
        eprintln!("Error: {:#}", error);
        process::exit(1);
    }
}

fn cli(
    command: &OsString,
    args: Vec<&OsString>,
    params: Params,
) -> anyhow::Result<()> {
    let mut child = process::Command::new(command)
        .args(args)
        .stdout(process::Stdio::piped())
        .stderr(process::Stdio::piped())
        .spawn()?;

    let mut buffer = [0; 1024]; // FIXME: best buffer size?
    let mut sources = popol::Sources::with_capacity(2);
    let mut events = popol::Events::new();

    let mut child_out = child.stdout.take().expect("child.stdout is None");
    sources.register(PollKey::Out, &child_out, popol::interest::READ);

    let mut child_err = child.stderr.take().expect("child.stderr is None");
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

    // FIXME this sometimes messes up the order if stderr and stdout are used
    // in the same line. Not sure this is possible to fix.
    'outer: loop {
        wait_on(&mut sources, &mut events, params.idle_timeout)?;

        for (key, event) in events.iter() {
            // FIXME does read ever return non-zero if event.hangup?
            if event.readable || event.hangup {
                loop {
                    let count = if *key == PollKey::Out {
                        child_out.read(&mut buffer)?
                    } else {
                        child_err.read(&mut buffer)?
                    };

                    if count == 0 {
                        // FIXME detect actual EOF, or SIGCHILD?
                        break 'outer;
                    }

                    if *key == PollKey::Out {
                        out_out.write_all(&buffer[..count])?;
                        out_out.flush()?; // If there wasn’t a newline.
                    } else {
                        out_err.set_color(&err_color)?;
                        out_err.write_all(&buffer[..count])?;
                        out_err.reset()?;
                        out_err.flush()?; // If there wasn’t a newline.
                    }

                    if count < buffer.len() {
                        break;
                    }
                }
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

fn wait_on(
    sources: &mut popol::Sources<PollKey>,
    events: &mut popol::Events<PollKey>,
    timeout: Option<Duration>,
) -> anyhow::Result<()> {
    match timeout {
        Some(timeout) => sources.wait_timeout(events, timeout),
        None => sources.wait(events),
    }
    .map_err(|e| e.into())
    // FIXME? handle if err.kind() == io::ErrorKind::TimedOut
}

/// Get the actual exit code from a finished child process
fn wait_status_to_code(status: process::ExitStatus) -> Option<i32> {
    status.code().or_else(|| Some(128 + status.signal()?))
}
