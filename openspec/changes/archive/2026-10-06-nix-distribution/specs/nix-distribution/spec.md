# Spec Delta

## Purpose

Make Radar installable from a locked Nix flake and configurable through Home Manager without a development checkout or a running fleet.

## ADDED Requirements

### Requirement: Binary installation through flake outputs
The flake SHALL expose `packages.<system>.radar`, its `default` alias and a default app invoking the installed `radar` binary on `x86_64-linux` and `aarch64-linux`. Existing development shells SHALL remain available. The package SHALL not require a development shell to execute.

#### Scenario: Default package and app
- **WHEN** a consumer builds the default package or selects `packages.<system>.radar`
- **THEN** both select the same package containing `bin/radar`
- **AND** the default app invokes that installed executable

#### Scenario: Noninteractive invocation without a fleet
- **WHEN** the installed binary is invoked with `--help` or `--print-config` in an isolated user environment with no Herdr executable or owner-control directories
- **THEN** it produces its existing help or effective configuration output without requiring fleet access

#### Scenario: Development shell preserved
- **WHEN** a contributor enters the existing default development shell
- **THEN** its Rust and development tools remain available

### Requirement: Locked and checked package build
The package SHALL build its Rust dependencies from `Cargo.lock` using declared Nix inputs without network access in the ordinary build/check phases. Package checks SHALL remain enabled, and `checks.<system>.package` SHALL select the package derivation. Automated checks SHALL use isolated fake transports, never a real fleet or owner-control directory.

#### Scenario: Sandboxed build
- **WHEN** the package is built in the supported native Linux sandbox after its declared sources are available
- **THEN** Cargo builds and checks using locked dependencies without network access
- **AND** required test interpreters and utilities are supplied explicitly rather than relying on a development shell

#### Scenario: Flake package check
- **WHEN** the consumer runs the package check through the flake
- **THEN** the same checked package derivation is built without starting or mutating a live fleet

### Requirement: Declarative per-user installation
The flake SHALL export `homeManagerModules.default` with `programs.radar.enable` and `programs.radar.package`. Enabling the module SHALL install the selected package, defaulting to this flake's package for the consumer system. Disabling it SHALL add neither a package nor a configuration file. The module SHALL create no service or runtime-state directories.

#### Scenario: Enabled module
- **WHEN** a Home Manager consumer imports the module and enables Radar
- **THEN** the flake's package is included in the user's packages

#### Scenario: Package override
- **WHEN** an enabled consumer supplies `programs.radar.package`
- **THEN** that package is installed instead of the default package

#### Scenario: Disabled module
- **WHEN** the imported module is disabled
- **THEN** it adds no Radar package, generated configuration, service or runtime directory

### Requirement: Optional native configuration
The module SHALL expose nullable freeform TOML `programs.radar.settings`, defaulting to null. Null SHALL leave configuration unmanaged. Supplied settings SHALL generate `radar/config.toml` under the user's XDG configuration directory without replacing Radar's native parsing, defaults or environment precedence.

#### Scenario: Installation only
- **WHEN** a consumer enables Radar without supplying settings
- **THEN** the module installs Radar without managing a configuration file

#### Scenario: Explicit settings
- **WHEN** an enabled consumer supplies settings including a colour, animation or process mark
- **THEN** the module generates valid TOML at the existing XDG path
- **AND** Radar reads those values through its native configuration loader

#### Scenario: Existing configuration override
- **WHEN** the consumer sets `RADAR_CONFIG` to another configuration path
- **THEN** Radar's existing environment precedence is unchanged
- **AND** the module does not force an overriding `RADAR_CONFIG` value

### Requirement: Explicit runtime dependencies and consumer guidance
The distribution SHALL leave Herdr and optional icon fonts separately installed and preserve native fallback behavior. Installation documentation SHALL show the canonical flake URL, package/app use and Home Manager import/configuration, including runtime prerequisites and the supported package systems.

#### Scenario: No bundled runtime
- **WHEN** a consumer installs Radar from the flake
- **THEN** it does not implicitly install Herdr, copy font assets or depend on a local developer checkout
- **AND** missing runtime/font behavior remains the same as the source-built program

#### Scenario: Direct flake input
- **WHEN** a consumer follows the documented flake-input and Home Manager example using compatible locked inputs
- **THEN** module evaluation selects the package and generates the requested native configuration without requiring this repository's development shell
