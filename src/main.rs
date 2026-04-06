mod keyboard;

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use keyboard::{Keyboard, ProfileNumber};

#[derive(Parser)]
#[command(name = "wootswitch", about = "Wooting keyboard profile switcher")]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,

    /// Print only the current profile name (plain text, for scripts)
    #[arg(short, long)]
    current: bool,
}

#[derive(Subcommand)]
enum Command {
    /// List all profiles with the active one marked (default)
    List,
    /// Switch to profile N (1-based)
    Switch {
        /// Profile number (1-based)
        profile: u8,
    },
    /// Switch to the next profile (wraps around)
    Next,
    /// Switch to the previous profile (wraps around)
    Prev,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let api = hidapi::HidApi::new().map_err(|e| anyhow::anyhow!("Failed to initialise HID API: {e}"))?;
    let keyboard = Keyboard::find(&api)?;

    match args.command {
        Some(Command::Switch { profile }) => {
            let switched = keyboard.switch_to(ProfileNumber::from(profile))?;
            println!("switched to {switched}");
        }
        Some(Command::Next) => {
            let switched = keyboard.switch_next()?;
            println!("switched to {switched}");
        }
        Some(Command::Prev) => {
            let switched = keyboard.switch_prev()?;
            println!("switched to {switched}");
        }
        Some(Command::List) | None => {
            let listing = keyboard.profiles()?;
            if args.current {
                let current = listing.profiles().iter().find(|p| p.is_current());
                match current {
                    Some(profile) => println!("{}", profile.name()),
                    None => bail!("Could not read current profile from keyboard"),
                }
            } else {
                for profile in listing.profiles() {
                    if profile.is_current() {
                        println!("* {profile} (current)");
                    } else {
                        println!("  {profile}");
                    }
                }
            }
        }
    }

    Ok(())
}
