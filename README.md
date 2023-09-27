This is a fork of Rojo with changes specific to the workflow at Uplift Games.

Changes from upstream Rojo:
* TOML support
* Ability to define the type of a file using glob patterns
* Font property support
* Model Scale property support
* Adds Font and Gui Inset migrations
* MeshPart support
* UniqueId support

## Syncback Experimental Branch

This branch additionally contains experimental `syncback` and `diff` commands, implementing **file-based two-way sync** and tree diffing.

Known issues:
- The code is messy! This was hacked away at constantly with little cleanup.
- Diffing can take a while on large projects
- The diff display displays some changes which are intentionally never
  committed. For example, it displays all extra services in the place file.
- ~~Localization files nearly always show up in diffs. Their contents are actually
  just a json string, and they'll be "different" even if the json decodes to the
  same. Additionally, Roblox's json output is not deterministic.~~\
  This is mostly fixed now — we reserialize the localization table json
  deterministically before diffing.
- Absolutely zero tests for all of this new functionality
- Other things I'm almost certainly forgetting!

How to:
1. Prepare your project for assets by adding folders and project file entries for common asset locations like Workspace and ReplicatedStorage
2. Save your place file somewhere accessible
3. Syncback: `rojo syncback --input path/to/your/place.rbxl path/to/your/project_name.project.json`
4. Confirm that the diff looks right and it will write the assets to the filesystem

Exciting features list:
- Can turn any instance into an item on the filesystem
- Chooses filesystem representation intelligently based on instance class,
  properties, etc.
- Prefers to keep existing filesystem representation if possible
- Only syncs changed instances to minimize git diffs
- Built-in filters for problematic properties, plus the ability to define your own
- Blob exclusion lists for places that shouldn't be sync back into (e.g. code directories)
- Ability to sync from a project to another project

New `project.json` fields:

```jsonc
{
    "syncback": { // optional
        "excludeGlobs": ["glob/paths/here"], // optional
        "skipInstanceNames": [               // optional
            // skip every instance with one of these names

            // These instances practically become invisible to all syncback operations except
            // serialization of their ancestors. They will not trigger diff changes and they
            // will not be saved as their own file on the filesystem.
            "InstanceName"
        ],
        "propertyDefaults": {                // optional
            "PropertyName": { "Float32": 0.0 } // skip this property in all circumstances when it's this default value
        },
        "propertyFilters": {                 // optional
            "PropertyNameSample1": {
                "diff": "always", // optional, default
                "save": "always", // optional, default
                // diff controls whether a property can trigger a change.
                // save controls whether a property appears in json models and meta files.
            },
            "PropertyNameSample2": {
                "diff": "never", // never [diff/save] this property
                "save": {
                    "whenNotEqual": [ // skip this property if it equals any of these values
                        { "Float32": 0.0 }, // exact comparison is done, so typically for
                        { "Float64": 0.0 }, // numbers you should define all numerical types
                        { "Int32": 0 },
                        { "Int64": 0 }, // see https://rojo.space/docs/v7/properties for more fully qualified property samples
                    ]
                }
            },
        }
    }
}
```

```jsonc
{
    "snapshotRules": [ // optional
        // custom glob rules for what type of instance files should be turn into.
        {
            "use": "rojo/txt",      // required
            "include": ["*.txt"],   // required
            "exclude": ["docs/**"], // optional
        }
        // full list of types:
        //  "rojo/lua", "rojo/txt", "rojo/csv", "rojo/json", "rojo/toml",
        //  "rojo/json_model", "rojo/rbxm", "rojo/rbxmx",
        //  "rojo/project", "rojo/directory",
        // (project and directory are not really intended for external use, but they may work fine)
    ]
}
```

<details><summary>Release Instructions</summary>

New Uplift Games-specific releases should:
* Be created via [workflow dispatch on the Release action](https://github.com/UpliftGames/rojo/actions/workflows/release.yml)
  ![image](https://user-images.githubusercontent.com/1669436/233771073-ccbd1834-3341-4aeb-91cd-be7b02878b39.png)
  * Be created on the `uplift` branch _(this is our `main`)_
  * Be tagged with an appropriate semver **plus** a pre-release tag in the following format:\
    `v1.2.3-uplift.1`\
    ...where `v1.2.3` is the semver and `uplift.1` increments for each release we make.
    It is acceptable to maintain the release count across semver changes.
  * Once the release action finishes there will be a release draft. Add a changelog and publish it.
    If any release job fails due to aftman github limits, re-run failed jobs.
* Add our changes to `CHANGELOG.md`. If we rebase on a
  new version of Rojo that includes some of our additions, we should list only
  what has changed between upstream Rojo and our fork.
* Where possible, our changes should become PRs to the upstream Rojo repo. When
  we do this, we should include a link to the PR in the changelog entry.

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
rojo = "UpliftGames/rojo@7.3.0-uplift.12.pre.4"
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
