use clap::Parser;
use popol;
use simplelog::{
    ColorChoice, CombinedLogger, Config, ConfigBuilder, LevelFilter,
    TermLogger, TerminalMode,
};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;
use termcolor::{Color, ColorSpec, WriteColor};

// FIXME UNIX only
use std::os::unix::process::ExitStatusExt;

#[derive(Debug, clap::Parser)]
#[clap(version, about)]
struct Params {
    /// Executable to run
    #[clap(parse(from_os_str))]
    command: PathBuf,
    /// Verbosity (may be repeated up to three times)
    #[clap(short, long, parse(from_occurrences))]
    verbose: u8,
    /// Timeout on individual reads (e.g. "1s", "1h", or "30ms")
    #[clap(long, name="duration", parse(try_from_str = duration_str::parse))]
    idle_timeout: Option<Duration>,
}

fn main() {
    if let Err(error) = cli(Params::parse()) {
        eprintln!("Error: {:#}", error);
        std::process::exit(1);
    }
}

fn cli(params: Params) -> anyhow::Result<()> {
    let filter = match params.verbose {
        3.. => LevelFilter::Trace,
        2 => LevelFilter::Debug,
        1 => LevelFilter::Info,
        0 => LevelFilter::Warn,
    };

    CombinedLogger::init(vec![
        // Default logger
        new_term_logger(filter, new_logger_config().build()),
    ])
    .unwrap();

    println!("command: {:?}", params.command);

    let mut child = Command::new(params.command)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let mut buffer = [0; 1024]; // FIXME: best buffer size?
    let mut sources = popol::Sources::with_capacity(2);
    let mut events = popol::Events::with_capacity(2);

    let mut child_out = child.stdout.take().expect("child.stdout is None");
    sources.register(1, &child_out, popol::interest::READ);

    let mut child_err = child.stderr.take().expect("child.stderr is None");
    sources.register(2, &child_err, popol::interest::READ);

    // FIXME? check if it’s a TTY?
    let mut stdout =
        termcolor::StandardStream::stdout(termcolor::ColorChoice::Auto);
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
                    let count = if *key == 1 {
                        child_out.read(&mut buffer)?
                    } else {
                        child_err.read(&mut buffer)?
                    };

                    if count == 0 {
                        // FIXME detect actual EOF, or SIGCHILD?
                        break 'outer;
                    }

                    if *key == 2 {
                        stdout.set_color(&err_color)?;
                    }

                    stdout.write_all(&buffer[..count])?;

                    if *key == 2 {
                        stdout.reset()?;
                    }

                    if count < buffer.len() {
                        break;
                    }
                }
            }
        }
    }

    let status = child.wait().expect("failed to wait on child");
    std::process::exit(
        code_for_wait_status(status).expect("no exit code or signal for child"),
    );
}

fn wait_on<T>(
    sources: &mut popol::Sources<T>,
    events: &mut popol::Events<T>,
    timeout: Option<Duration>,
) -> anyhow::Result<()>
where
    T: Clone,
    T: Eq,
{
    match timeout {
        Some(timeout) => sources.wait_timeout(events, timeout),
        None => sources.wait(events),
    }
    .map_err(|e| e.into())
    // FIXME? handle if err.kind() == io::ErrorKind::TimedOut
}

fn code_for_wait_status(status: std::process::ExitStatus) -> Option<i32> {
    status.code().or(Some(128 + status.signal()?))
}

fn new_term_logger(level: LevelFilter, config: Config) -> Box<TermLogger> {
    TermLogger::new(level, config, TerminalMode::Mixed, ColorChoice::Auto)
}

fn new_logger_config() -> ConfigBuilder {
    let mut builder = ConfigBuilder::new();
    builder.set_time_to_local(true);
    builder.set_target_level(LevelFilter::Error);
    builder
}
