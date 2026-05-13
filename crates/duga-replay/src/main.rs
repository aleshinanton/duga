use clap::{Parser, ValueEnum};
use duga_replay::{final_text, JsonlReader};
use std::path::PathBuf;

#[derive(Clone, Debug, ValueEnum)]
enum Mode {
    MockAll,
    SubstituteLlm,
    LiveLlm,
}

#[derive(Debug, Parser)]
struct Cli {
    #[arg(long)]
    input: PathBuf,
    #[arg(long, value_enum, default_value_t = Mode::MockAll)]
    mode: Mode,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let events = JsonlReader::read(&cli.input)?;

    match cli.mode {
        Mode::MockAll => {
            println!("{}", final_text(&events).unwrap_or_default());
        }
        Mode::SubstituteLlm | Mode::LiveLlm => {
            println!("loaded {} replay events", events.len());
        }
    }

    Ok(())
}
