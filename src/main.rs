mod keyboard;

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use serde::Serialize;
use serde_json::to_string_pretty;

use keyboard::{Keyboard, Profile, ProfileListing, ProfileNumber};

#[derive(Parser)]
#[command(name = "wootswitch", about = "Wooting keyboard profile switcher")]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,

    /// Print only the current profile number (1-based)
    #[arg(short, long)]
    current: bool,

    /// List all detected Wooting HID interfaces (for debugging)
    #[arg(short = 'D', long)]
    list_devices: bool,

    /// Output as Waybar-compatible JSON
    #[arg(short, long)]
    json: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Switch to profile N (1-based)
    Switch {
        /// Profile number (1-based)
        profile: u8,
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

fn waybar_from_listing(listing: &ProfileListing) -> WaybarOutput {
    let current_profile = listing.profiles().iter().find(|p| p.is_current());
    let text = current_profile.map(|p| p.name().to_string()).unwrap_or_default();
    let class = current_profile
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

fn waybar_from_profile(profile: &Profile) -> WaybarOutput {
    let text = profile.name().to_string();
    let class = format!("profile-{}", profile.number());
    let tooltip = format!("{profile}");
    let alt = text.clone();
    WaybarOutput { text, tooltip, class, alt }
}

fn print_listing(listing: &ProfileListing) {
    for profile in listing.profiles() {
        if profile.is_current() {
            println!("  * {profile} (current)");
        } else {
            println!("    {profile}");
        }
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    let api = hidapi::HidApi::new().map_err(|e| anyhow::anyhow!("Failed to initialise HID API: {e}"))?;

    if args.list_devices {
        Keyboard::list_all_devices(&api);
        return Ok(());
    }

    if let Some(Command::Switch { profile }) = args.command {
        let keyboard = Keyboard::find(&api)?;
        let switched = keyboard.switch_to(ProfileNumber::from(profile))?;
        if args.json {
            let output = waybar_from_profile(&switched);
            println!("{}", to_string_pretty(&output)?);
        } else {
            println!("{keyboard}: switched to {switched}");
        }
        return Ok(());
    }

    let keyboard = Keyboard::find(&api)?;

    if args.current {
        if args.json {
            let listing = keyboard.profiles()?;
            let output = waybar_from_listing(&listing);
            println!("{}", to_string_pretty(&output)?);
        } else {
            let active = keyboard.active_profile().ok();
            match active {
                Some(number) => println!("{number}"),
                None => bail!("Could not read current profile from keyboard"),
            }
        }
        return Ok(());
    }

    let listing = keyboard.profiles()?;
    if args.json {
        let output = waybar_from_listing(&listing);
        println!("{}", to_string_pretty(&output)?);
    } else {
        println!("{keyboard}");
        print_listing(&listing);
    }

    Ok(())
}
