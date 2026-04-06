mod keyboard;

use anyhow::{bail, Result};
use clap::{ArgGroup, Parser, Subcommand};
use serde::Serialize;
use serde_json::to_string;

use keyboard::{Keyboard, ProfileListing, ProfileNumber};

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
    List {
        /// Output Waybar-compatible JSON instead of plain text
        #[arg(short, long)]
        waybar: bool,
    },
    /// Switch profiles
    #[command(group(ArgGroup::new("target").required(true).args(["profile", "next", "previous"])))]
    Switch {
        /// Profile number (1-based) or profile name (case-insensitive); ambiguous names error with a list
        profile: Option<String>,
        /// Switch to the next profile (wraps around)
        #[arg(long)]
        next: bool,
        /// Switch to the previous profile (wraps around)
        #[arg(long)]
        previous: bool,
    },
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
    WaybarOutput {
        text,
        tooltip,
        class,
        alt,
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    let api =
        hidapi::HidApi::new().map_err(|e| anyhow::anyhow!("Failed to initialise HID API: {e}"))?;
    let keyboard = Keyboard::find(&api)?;

    match args.command {
        Some(Command::Switch {
            profile,
            next,
            previous,
        }) => {
            let switched = if next {
                keyboard.switch_next()?
            } else if previous {
                keyboard.switch_prev()?
            } else {
                let target = profile.unwrap();
                if let Ok(number) = target.parse::<u8>() {
                    keyboard.switch_to(ProfileNumber::from(number))?
                } else {
                    let listing = keyboard.profiles()?;
                    let matches: Vec<_> = listing
                        .profiles()
                        .iter()
                        .filter(|p| p.name().eq_ignore_ascii_case(&target))
                        .collect();
                    match matches.as_slice() {
                        [] => bail!("No profile named '{target}'"),
                        [found] => keyboard.switch_to(found.number())?,
                        _ => {
                            let candidates = matches
                                .iter()
                                .map(|p| format!("  {} — {}", p.number(), p.name()))
                                .collect::<Vec<_>>()
                                .join("\n");
                            bail!("Ambiguous name '{target}', use a number instead:\n{candidates}")
                        }
                    }
                }
            };
            println!("switched to {switched}");
        }
        Some(Command::List { waybar }) => {
            let listing = keyboard.profiles()?;
            if waybar {
                let output = waybar_output(&listing);
                println!("{}", to_string(&output)?);
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
        None => {
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
