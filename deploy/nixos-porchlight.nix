# Porchlight host prerequisites (master plan M9.1). The owner copies this module into the server's NixOS configuration,
# imports it, sets the two machine-specific values there (they never live in this repo), and rebuilds:
#   porchlight.host = { user = "<SSH login that runs the stack>"; dataMount = "<PORCHLIGHT_DEPLOY_DATA_MOUNT>"; };
{ config, lib, pkgs, ... }:
let
  cfg = config.porchlight.host;
in
{
  options.porchlight.host = {
    user = lib.mkOption {
      type = lib.types.str;
      description = "Account that runs the stack and owns its directories: the deploy SSH login.";
    };
    dataMount = lib.mkOption {
      type = lib.types.str;
      description = "Mount point holding the stack root (<dataMount>/porchlight) and incoming releases.";
    };
  };

  config = {
    # Agent socket group; the API container runs as the stack user with this group.
    users.groups.porchlight = { };
    users.users.${cfg.user}.extraGroups = [ "porchlight" ];

    # The pinned installer verifies releases with minisign; restore decrypts bundles with age.
    environment.systemPackages = with pkgs; [ minisign age ];

    # /run is a tmpfs: recreate the socket directory on every boot, before Docker starts the stack.
    systemd.tmpfiles.rules = [
      "d /run/porchlight 0750 ${cfg.user} porchlight -"
      "d ${cfg.dataMount}/porchlight 0750 ${cfg.user} porchlight -"
    ];

    # Defense in depth for containers outside the release Compose logging policy.
    virtualisation.docker.logDriver = "json-file";
    virtualisation.docker.daemon.settings."log-opts" = {
      "max-size" = "10m";
      "max-file" = "3";
    };

    # The data mount may be nofail: never let Docker start the stack against an empty mountpoint.
    systemd.services.docker.unitConfig.RequiresMountsFor = [ cfg.dataMount ];

    # LAN mTLS API and LAN-only enrollment (the API also checks the enrollment peer CIDR). 9443 stays unpublished.
    networking.firewall.allowedTCPPorts = [ 8443 8444 ];
  };
}
