mod keyboard;

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use serde::Serialize;
use serde_json::to_string_pretty;

use keyboard::{Keyboard, Profile, ProfileNumber};

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

/// Waybar custom module output format.
///
/// Waybar reads this when `return-type = "json"` is set on the module.
/// `text` is shown in the bar; `tooltip` on hover; `class` enables CSS styling.
#[derive(Serialize)]
struct WaybarOutput {
    text: String,
    tooltip: String,
    class: String,
    alt: String,
}

fn waybar_from_profile(profile: &Profile) -> WaybarOutput {
    let text = profile.name().to_string();
    let class = format!("profile-{}", profile.number());
    let tooltip = format!("{profile}");
    let alt = text.clone();
    WaybarOutput { text, tooltip, class, alt }
}

fn print_json(output: &WaybarOutput) -> Result<()> {
    println!("{}", to_string_pretty(output)?);
    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();
    let api = hidapi::HidApi::new().map_err(|e| anyhow::anyhow!("Failed to initialise HID API: {e}"))?;
    let keyboard = Keyboard::find(&api)?;

    match args.command {
        Some(Command::Switch { profile }) => {
            let switched = keyboard.switch_to(ProfileNumber::from(profile))?;
            let output = waybar_from_profile(&switched);
            print_json(&output)?;
        }
        Some(Command::Next) => {
            let switched = keyboard.switch_next()?;
            let output = waybar_from_profile(&switched);
            print_json(&output)?;
        }
        Some(Command::Prev) => {
            let switched = keyboard.switch_prev()?;
            let output = waybar_from_profile(&switched);
            print_json(&output)?;
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
