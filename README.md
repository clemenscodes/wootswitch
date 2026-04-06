# wootswitch

A Rust CLI for switching Wooting keyboard profiles directly via HID — no Wootility required.

## Usage

```
wootswitch [OPTIONS] [COMMAND]

Commands:
  list         List all profiles with the active one marked [default]
  switch <N>   Switch to profile N (1-based)
  next         Switch to the next profile (wraps around)
  prev         Switch to the previous profile (wraps around)

Options:
  -c, --current   Print only the current profile name (plain text, for scripts)
  -h, --help      Print help
```

All commands output Waybar-compatible JSON by default. Use `--current` to get just the
profile name as plain text.

### Examples

```sh
# Waybar JSON — current profile with full tooltip
wootswitch

# Switch to profile 2 — outputs Waybar JSON for the new state
wootswitch switch 2

# Cycle profiles from a keybind
wootswitch next
wootswitch prev

# Plain text profile name for scripts
wootswitch --current
```

## Supported devices

Any Wooting keyboard. Two firmware variants are supported:

| Variant  | HID `usage_page` | Known devices                        |
|----------|-----------------|--------------------------------------|
| Standard | `0x1337`        | Wooting One, Two, 60HE (original)    |
| ARM      | `0xFF55`        | Wooting 60HE+, Two HE ARM, and later |

Profile names are read directly from the keyboard firmware via `GetProfileMetadata`
(command 55). No state files, no config files, no Wootility dependency.

## Installation

### NixOS (flake)

Add to your flake inputs and enable the NixOS module:

```nix
inputs.wootswitch.url = "github:clemenscodes/wootswitch";

# In your NixOS configuration:
imports = [ inputs.wootswitch.nixosModules.default ];
programs.wootswitch.enable = true;
```

The module installs the binary and configures udev rules so the device is accessible
without root.

### From source

Requires Rust (see `rust-toolchain.toml`) and `libudev`.

```sh
cargo build --release
```

## udev rules

Raw HID access to Wooting devices requires a udev rule. The NixOS module installs this
automatically. For other distributions, add:

```
SUBSYSTEM=="hidraw", ATTRS{idVendor}=="31e3", TAG+="uaccess"
```

Then reload rules: `sudo udevadm control --reload && sudo udevadm trigger`.

## Waybar module

All `--json` output follows the Waybar custom module format — `text` (bar display),
`tooltip` (hover), `class` (CSS), and `alt`.

Add to your Waybar config:

```json
"custom/wootswitch": {
    "exec": "wootswitch",
    "return-type": "json",
    "interval": 5,
    "on-click": "wootswitch next",
    "on-click-right": "wootswitch prev",
    "format": " {}"
}
```

Example output of `wootswitch --current --json`:

```json
{
  "text": "Office",
  "tooltip": "  Profile 1 — Gaming\n* Profile 2 — Office (current)\n  Profile 3 — Media",
  "class": "profile-2",
  "alt": "Office"
}
```

Style by profile in `style.css`:

```css
#custom-wootswitch.profile-1 { color: #ff6b6b; }
#custom-wootswitch.profile-2 { color: #a8e6cf; }
#custom-wootswitch.profile-3 { color: #ffd3a5; }
#custom-wootswitch.profile-4 { color: #c3a6ff; }
```

## Protocol

See [`docs/hid-protocol.md`](docs/hid-protocol.md) for a full account of the reverse-engineered
Wooting HID wire protocol, including packet format, command IDs, ARM vs. Standard differences,
and the profile switching sequence.
