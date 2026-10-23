# An agent started as a Radar managed launch.
#
# The wrapper takes the original's name and runs it through `radar run`, naming
# the original by store path. A wrapper that named it by `PATH` lookup would
# resolve back to itself the moment it was placed ahead of the original.
{
  lib,
  writeShellApplication,
  radar,
  program,
}:

writeShellApplication {
  name = program.meta.mainProgram or program.pname or (lib.getName program);
  text = ''
    exec ${radar}/bin/radar run --agent -- ${lib.getExe program} "$@"
  '';
  meta.description = "${lib.getName program}, started as a Radar managed agent";
}
