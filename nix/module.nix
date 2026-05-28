self: {
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.programs.wootswitch;
in {
  options.programs.wootswitch = {
    enable = lib.mkEnableOption "wootswitch Wooting keyboard profile switcher";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.wootswitch;
      defaultText = lib.literalExpression "inputs.wootswitch.packages.\${system}.wootswitch";
      description = "The wootswitch package to use.";
    };
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [cfg.package];

    # Grant logged-in users (via systemd-logind uaccess) read/write access to
    # Wooting HID raw devices without requiring root or group membership.
    services.udev.extraRules = ''
      SUBSYSTEM=="hidraw", ATTRS{idVendor}=="03eb", TAG+="uaccess"
      SUBSYSTEM=="hidraw", ATTRS{idVendor}=="31e3", TAG+="uaccess"
    '';
  };
}
