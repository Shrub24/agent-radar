## MODIFIED Requirements

### Requirement: Declarative per-user installation
The flake SHALL export `homeManagerModules.default` with `programs.radar.enable` and `programs.radar.package`. Enabling the module SHALL install the selected package, defaulting to this flake's package for the consumer system, and SHALL declare the daemon's socket and service units and the tmux server template. Disabling it SHALL add neither a package, a configuration file, a unit nor a runtime directory. The module SHALL create no runtime-state directory itself.

#### Scenario: Enabled module
- **WHEN** a Home Manager consumer imports the module and enables Radar
- **THEN** the flake's package is included in the user's packages and the daemon units are declared

#### Scenario: Package override
- **WHEN** an enabled consumer supplies `programs.radar.package`
- **THEN** that package is installed instead of the default package and the units run it

#### Scenario: Disabled module
- **WHEN** the imported module is disabled
- **THEN** it adds no Radar package, generated configuration, unit or runtime directory
