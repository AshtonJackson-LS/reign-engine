# Releasing Reign

How changes get from a pull request to everyone's Reign Hub. After a
one-time setup, nobody runs anything on their own computer: GitHub builds,
signs and publishes, and installed hubs update themselves.

## How it fits together

```
feature branch ──PR──► main ──► beta release (v0.3.0-beta.4)  → hubs with Beta updates on
                         │
                         └────► "Release v0.3.0" PR ──merge──► stable release (v0.3.0) → every hub
```

1. **Work happens on branches.** Each change is a pull request into `main`.
   CI (`ci.yml`) builds and tests it on Windows, macOS and Linux.
2. **Label the pull request** so the release knows what it is:
   `breaking` (old projects or scenes stop working), `feature`
   (something new) or `fix`. Unlabelled changes count as small ones.
3. **Every merge into `main` makes a beta.** `release.yml` tests it, builds
   Windows (x64, ARM64), macOS (Apple Silicon and Intel) and Linux (x64,
   ARM64), signs everything and publishes a pre-release such as
   `v0.3.0-beta.4`. Hubs with **Engine → Beta release** (About page) install it.
   Only the newest three betas are kept.
4. **The Release PR keeps itself up to date.** The same run opens or
   updates a pull request called **Release v0.3.0**. It holds the version
   bump and a `CHANGELOG.md` entry listing the merged pull requests. The
   labels decide the version: any `breaking` → next major, any `feature` →
   next minor, otherwise next patch.
5. **Merging the Release PR ships it.** That push builds, signs and
   publishes `v0.3.0` as a stable release. Every hub installs it in the
   background and shows a green arrow to restart. The README's download
   links always point at the newest stable release.

Projects stay on their engine version until someone changes it (••• →
Engine version…). The version picker only offers stable releases.

How a push is classified lives in `scripts/plan-release.sh`; the Release PR
is written by `scripts/update-release-pr.sh`.

## One-time setup (the repository owner)

In the repository's **Settings**:

1. **Let Actions open the Release PR:** **Actions → General → Workflow
   permissions**: tick **Allow GitHub Actions to create and approve pull
   requests**, and save.
2. **Make the `release` environment:** **Environments → New environment**,
   name it `release`. Leave **Required reviewers** off: merging the Release
   PR is the approval, and with reviewers on every beta would wait for one.
3. **Add the signing key:** on that environment, **Add environment secret**
   named `REIGN_RELEASE_KEY`, with the key file's one line as the value. Back
   the key file up somewhere safe (a password manager) and never commit it:
   whoever has it can push updates to every Reign user. No key yet? Make one
   with `cargo run -p reign_hub --bin reign-release -- keygen C:\somewhere\reign-release.key`.
4. **Protect `main`:** **Branches → Add branch ruleset** (or **Add rule**)
   for `main`: require a pull request before merging, and require the
   status checks `Build and test (ubuntu-latest)`, `Build and test
   (windows-latest)`, `Build and test (macos-latest)` and `License and
   advisory check`. Then nothing reaches users without passing CI.
5. **Add collaborators** under **Collaborators → Add people**. Anyone who
   can merge into `main` can ship betas, and merge the Release PR.

## Every release

1. Merge pull requests into `main` as usual. Each one becomes a beta within
   about 20 minutes (watch the **Actions** tab).
2. When you want to ship, open the **Release v…** pull request, check the
   changelog, fix any labels on the merged pull requests if the version
   looks wrong (the PR updates on the next merge), and merge it.

That's all. To check a beta yourself first, set **Engine** to **Beta release** in your
hub's About page.

## If something goes wrong

| Problem | Fix |
| --- | --- |
| No Release PR appears | Setup step 1 is missing. Then re-run the latest **Release** run (**Actions → Release → Re-run all jobs**). |
| The run fails at "Sign" with "No REIGN_RELEASE_KEY" | Setup step 3. Add the secret, then **Re-run failed jobs**. |
| The run fails at "Sign" with "doesn't match" | The secret isn't the partner of the public key the hub trusts. Paste the key file's line into the secret again and re-run. In a fork, check the `REIGN_UPDATE_PUBKEY` variable. |
| The run waits at "Sign and publish" | The `release` environment has required reviewers. Approve the run, or turn reviewers off (setup step 2). |
| A test or build fails | Nothing was published. Fix it in a new pull request; its merge makes the next beta. A GitHub hiccup instead? **Re-run failed jobs**. |
| The Release PR has no checks | Its CI run is started by the workflow; look under **Actions → CI** for the `release/next` branch, or run CI there by hand (**Run workflow**). |
| The version in the Release PR is wrong | Fix the labels on the merged pull requests; the next merge into `main` rewrites the Release PR. |
| Users see "Windows protected your PC" | Normal for an installer that isn't code-signed: **More info → Run anyway**. Code signing removes it (SignPath offers it free to open-source projects). |
| Lost or leaked key | Make a new key with `keygen` (setup step 3). Put the printed public key in `OFFICIAL_PUBLIC_KEY` in `hub/src/signing.rs`, replace the environment secret, and release. Hubs from before can't verify the new key, so their users reinstall once. |

## Shipping your own fork

A fork starts with no key, so its runs stop at "Sign". To give it its own
releases:

1. Make a key, outside the repository:
   `cargo run -p reign_hub --bin reign-release -- keygen C:\somewhere\my-reign.key`.
   It prints your public key.
2. Do the one-time setup above in your fork, with that key.
3. In **Settings → Secrets and variables → Actions → Variables**, add the
   repository variable `REIGN_UPDATE_PUBKEY`: your public key.

The workflow builds your hubs with your repository's name and public key, so
they follow and trust only your releases. Building a hub by hand instead? Set
`REIGN_UPDATE_REPO=you/your-fork` and `REIGN_UPDATE_PUBKEY=<your public key>`
first.

## Building without releasing

- **`update-reign.bat`** builds everything from your folder and installs it
  on this computer, the way `ReignSetup.exe` does for users.
- **`scripts\build-setup.ps1`** builds `dist\ReignSetup.exe` and the update
  zip (`-Arch arm64` for ARM PCs). On a Mac, `scripts/bundle-macos.sh`; on
  Linux, `scripts/package-linux.sh`.
- **`scripts/plan-release.sh`** prints what the next push to `main` would
  release, from your local history (without `gh`, it can't read labels).
