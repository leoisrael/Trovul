# Contributing to Trovul

Thank you for helping make Trovul better. Trovul is source-available under the
[PolyForm Shield License 1.0.0](LICENSE), not an OSI Open Source license. You
may contribute fixes and improvements, but neither the project nor derivative
works may be used to provide a product that competes with Trovul.

Before opening a pull request, read and accept the
[Contributor License Agreement](CONTRIBUTOR-LICENSE-AGREEMENT.md). The CLA lets
the project maintainers use, distribute, and relicense accepted contributions
while you retain credit for your work.

## How to contribute

### Bugs and feature requests

Check existing issues before filing a new one. A useful report includes:

- Windows version and audio devices in use;
- precise steps to reproduce the behavior;
- expected and actual behavior;
- relevant logs with personal data and tokens removed.

For security concerns, follow [SECURITY.md](SECURITY.md) instead of opening a
public issue.

### Pull requests

1. Fork the repository and create a focused branch.
2. Make the smallest complete change that solves the problem.
3. Run the relevant checks and explain anything you could not run.
4. Open a pull request against `main`, complete its checklist, and confirm that
   you accept the CLA.

Do not include credentials, customer data, generated build artifacts,
proprietary code, or assets you do not have permission to license.

## Development setup

- Node.js 20.19+ and current stable Rust are required.
- Install JavaScript dependencies with `npm ci`.
- Run the desktop app with `npm run tauri dev`.
- Run frontend checks with `npm run build` and Rust checks from `src-tauri`.

## Project structure

```
trovul/
├── src/              React and TypeScript desktop interface
├── src-tauri/src/    Rust services, networking, audio, and Windows integration
├── streamdeck/       Stream Deck plug-in
├── docs/             User-facing technical and release documentation
└── package.json      Frontend scripts and dependencies
```

Start with an issue or pull-request discussion before attempting a broad
feature. Changes to pairing, network protocol, privileged input, installer
behavior, updates, or the Companion API need maintainer approval before code is
written.

## Code style

- Run `cargo fmt` for Rust changes.
- Keep TypeScript and React changes consistent with the surrounding code.
- Use clear, focused commits and explain user-visible behavior.

## Code of conduct

This project follows the [Contributor Covenant Code of
Conduct](CODE_OF_CONDUCT.md). By participating, you agree to uphold a welcoming
and inclusive environment.
