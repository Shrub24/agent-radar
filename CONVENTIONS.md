# Repository conventions

## Commit messages

Use [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/) for new commits:

```text
<type>[optional scope][!]: <description>
```

- `feat`: new functionality.
- `fix`: bug fixes.
- `docs`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`: use the type that describes the change.
- Scope is optional; use it when it clarifies ownership, e.g. `feat(pi-radar): ...` or `build(nix): ...`.
- Mark breaking changes with `!` after the type/scope and explain the break and required migration in the body. A `BREAKING CHANGE:` footer is also valid.
- Write a short, imperative description. Use the body for reasoning and migration instructions when needed.

Examples:

```text
feat(cli): add shell completions
fix(lifecycle): refuse controls naming a stale claim
feat(pi-radar)!: require daemon wire revision 5
docs: record commit message conventions
```

These conventions support future release automation. Product versions are not tied to wire revisions: features and fixes warrant releases even when the wire contract is unchanged. Wire revision and capabilities remain the compatibility contract.

Do not rewrite historical commits to adopt this convention.
