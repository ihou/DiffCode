# Security policy

## Reporting a vulnerability

Please report security problems privately, not in a public issue: open the repository's **Security** tab and choose **Report a vulnerability** (GitHub's private vulnerability reporting). Include what you found, the steps to reproduce it, and the DiffCode version or commit.

You will get an answer as soon as possible. Once a fix is released, the advisory will be published with credit to you, unless you prefer otherwise.

## Supported versions

Fixes go into the latest release and the `main` branch.

## Worth knowing

Some things are by design rather than vulnerabilities:

- The built-in terminal runs your login shell with your own privileges, exactly like Terminal.app. Programs started there, including coding agents, can do whatever your account can.
- DiffCode runs your `git` with each repository's own configuration, and committing runs the repository's hooks, as `git commit` does on the command line. Treat an untrusted repository in DiffCode the way you would treat it in a terminal.
- The downloadable edition checks for updates with [Sparkle](https://sparkle-project.org). Updates are signed with an EdDSA key and the app is notarized by Apple; builds from source do not check for updates.
