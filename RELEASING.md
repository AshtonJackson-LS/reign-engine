# Releasing Reign

How a new version of Reign gets to users. You do two things: a one-time
setup, then **double-click `release.bat`** whenever you want to ship.

## How it fits together

- **The key belongs to the repository**, not to a person. It's stored in a
  GitHub *environment* called `release`, and only the publishing step can
  use it. Nobody can read it back out, not even the owner.
- **Pushing to `main` never releases anything.** Only a version tag (like
  `v0.2.0`) does, and `release.bat` is what makes one.
- **Anyone with write access can start a release**, but it stops before
  signing and waits for an approver (the owner) to click **Approve**. Until
  then nothing is signed or published, and the key is never handed out.
- **Forks never get the key.** GitHub doesn't copy secrets into forks. A fork
  that wants its own releases makes its own key (see "Shipping your own
  fork"), and its hubs then trust only that fork's releases.

## One-time setup (the repository owner)

Only the owner can do these, in the repository's **Settings**:

1. **Add collaborators** who will make releases: **Collaborators → Add
   people**.
2. **Make the `release` environment:** **Environments → New environment**,
   name it `release`.
3. **Require approval:** on that environment, tick **Required reviewers**,
   add yourself (and anyone else allowed to approve releases), and save.
4. **Add the key to the environment:** on the same page, under **Environment
   secrets → Add environment secret**, name it `REIGN_RELEASE_KEY`, paste the
   key's one line as the value, and save. Put it here, not under "Repository
   secrets": then only an approved release can use it.
5. **Back the key file up** somewhere safe, like a password manager, and
   never commit it or send it in a message. Whoever has it can push updates
   to every Reign user. If it's lost, see "Lost key" below.

GitHub Actions is already set up (`.github/workflows/release.yml`).

## Every release

1. Merge what you want to ship into `main`, then `git checkout main` and
   `git pull`.
2. Double-click **`release.bat`**. It asks for the new version (press Enter to
   take the suggestion) and checks with you before doing anything. Then it:
   - puts the version in `Cargo.toml`,
   - commits and tags it (for example `v0.2.0`),
   - pushes both to GitHub.
3. GitHub runs every test and builds Windows (about 20 minutes). Watch it
   under the repository's **Actions** tab.
4. **The approver approves.** The run stops at "Sign and publish", and GitHub
   emails the approvers. They open the run, click **Review deployments →
   release → Approve and deploy**. Rejecting it ends the release with nothing
   published.
5. GitHub signs the update, publishes the release, then adds the macOS and
   Linux builds. Users download the new version, and installed hubs show the
   green update arrow the next time they open.

### Picking the version number

| You changed… | Example | Bump |
| --- | --- | --- |
| Only bug fixes | 0.2.0 → 0.2.1 | last number |
| Added features | 0.2.1 → 0.3.0 | middle number |
| Broke old projects or scenes | 0.3.0 → 1.0.0 | first number |

## What users get

- **New users:** the download link
  `https://github.com/AshtonJackson-LS/reign-engine/releases/latest/download/ReignSetup.exe`
  always points at the newest installer. Running it installs Reign Hub to
  the Start menu and the Desktop, with an Uninstall entry in Settings → Apps.
  No administrator rights are needed.
- **Existing users:** a green arrow in the hub's top bar. One click
  downloads, checks the signature and restarts on the new version.
- **Projects** stay on the engine version they were made with until someone
  changes it in the hub (••• → Engine version…).

## If something goes wrong

| Problem | Fix |
| --- | --- |
| `release.bat` says "pushing failed" | You don't have write access yet (one-time setup, step 1). Your commit and tag are kept: once you have access, run the `git push` line it printed. |
| The run waits at "Sign and publish" | It's waiting for approval (Every release, step 4). The approvers got an email. |
| The run fails at "The release key is set" | The `release` environment has no `REIGN_RELEASE_KEY` (one-time setup, step 4). Add it, then on the failed run click **Re-run failed jobs**. The run's summary page lists the steps. |
| The run fails at "Sign" | The secret isn't the partner of the key the hub trusts. Paste the key file's line into the secret again, then re-run. In a fork, check the `REIGN_UPDATE_PUBKEY` variable. |
| The Actions run fails at clippy or a test | Nothing was published. Fix it on `main`, then run `release.bat` again with the next version (for example 0.2.1). |
| Only the macOS or Linux job failed | The Windows release is already out. Fix it later; the next release brings those builds back. |
| Users see "Windows protected your PC" | Normal for an installer that isn't code-signed: **More info → Run anyway**. Code signing removes it (SignPath offers it free to open-source projects). |
| Lost or leaked key | Make a new key: `cargo run -p reign_hub --bin reign-release -- keygen C:\somewhere\new.key`. Put the printed public key in `OFFICIAL_PUBLIC_KEY` in `hub/src/signing.rs`, replace the environment secret, and release. Hubs from before can't verify the new key, so their users reinstall once with ReignSetup.exe. |

## Shipping your own fork

A fork starts with no key, and its release runs stop with steps to follow.
A hub built from an unchanged fork still follows and trusts the official
releases. To give your fork its own releases:

1. Make a key, outside the repository:
   `cargo run -p reign_hub --bin reign-release -- keygen C:\somewhere\my-reign.key`.
   It prints your public key and these same steps.
2. In your fork's **Settings → Environments**, make `release` (add required
   reviewers if you want approvals). Add the environment secret
   `REIGN_RELEASE_KEY`: the line in your key file.
3. In **Settings → Secrets and variables → Actions → Variables**, add the
   repository variable `REIGN_UPDATE_PUBKEY`: the public key from step 1.
4. Run `release.bat`.

Nothing in the code needs to change. The release workflow builds your
hubs with your repository's name and your public key. They look for updates
in your fork, trust only your signatures, and new projects fetch the engine
from your fork. Official hubs never trust your releases, and yours never
trust official ones.

Building a fork's hub on your own machine (not through a release)? Set the
same two values first:

```bash
set REIGN_UPDATE_REPO=you/your-fork
set REIGN_UPDATE_PUBKEY=<your public key>
```

## Testing a build without releasing

- **`update-reign.bat`** builds everything from your folder and installs it
  on this computer, the same way `ReignSetup.exe` does for users.
- **`scripts\build-setup.ps1`** only builds `dist\ReignSetup.exe`, so you can
  try the installer or send it to someone.
- **`scripts\release.ps1 -DryRun`** does a whole release locally (version,
  commit, tag) without pushing. It prints how to undo it.
