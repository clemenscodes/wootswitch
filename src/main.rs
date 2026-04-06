mod keyboard;

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use serde::Serialize;
use serde_json::to_string_pretty;

use keyboard::{Keyboard, ProfileListing, ProfileNumber};

#[derive(Parser)]
#[command(name = "wootswitch", about = "Wooting keyboard profile switcher")]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,

    /// Output Waybar-compatible JSON instead of plain text
    #[arg(short, long)]
    waybar: bool,

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

fn waybar_output(listing: &ProfileListing) -> WaybarOutput {
    let current = listing.profiles().iter().find(|p| p.is_current());
    let text = current.map(|p| p.name().to_string()).unwrap_or_default();
    let class = current
        .map(|p| format!("profile-{}", p.number()))
        .unwrap_or_else(|| "profile-unknown".to_string());
    let tooltip = listing
        .profiles()
        .iter()
        .map(|p| {
            if p.is_current() {
                format!("* {p} (current)")
            } else {
                format!("  {p}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let alt = text.clone();
    WaybarOutput { text, tooltip, class, alt }
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
            if args.waybar {
                let output = waybar_output(&listing);
                println!("{}", to_string_pretty(&output)?);
            } else if args.current {
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
