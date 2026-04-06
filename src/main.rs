mod keyboard;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use hidapi::HidApi;
use serde_json::{json, to_string_pretty};

use keyboard::{Keyboard, ProfileListing, ProfileNumber};

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

    /// Output as JSON
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
    let api = HidApi::new().context("Failed to initialise HID API")?;

    if args.list_devices {
        Keyboard::list_all_devices(&api);
        return Ok(());
    }

    if let Some(Command::Switch { profile }) = args.command {
        let keyboard = Keyboard::find(&api)?;
        let switched = keyboard.switch_to(ProfileNumber::from(profile))?;
        if args.json {
            println!("{}", json!({ "switched_to": profile }));
        } else {
            println!("{keyboard}: switched to {switched}");
        }
        return Ok(());
    }

    let keyboard = Keyboard::find(&api)?;

    if args.current {
        let active = keyboard.active_profile().ok();
        match (active, args.json) {
            (Some(number), true) => println!("{}", json!({ "current": number })),
            (Some(number), false) => println!("{number}"),
            (None, true) => println!("{}", json!({ "current": null })),
            (None, false) => bail!("Could not read current profile from keyboard"),
        }
        return Ok(());
    }

    let listing = keyboard.profiles()?;
    if args.json {
        println!("{}", to_string_pretty(&listing)?);
    } else {
        println!("{keyboard}");
        print_listing(&listing);
    }

    Ok(())
}
