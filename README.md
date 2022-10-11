This is a fork of Rojo with changes specific to the workflow at Uplift Games.

New Uplift Games-specific releases should:
* Be created on the `uplift-games-fork-releases` branch (this is like our `main`)
* Be tagged with an appropriate semver **plus** a pre-release tag in the following format:\
  `v1.2.3-uplift.release.1`\
  ...where `v1.2.3` is the semver and `uplift.release.1` increments with each
  release under that semver.\
  **This tag should be created locally and pushed to kick off automated builds (see *Notes on version tags*)**
* Include additions to `CHANGELOG.md` listing the additions. If we rebase on a
  new version of Rojo that includes some of our additions, we should list only
  what has changed between upstream Rojo and our fork.
* Where possible, our changes should become PRs to the upstream Rojo repo. When
  we do this, we should include a link to the PR in the changelog entry.

Notes on version tags:
* Tags can be created locally with the command `git tag v1.2.3-uplift.release.1`
* Tags can be pushed to the remote with the command `git push origin v1.2.3-uplift.release.1`
* When a tag starting with `v` is pushed to this repo, an action is kicked off
  which creates a release draft and attached build artifacts when they're
  completed. Go to the releases page and edit the draft to publish it.

---

<div align="center">
    <a href="https://rojo.space"><img src="assets/logo-512.png" alt="Rojo" height="217" /></a>
</div>

<div>&nbsp;</div>

<div align="center">
    <a href="https://github.com/rojo-rbx/rojo/actions"><img src="https://github.com/rojo-rbx/rojo/workflows/CI/badge.svg" alt="Actions status" /></a>
    <a href="https://crates.io/crates/rojo"><img src="https://img.shields.io/crates/v/rojo.svg?label=latest%20release" alt="Latest server version" /></a>
    <a href="https://rojo.space/docs"><img src="https://img.shields.io/badge/docs-website-brightgreen.svg" alt="Rojo Documentation" /></a>
    <a href="https://www.patreon.com/lpghatguy"><img src="https://img.shields.io/badge/sponsor-patreon-red" alt="Patreon" /></a>
</div>

<hr />

**Rojo** is a tool designed to enable Roblox developers to use professional-grade software engineering tools.

With Rojo, it's possible to use industry-leading tools like **Visual Studio Code** and **Git**.

Rojo is designed for power users who want to use the best tools available for building games, libraries, and plugins.

## Features
Rojo enables:

* Working on scripts and models from the filesystem, in your favorite editor
* Versioning your game, library, or plugin using Git or another VCS
* Streaming `rbxmx` and `rbxm` models into your game in real time
* Packaging and deploying your project to Roblox.com from the command line

In the future, Rojo will be able to:

* Sync instances from Roblox Studio to the filesystem
* Automatically convert your existing game to work with Rojo
* Import custom instances like MoonScript code

## [Documentation](https://rojo.space/docs)
Documentation is hosted in the [rojo.space repository](https://github.com/rojo-rbx/rojo.space).

## Contributing
Check out our [contribution guide](CONTRIBUTING.md) for detailed instructions for helping work on Rojo!

Pull requests are welcome!

Rojo supports Rust 1.58.1 and newer. The minimum supported version of Rust is based on the latest versions of the dependencies that Rojo has.

## License
Rojo is available under the terms of the Mozilla Public License, Version 2.0. See [LICENSE.txt](LICENSE.txt) for details.