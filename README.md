<div align="center">
    <a href="https://rojo.space"><img src="assets/logo-512.png" alt="Rojo" height="217" /></a>
</div>

<div>&nbsp;</div>

<div align="center">
    <a href="https://github.com/rojo-rbx/rojo/actions"><img src="https://github.com/rojo-rbx/rojo/workflows/CI/badge.svg" alt="Actions status" /></a>
    <a href="https://crates.io/crates/rojo"><img src="https://img.shields.io/crates/v/rojo.svg?label=latest%20release" alt="Latest server version" /></a>
    <a href="https://rojo.space/docs"><img src="https://img.shields.io/badge/docs-website-brightgreen.svg" alt="Rojo Documentation" /></a>
</div>

<hr />

# Uplift Games Fork

This branch is used for making releases for Uplift Games' fork of Rojo. Very little thought has been put into how this branch would merge with others, because it is not the intention.

Instead, this branch has changes made freely so that [releases](https://github.com/UpliftGames/rojo/releases) for Syncback's prototype can be made. These include at the time of writing:

- The patch for Model pivots, which is present upstream but not in the version syncback is based on. It is in its own branch to avoid merge conflicts down the line.
- Changing the version of the CLI and plugin.
- All [`rbx-dom`](https://github.com/rojo-rbx/rbx-dom/) dependencies point directly at [our fork](https://github.com/UpliftGames/rbx-dom/tree/master)'s `master` branch
- This README change.

However, this may change since **no stability is guaranteed on this branch**. If you're looking for syncback's implementation, check `syncback-tests` and `syncback-incremental`.

# Rojo README

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

Rojo supports Rust 1.70.0 and newer. The minimum supported version of Rust is based on the latest versions of the dependencies that Rojo has.

## License
Rojo is available under the terms of the Mozilla Public License, Version 2.0. See [LICENSE.txt](LICENSE.txt) for details.