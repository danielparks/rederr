use simplelog::{
    ColorChoice, CombinedLogger, Config, ConfigBuilder, LevelFilter,
    TermLogger, TerminalMode,
};
use std::io::Read;
use std::path::PathBuf;
use std::process::exit;
use std::process::{Command, Stdio};
use structopt::StructOpt;

#[derive(Debug, StructOpt)]
struct Params {
    /// Executable to run
    #[structopt(parse(from_os_str))]
    command: PathBuf,
    /// Verbosity (may be repeated up to three times)
    #[structopt(short, long, parse(from_occurrences))]
    verbose: u8,
}

fn main() {
    if let Err(error) = cli(Params::from_args()) {
        eprintln!("Error: {:#}", error);
        exit(1);
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
        .spawn()?;

    let mut buffer = [0; 5]; // FIXME: best buffer size?
    let mut child_out = child
        .stdout
        .take()
        .expect("could not get stdout from child");

    loop {
        let bytes = child_out.read(&mut buffer)?;

        if bytes == 0 {
            // FIXME detect actual EOF, or SIGCHILD?
            break;
        }

        println!(
            "output [{}]: {:?}",
            bytes,
            std::str::from_utf8(&buffer[0..bytes])?
        );
    }

    let output = child.wait_with_output().expect("failed to wait on child");

    println!(
        "final output: {:?}",
        std::str::from_utf8(output.stdout.as_slice())?
    );

    Ok(())
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
