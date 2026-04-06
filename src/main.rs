mod keyboard;

use anyhow::{bail, Result};
use clap::{ArgGroup, CommandFactory, Parser, Subcommand};
use clap_complete::{generate, Shell};
use serde::Serialize;
use serde_json::to_string;
use std::io::stdout;

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
    /// Print a shell completion script to stdout
    #[command(hide = true)]
    Completions { shell: Shell },
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

    // Completions are handled before HID initialisation: the Nix build sandbox
    // and CI have no keyboard hardware, but still need to generate completion scripts.
    if let Some(Command::Completions { shell }) = args.command {
        generate(shell, &mut Args::command(), "wootswitch", &mut stdout());
        return Ok(());
    }

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
                let Some(target) = profile else {
                    unreachable!(
                        "clap ArgGroup guarantees profile is set when next and previous are false"
                    );
                };
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
                print!("{listing}");
            }
        }
        Some(Command::Completions { .. }) => unreachable!("handled before HID init"),
        None => {
            let listing = keyboard.profiles()?;
            if args.current {
                let current = listing.profiles().iter().find(|p| p.is_current());
                match current {
                    Some(profile) => println!("{}", profile.name()),
                    None => bail!("Could not read current profile from keyboard"),
                }
            } else {
                print!("{listing}");
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use keyboard::testutil;

    fn two_profile_listing() -> ProfileListing {
        testutil::listing(
            vec![
                testutil::profile(1, false, "Default"),
                testutil::profile(2, true, "Gaming"),
            ],
            Some(2),
        )
    }

    #[test]
    fn waybar_text_is_current_profile_name() {
        let output = waybar_output(&two_profile_listing());
        assert_eq!(output.text, "Gaming");
    }

    #[test]
    fn waybar_alt_matches_text() {
        let output = waybar_output(&two_profile_listing());
        assert_eq!(output.alt, output.text);
    }

    #[test]
    fn waybar_class_includes_profile_number() {
        let output = waybar_output(&two_profile_listing());
        assert_eq!(output.class, "profile-2");
    }

    #[test]
    fn waybar_tooltip_marks_current_profile() {
        let output = waybar_output(&two_profile_listing());
        assert!(output.tooltip.contains("* Profile 2 — Gaming (current)"));
        assert!(output.tooltip.contains("  Profile 1 — Default"));
    }

    #[test]
    fn waybar_unknown_class_when_no_active_profile() {
        let listing = testutil::listing(vec![testutil::profile(1, false, "Default")], None);
        let output = waybar_output(&listing);
        assert_eq!(output.class, "profile-unknown");
        assert_eq!(output.text, "");
    }
}
