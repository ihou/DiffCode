# Contributing to DiffCode

Thanks for helping. Bug reports, ideas and pull requests are all welcome. For anything larger than a small fix, please open an issue first so we can agree on the approach before you spend time on it.

## Building and testing

```bash
swift run                 # debug build of the app
swift test                # unit tests and end-to-end tests on temporary repositories
Scripts/build-app.sh      # dist/DiffCode.app, signed ad hoc for this Mac
```

You need macOS 14 or later and a Swift toolchain (Xcode, or the Command Line Tools). The [user guide](docs/GUIDE.zh-CN.md) describes the code layout.

Debug builds can drive their own UI and capture their window without Screen Recording permission, which helps when testing a change or reproducing a bug. See the header of [`Sources/Dif/App/DebugHooks.swift`](Sources/Dif/App/DebugHooks.swift), for example:

```bash
DIF_ACTIONS="split;select:src/Main.kt" DIF_SNAPSHOT=/tmp/out.png .build/debug/Dif /path/to/a/test/repo
```

The Windows version is a Rust workspace in [`windows/`](windows/PORTING.md): `cargo test --workspace` there runs its tests on any platform, and [PORTING.md](windows/PORTING.md) explains how to build, screenshot and package it.

Test against throwaway repositories, never against one whose history you care about.

## What a change should keep true

- **No warnings.** The package builds with full concurrency checking and no warnings.
- **Light on memory.** Low memory use is the point of the app. If a change adds a view, a cache or a background task, measure the app's memory before and after (the `memory` debug action, or `vmmap`) and mention the numbers in the pull request.
- **Read-only until asked.** Refreshing and diffing must never write to a repository or take `index.lock`. Anything that writes happens only because the user asked, asks first when it is destructive, and offers undo where it can.
- **Both languages.** Interface text goes through `L("English", "简体中文")`. If you cannot write one of the two, say so in the pull request and someone will fill it in.
- **Tests for behaviour.** Git behaviour is tested end to end on temporary repositories (see `Tests/DifTests`). Please add a test with every fix or feature.

## Pull requests

Keep each pull request to one change, describe what users will notice, and add a screenshot for anything visible. Run `swift test` (and `cargo test --workspace` in `windows/` for Windows changes) before you send it.

## Contributor License Agreement

DiffCode is licensed under the GPL v3. To keep the option of publishing it in places whose terms are not compatible with the GPL, such as the Mac App Store, the project needs a little more than the GPL from contributors. By submitting a contribution (code, documentation, artwork or anything else) you agree that:

1. You created the contribution, or otherwise have the right to submit it under these terms.
2. The contribution is licensed to everyone under the GNU General Public License v3.0, like the rest of DiffCode.
3. You also grant the DiffCode maintainers a perpetual, worldwide, non-exclusive, royalty-free, irrevocable license to use, copy, modify, distribute and sublicense the contribution under other terms as well.

You keep the copyright to your contribution. Opening a pull request means you agree to these terms.

## Security issues

Please do not report vulnerabilities in public issues. See [SECURITY.md](SECURITY.md).
