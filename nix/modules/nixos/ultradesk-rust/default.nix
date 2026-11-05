{
  inputs,
}:
{
  config,
  lib,
  pkgs,
  ...
}:
with lib;
let
  cfg = config.programs.ultradesk-rust;
  ultradesk-rust = inputs.self.packages.${pkgs.stdenv.hostPlatform.system}.ultradesk-rust;
in
{
  options.programs.ultradesk-rust = {
    enable = mkEnableOption "UltraDesk (Rust Edition) remote desktop controller and host";

    package = mkOption {
      type = types.package;
      default = ultradesk-rust;
      defaultText = literalExpression "inputs.self.packages.\${pkgs.stdenv.hostPlatform.system}.ultradesk-rust";
      description = "The ultradesk-rust package to install.";
    };
  };

  config = mkIf cfg.enable {
    environment.systemPackages = [ cfg.package ];
  };
}
