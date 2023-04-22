This is a fork of Rojo with changes specific to the workflow at Uplift Games.

Changes from upstream Rojo:
* TOML support
* Ability to define the type of a file using glob patterns
* Font property support
* Model Scale property support
* Adds Font and Gui Inset migrations


<details><summary>Release Instructions</summary>

New Uplift Games-specific releases should:
* Be created on the `uplift-games-fork-releases` branch (this is like our `main`)
* Be tagged with an appropriate semver **plus** a pre-release tag in the following format:\
  `v1.2.3-uplift.1`\
  ...where `v1.2.3` is the semver and `uplift.1` increments with each
  release under that semver.\
  **This tag should be created locally and pushed to kick off automated builds (see *Notes on version tags*)**
* The chosen semver should be *relative to upstream according to the difference
  at that release.*\
  For example, if upstream is on `v1.0.0` and we make a minor
  change, we'll be on `v1.0.1-uplift.1`. If we make another minor
  change, we'll be on `v1.0.1-uplift.2` *because we are still only minor
  changes away from upstream*. This way, if our changes get upstreamed, we won't
  be going backwards in semver.
* Add our changes to `CHANGELOG.md`. If we rebase on a
  new version of Rojo that includes some of our additions, we should list only
  what has changed between upstream Rojo and our fork.
* Where possible, our changes should become PRs to the upstream Rojo repo. When
  we do this, we should include a link to the PR in the changelog entry.
* Where possible, our changes should become PRs to the upstream Rojo repo. When
  we do this, we should include a link to the PR in the changelog entry.

Notes on version tags:
* Tags can be created locally with the command `git tag v1.2.3-uplift.1`
* Tags can be pushed to the remote with the command `git push origin v1.2.3-uplift.1`
* When a tag starting with `v` is pushed to this repo, an action is kicked off
  which creates a release draft and attached build artifacts when they're
  completed. Go to the releases page and edit the draft to publish it.

</details>

---

<div align="center">
    <a href="https://rojo.space"><img src="assets/logo-512.png" alt="Rojo" height="217" /></a>
</div>

<div>&nbsp;</div>

<hr />

**Rojo** is a tool designed to enable Roblox developers to use professional-grade software engineering tools.

With Rojo, it's possible to use industry-leading tools like **Visual Studio Code** and **Git**.

Rojo is designed for power users who want to use the best tools available for building games, libraries, and plugins.


## Installation

The following instructions are for installing the Uplift fork of rojo. If
you're looking to install standard Rojo, see [Rojo's Installation
section](https://rojo.space/docs/v7/getting-started/installation/).

### With [Aftman](https://github.com/LPGhatguy/aftman)
Rojo can be installed with Aftman, a toolchain manager for Roblox projects:

```toml
[tools]
rojo = "UpliftGames/rojo@7.3.0-uplift.10"
```

### From GitHub Releases
You can download pre-built binaries from [the GitHub Releases page](https://github.com/UpliftGames/rojo/releases).

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