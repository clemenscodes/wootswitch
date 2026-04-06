# wootswitch

A Rust CLI for switching Wooting keyboard profiles directly via HID — no Wootility required.

## Usage

```
wootswitch [OPTIONS] [COMMAND]

Commands:
  list            List all profiles with the active one marked (default)
  switch [N]      Switch profiles

List options:
  -w, --waybar      Output Waybar-compatible JSON instead of plain text

Switch options (exactly one required):
  <N>               Profile number (1-based)
  --next            Switch to the next profile (wraps around)
  --previous        Switch to the previous profile (wraps around)

Options:
  -c, --current   Print only the current profile name (plain text, for scripts)
  -h, --help      Print help
```

### Examples

```sh
# List all profiles (human-readable text)
wootswitch list

# Waybar JSON — current profile with full tooltip (for the widget)
wootswitch

# Switch to profile 2
wootswitch switch 2

# Cycle profiles from a keybind
wootswitch switch --next
wootswitch switch --previous

# Plain text profile name for scripts
wootswitch --current
```

`wootswitch list` output:

```
* Profile 1 — Coding (current)
  Profile 2 — CS2
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

```json
"custom/wootswitch": {
    "exec": "wootswitch list --waybar",
    "return-type": "json",
    "interval": 5,
    "on-click": "wootswitch switch --next",
    "on-click-right": "wootswitch switch --previous",
    "format": " {}"
}
```

`wootswitch --waybar` output:

```json
{
  "text": "Coding",
  "tooltip": "* Profile 1 — Coding (current)\n  Profile 2 — CS2",
  "class": "profile-1",
  "alt": "Coding"
}
```

Style by profile in `style.css`:

```css
#custom-wootswitch.profile-1 { color: #a8e6cf; }
#custom-wootswitch.profile-2 { color: #ff6b6b; }
```


## Protocol

See [`docs/hid-protocol.md`](docs/hid-protocol.md) for a full account of the reverse-engineered
Wooting HID wire protocol, including packet format, command IDs, ARM vs. Standard differences,
and the profile switching sequence.
