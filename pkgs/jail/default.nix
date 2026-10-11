# Wraps every binary of a package so it runs with the given env vars (name ->
# fixed absolute path, created 0700 on start); child processes inherit them.
# Desktop files are copied with the store path swapped, so they launch the wrapper.
#   jail pkgs.obsidian {XDG_CONFIG_HOME = "/persist/generated/home/nikodem/obsidian";}
{
  lib,
  symlinkJoin,
  makeWrapper,
  coreutils,
}: package: env:
assert lib.assertMsg (lib.all (n: builtins.match "[A-Z_][A-Z0-9_]*" n != null) (lib.attrNames env)) "jail ${package.name}: env names must match [A-Z_][A-Z0-9_]*";
assert lib.assertMsg (lib.all (v: builtins.match "/[A-Za-z0-9._/-]*" v != null) (lib.attrValues env)) "jail ${package.name}: env values must be absolute paths of [A-Za-z0-9._/-]";
  symlinkJoin {
    inherit (package) name meta;
    passthru = package.passthru or {};
    paths = [package];
    nativeBuildInputs = [makeWrapper];
    postBuild = ''
      for f in $out/bin/*; do
        wrapProgram "$f" ${lib.concatStringsSep " " (lib.mapAttrsToList (n: v: "--set ${n} ${v} --run ${lib.escapeShellArg "${coreutils}/bin/mkdir -p -m 0700 ${v}"}") env)}
      done

      apps=$out/share/applications
      if [ -d "$apps" ]; then
        if [ -L "$apps" ]; then
          rm "$apps"
          mkdir "$apps"
          ln -s ${package}/share/applications/* "$apps"/
        fi
        for f in "$apps"/*.desktop; do
          sed "s|${package}|$out|g" "$f" > "$f.new"
          mv "$f.new" "$f"
        done
      fi
    '';
  }
